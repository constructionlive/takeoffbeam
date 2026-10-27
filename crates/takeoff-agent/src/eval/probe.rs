//! The vision accuracy probe: render one view the way the agent's `view` tool
//! does, captioned in a given coordinate dialect, ask the model to box known
//! targets, and measure how far its boxes land from the truth. Each reply is
//! also read in the other dialects, so a model answering in a convention it
//! wasn't asked for shows up as "best reading".

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use takeoff_core::coords::{dialect_for, region_to_page, Dialect};
use takeoff_core::geometry::iou;
use takeoff_core::{sidecar, Document, TakeoffData};

use super::{dialect_name, Case, Target, DIALECTS};
use crate::tools::{self, ToolCtx, View};
use crate::{llm, ProviderConfig};

/// One target question on one view, for one model and dialect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeRow {
	pub case: String,
	pub model: String,
	pub dialect: String,
	/// The dialect Takeoff Desk picks for this model today.
	pub native: bool,
	pub ask: String,
	/// The view's size as the model saw it.
	pub view_px: [u32; 2],
	pub targets: usize,
	/// Boxes the model gave.
	pub found: usize,
	/// Boxes whose centre landed within one target size of a target's centre (one-to-one).
	pub hits: usize,
	/// Centre error of the hits, in view pixels and in % of the view's long edge.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub median_err_px: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub median_err_pct: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mean_iou: Option<f64>,
	/// The dialect that reads this reply best, and its hits.
	pub best_reading: String,
	pub best_hits: usize,
	pub wall_s: f64,
	pub prompt_tokens: u64,
	pub completion_tokens: u64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
	#[serde(default)]
	pub reply: String,
}

/// Probe every target of `case` with `provider`, asking in `dialect`.
pub fn probe_case(case: &Case, provider: &ProviderConfig, dialect: Dialect) -> Vec<ProbeRow> {
	let base = ProbeRow { case: case.name.clone(), model: provider.model.clone(), dialect: dialect_name(dialect).into(), native: dialect_for(&provider.model) == dialect, ..Default::default() };
	let fail = |e: String| vec![ProbeRow { error: Some(e), ..base.clone() }];
	let Some(probe) = &case.probe else { return vec![] };
	let pdf = match case.pdf_path() {
		Ok(p) => p,
		Err(e) => return fail(e),
	};
	let drawing = match takeoff_pdf::Drawing::open(&pdf) {
		Ok(d) => d,
		Err(e) => return fail(format!("could not open {}: {e}", pdf.display())),
	};
	if case.page == 0 || case.page > drawing.page_count() {
		return fail(format!("page {} does not exist", case.page));
	}
	let sizes = (1..=drawing.page_count()).map(|p| drawing.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect();
	// The truth, before the drawing goes to the tools.
	let mut truth = Vec::new();
	for t in &probe.targets {
		match target_boxes(t, &pdf, &drawing, case.page) {
			Ok(b) if !b.is_empty() => truth.push(b),
			Ok(_) => return fail(format!("target \"{}\" has no boxes", t.ask)),
			Err(e) => return fail(e),
		}
	}
	// An empty takeoff, so nothing marked by hand shows in the render.
	let doc = std::sync::Arc::new(parking_lot::Mutex::new(Document::new(TakeoffData::default(), sizes, "probe")));
	let mut ctx = ToolCtx::new(doc, drawing, &provider.model, None);
	ctx.dialect = dialect;
	ctx.current_page = case.page;
	let mut args = json!({"page": case.page, "show_marks": false, "show_measurements": false, "show_annotations": false, "show_overlays": false});
	if let Some([x, y, w, h]) = probe.region {
		args["region"] = json!({"x": x, "y": y, "w": w, "h": h});
	}
	let out = match tools::call(&mut ctx, "view", args) {
		Ok(o) => o,
		Err(e) => return fail(format!("view failed: {e}")),
	};
	let Some(img) = out.images.into_iter().next() else { return fail("view gave no image".into()) };
	let Ok(view) = ctx.view(&img.view_id) else { return fail("view not registered".into()) };
	let mut rows = Vec::new();
	for (t, gt) in probe.targets.iter().zip(truth) {
		let mut row = ProbeRow { ask: t.ask.clone(), view_px: [img.width, img.height], targets: gt.len(), ..base.clone() };
		let prompt = format!(
			"{}\n\nThis is part of a construction drawing. Find {}. Respond with ONLY a JSON array, one entry per match, each with a \"label\". {} If there are none, answer [].",
			img.caption,
			t.ask,
			dialect.box_request(img.width, img.height)
		);
		let msgs = vec![json!({"role": "user", "content": [{"type": "text", "text": prompt}, llm::image_part(&img.png, &provider.model)]})];
		let t0 = Instant::now();
		let reply = llm::complete_with(provider, &msgs, Some(4096), true);
		row.wall_s = t0.elapsed().as_secs_f64();
		match reply {
			Ok((text, pt, ct)) => {
				(row.prompt_tokens, row.completion_tokens) = (pt, ct);
				score_reply(&mut row, &text, &view, &gt, dialect);
				row.reply = text.chars().take(1500).collect();
			}
			Err(e) => row.error = Some(e),
		}
		rows.push(row);
	}
	rows
}

/// Where a target's answers are, in page fractions.
fn target_boxes(t: &Target, pdf: &std::path::Path, drawing: &takeoff_pdf::Drawing, page: u32) -> Result<Vec<[f64; 4]>, String> {
	let mut out = t.boxes.clone();
	if let Some(item) = &t.item {
		let data = sidecar::load(pdf).map_err(|e| format!("could not read the takeoff next to {}: {e}", pdf.display()))?;
		let want = item.to_lowercase();
		let ids: Vec<&str> = data.categories.iter().filter(|c| c.name.to_lowercase() == want).map(|c| c.id.as_str()).collect();
		if ids.is_empty() {
			return Err(format!("no item named \"{item}\" in the takeoff next to {}", pdf.display()));
		}
		out.extend(data.marks.iter().filter(|m| m.page == page && m.category_id.as_deref().is_some_and(|c| ids.contains(&c))).map(|m| [m.x, m.y, m.w, m.h]));
	}
	if let Some(text) = &t.text {
		let hits = drawing.search_page(page, text, true, false, 100).map_err(|e| e.to_string())?;
		if hits.is_empty() {
			return Err(format!("text \"{text}\" is not on page {page}"));
		}
		out.extend(hits.iter().map(|h| [h.rect.0, h.rect.1, h.rect.2, h.rect.3]));
	}
	Ok(out)
}

/// A page-fraction box → (x, y, w, h) in the view's pixels.
fn to_view_px(v: &View, b: [f64; 4]) -> (f64, f64, f64, f64) {
	let (sx, sy) = (v.seen_w as f64 / v.region.w, v.seen_h as f64 / v.region.h);
	((b[0] - v.region.x) * sx, (b[1] - v.region.y) * sy, b[2] * sx, b[3] * sy)
}

/// The reply's boxes read in `d`, as page fractions.
fn read_as(reply: &str, v: &View, d: Dialect) -> Vec<[f64; 4]> {
	d.parse_boxes(reply)
		.into_iter()
		.map(|b| {
			let b = takeoff_core::coords::RawBox { x1: b.x1.min(b.x2), y1: b.y1.min(b.y2), x2: b.x1.max(b.x2), y2: b.y1.max(b.y2), label: None };
			let f = d.box_to_fractions(b, v.seen_w as f64, v.seen_h as f64);
			let p = region_to_page((v.region.x, v.region.y, v.region.w, v.region.h), f);
			[p.0, p.1, p.2, p.3]
		})
		.collect()
}

/// Greedy one-to-one matching, closest centres first. A hit: the centre is
/// within one target size (its longer side, at least 4 px) of the target's.
/// Returns (centre error px, IoU) per hit.
pub(crate) fn match_boxes(v: &View, gt: &[[f64; 4]], pred: &[[f64; 4]]) -> Vec<(f64, f64)> {
	let centre = |b: (f64, f64, f64, f64)| (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
	let mut pairs = Vec::new();
	for (i, g) in gt.iter().enumerate() {
		let g = to_view_px(v, *g);
		let reach = g.2.max(g.3).max(4.0);
		for (j, p) in pred.iter().enumerate() {
			let p = to_view_px(v, *p);
			let (a, b) = (centre(g), centre(p));
			let d = (a.0 - b.0).hypot(a.1 - b.1);
			if d <= reach {
				pairs.push((d, i, j, iou(g, p)));
			}
		}
	}
	pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
	let (mut used_g, mut used_p) = (vec![false; gt.len()], vec![false; pred.len()]);
	let mut out = Vec::new();
	for (d, i, j, o) in pairs {
		if !used_g[i] && !used_p[j] {
			used_g[i] = true;
			used_p[j] = true;
			out.push((d, o));
		}
	}
	out
}

fn median(mut v: Vec<f64>) -> Option<f64> {
	if v.is_empty() {
		return None;
	}
	v.sort_by(f64::total_cmp);
	let n = v.len();
	Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

fn score_reply(row: &mut ProbeRow, reply: &str, v: &View, gt: &[[f64; 4]], asked: Dialect) {
	let pred = read_as(reply, v, asked);
	let hits = match_boxes(v, gt, &pred);
	let long = v.seen_w.max(v.seen_h) as f64;
	row.found = pred.len();
	row.hits = hits.len();
	row.median_err_px = median(hits.iter().map(|h| h.0).collect());
	row.median_err_pct = row.median_err_px.map(|p| p / long * 100.0);
	row.mean_iou = (!hits.is_empty()).then(|| hits.iter().map(|h| h.1).sum::<f64>() / hits.len() as f64);
	let (mut best, mut best_hits) = (asked, hits.len());
	for d in DIALECTS.into_iter().filter(|d| *d != asked) {
		let n = match_boxes(v, gt, &read_as(reply, v, d)).len();
		if n > best_hits {
			(best, best_hits) = (d, n);
		}
	}
	row.best_reading = dialect_name(best).into();
	row.best_hits = best_hits;
}

/// One model in one dialect, over every target probed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
	pub model: String,
	pub dialect: String,
	pub native: bool,
	pub targets: usize,
	pub found: usize,
	pub hits: usize,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub median_err_pct: Option<f64>,
	pub errors: usize,
	/// "full takeoff", "assist" or "not usable".
	pub verdict: String,
}

/// Recall ≥ 90%, precision ≥ 80% and median error ≤ 1.5% of the view: the
/// model can place marks itself. Recall ≥ 50%: it can assist (find, read,
/// check) while detection runs elsewhere. Below that it should not place marks.
pub fn verdicts(rows: &[ProbeRow]) -> Vec<Verdict> {
	let mut keys: Vec<(String, String)> = Vec::new();
	for r in rows {
		let k = (r.model.clone(), r.dialect.clone());
		if !keys.contains(&k) {
			keys.push(k);
		}
	}
	keys.into_iter()
		.map(|(model, dialect)| {
			let rs: Vec<&ProbeRow> = rows.iter().filter(|r| r.model == model && r.dialect == dialect).collect();
			let targets: usize = rs.iter().map(|r| r.targets).sum();
			let found: usize = rs.iter().map(|r| r.found).sum();
			let hits: usize = rs.iter().map(|r| r.hits).sum();
			let errors = rs.iter().filter(|r| r.error.is_some()).count();
			// Pool the per-question medians weighted by hits.
			let pooled: Vec<f64> = rs.iter().filter_map(|r| r.median_err_pct.map(|e| std::iter::repeat_n(e, r.hits))).flatten().collect();
			let err = median(pooled);
			let recall = if targets > 0 { hits as f64 / targets as f64 } else { 0.0 };
			let precision = if found > 0 { hits as f64 / found as f64 } else { 0.0 };
			let verdict = if recall >= 0.9 && precision >= 0.8 && err.is_some_and(|e| e <= 1.5) {
				"full takeoff"
			} else if recall >= 0.5 {
				"assist"
			} else {
				"not usable"
			};
			Verdict { native: rs.iter().any(|r| r.native), model, dialect, targets, found, hits, median_err_pct: err, errors, verdict: verdict.into() }
		})
		.collect()
}

/// The probe as tables: every question, then per model and dialect, then
/// one line per model answering "full takeoff or assist?".
pub fn probe_table(rows: &[ProbeRow]) -> String {
	let mut s = format!("{:<22} {:<24} {:<7} {:<30} {:>5} {:>5} {:>8} {:>7} {:>5}  {}\n", "case", "model", "dialect", "target", "hits", "found", "err px", "err %", "IoU", "best reading");
	for r in rows {
		let f = |v: Option<f64>, p: usize| v.map(|v| format!("{v:.p$}")).unwrap_or_else(|| "-".into());
		s.push_str(&format!(
			"{:<22} {:<24} {:<7} {:<30} {:>5} {:>5} {:>8} {:>7} {:>5}  {}\n",
			short(&r.case, 22),
			short(&r.model, 24),
			format!("{}{}", r.dialect, if r.native { "*" } else { "" }),
			short(&r.ask, 30),
			format!("{}/{}", r.hits, r.targets),
			r.found,
			f(r.median_err_px, 1),
			f(r.median_err_pct, 2),
			f(r.mean_iou, 2),
			r.error.as_deref().map(|e| format!("error: {}", short(e, 60))).unwrap_or_else(|| format!("{} ({})", r.best_reading, r.best_hits))
		));
	}
	s.push_str("* = the dialect Takeoff Desk uses for this model today\n\n");
	let vs = verdicts(rows);
	s.push_str(&format!("{:<24} {:<7} {:>11} {:>9} {:>7}  {}\n", "model", "dialect", "recall", "precision", "err %", "verdict"));
	for v in &vs {
		let pct = |a: usize, b: usize| if b > 0 { format!("{:.0}%", a as f64 / b as f64 * 100.0) } else { "-".into() };
		s.push_str(&format!(
			"{:<24} {:<7} {:>11} {:>9} {:>7}  {}{}\n",
			short(&v.model, 24),
			format!("{}{}", v.dialect, if v.native { "*" } else { "" }),
			format!("{} {}", pct(v.hits, v.targets), format_args!("{}/{}", v.hits, v.targets)),
			pct(v.hits, v.found),
			v.median_err_pct.map(|e| format!("{e:.2}")).unwrap_or_else(|| "-".into()),
			v.verdict,
			if v.errors > 0 { format!(" ({} failed calls)", v.errors) } else { String::new() }
		));
	}
	let mut models: Vec<&str> = Vec::new();
	for v in &vs {
		if !models.contains(&v.model.as_str()) {
			models.push(&v.model);
		}
	}
	let level = |v: &Verdict| ["not usable", "assist", "full takeoff"].iter().position(|l| *l == v.verdict).unwrap_or(0);
	let rank = |v: &Verdict| (level(v), v.hits, std::cmp::Reverse((v.median_err_pct.unwrap_or(f64::MAX) * 1000.0) as u64));
	for m in models {
		let mine: Vec<&Verdict> = vs.iter().filter(|v| v.model == m).collect();
		let native = mine.iter().find(|v| v.native);
		let best = mine.iter().max_by_key(|v| rank(v));
		let line = match (native, best) {
			(Some(n), Some(b)) if level(b) > level(n) => format!("{}: {} in its current dialect ({}); {} in {}: consider switching", m, n.verdict, n.dialect, b.verdict, b.dialect),
			(Some(n), _) => format!("{}: {} ({})", m, n.verdict, n.dialect),
			(None, Some(b)) => format!("{}: {} at best ({}; its current dialect was not probed)", m, b.verdict, b.dialect),
			_ => continue,
		};
		s.push_str(&format!("→ {line}\n"));
	}
	s
}

fn short(s: &str, n: usize) -> String {
	if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}

/// For tests: a reply in `d` that boxes `boxes` (page fractions) exactly on view `v`.
pub fn reply_for(v: (f64, f64, f64, f64), seen: (u32, u32), d: Dialect, boxes: &[[f64; 4]]) -> String {
	let (sx, sy) = if d.is_grid() { (1000.0, 1000.0) } else { (seen.0 as f64, seen.1 as f64) };
	let items: Vec<Value> = boxes
		.iter()
		.map(|b| {
			let (x1, y1) = ((b[0] - v.0) / v.2 * sx, (b[1] - v.1) / v.3 * sy);
			let (x2, y2) = ((b[0] + b[2] - v.0) / v.2 * sx, (b[1] + b[3] - v.1) / v.3 * sy);
			match d {
				Dialect::Grid1000Yx => json!({"box_2d": [y1, x1, y2, x2], "label": "x"}),
				Dialect::Grid1000Xy => json!({"bbox_2d": [x1, y1, x2, y2], "label": "x"}),
				Dialect::Pixel => json!({"box": [x1, y1, x2, y2], "label": "x"}),
			}
		})
		.collect();
	Value::Array(items).to_string()
}
