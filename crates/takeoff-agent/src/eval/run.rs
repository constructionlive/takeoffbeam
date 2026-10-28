//! Run one case's task through the real agent loop, headless, then score the
//! takeoff it left against the case's expectations.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use takeoff_core::geometry::{scale_units_per_px, unit_key};
use takeoff_core::units::{length_factor, ScalePreset};
use takeoff_core::{Document, MeasurementType, TakeoffData};

use super::{Case, Expect};
use crate::session::Msg;
use crate::channel::{self, Channel, ChannelTurn, ToolHost};
use crate::{start_turn, AgentContext, AgentEvent, ChatSession, ProviderConfig};

pub struct RunOptions {
	pub provider: ProviderConfig,
	/// Cancel a case that runs longer (a case's own `timeout_secs` wins).
	pub timeout: Duration,
	/// Folder to give the agent a shell in; None = no shell.
	pub workspace: Option<PathBuf>,
	/// Save each case's resulting takeoff here as `<case>.takeoff.json`.
	pub keep: Option<PathBuf>,
	/// Run on a subscription CLI (Claude Code, Codex) with this model ("" = its
	/// default) instead of `provider`, the tools served over MCP as in the app.
	pub channel: Option<(Channel, String)>,
}

/// One expectation, scored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
	pub what: String,
	pub expected: String,
	pub got: String,
	/// Signed error against the expected value, where there is a number.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub err_pct: Option<f64>,
	pub pass: bool,
}

/// One case run against one model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseResult {
	pub case: String,
	pub model: String,
	pub base_url: String,
	/// 1-based, when a case runs more than once.
	#[serde(default)]
	pub repeat: u32,
	/// Not run (drawing missing, no task…).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub skipped: Option<String>,
	/// The run ended badly (model error, timeout); it is still scored.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
	pub wall_s: f64,
	pub steps: u32,
	pub prompt_tokens: u64,
	pub completion_tokens: u64,
	pub tool_calls: u32,
	pub tool_errors: u32,
	/// Calls per tool.
	#[serde(default)]
	pub tools: BTreeMap<String, u32>,
	pub passed: usize,
	pub total: usize,
	pub scores: Vec<Score>,
	/// The agent's last reply.
	#[serde(default)]
	pub answer: String,
}

/// Run `case`'s task. `on_event` sees the agent's events as they come (progress).
pub fn run_case(case: &Case, opts: &RunOptions, on_event: &mut dyn FnMut(&AgentEvent)) -> CaseResult {
	let mut r = match &opts.channel {
		Some((ch, model)) => CaseResult { case: case.name.clone(), model: format!("{}:{}", ch.key(), if model.is_empty() { "default" } else { model }), base_url: ch.name().into(), ..Default::default() },
		None => CaseResult { case: case.name.clone(), model: opts.provider.model.clone(), base_url: opts.provider.base_url.clone(), ..Default::default() },
	};
	let Some(task) = &case.task else {
		r.skipped = Some("no task".into());
		return r;
	};
	let pdf = match case.pdf_path() {
		Ok(p) => p,
		Err(e) => {
			r.skipped = Some(e);
			return r;
		}
	};
	let sizes = match page_sizes(&pdf) {
		Ok(s) => s,
		Err(e) => {
			r.skipped = Some(e);
			return r;
		}
	};
	if case.page == 0 || case.page as usize > sizes.len() {
		r.skipped = Some(format!("page {} does not exist ({} pages)", case.page, sizes.len()));
		return r;
	}
	r.total = task.expect.len();
	// A fresh takeoff: never the drawing's own sidecar, which may hold the answers.
	let mut data = TakeoffData::default();
	if let Some(label) = &task.setup.scale {
		match ScalePreset::all().find(|p| p.label == label.as_str()) {
			Some(p) => data.page_scales.push(p.to_page_scale(case.page, sizes[case.page as usize - 1].0)),
			None => {
				r.skipped = Some(format!("unknown scale preset '{label}'"));
				return r;
			}
		}
	}
	let doc = Arc::new(Mutex::new(Document::new(data, sizes, "AI")));
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let timeout = task.timeout_secs.map(Duration::from_secs).unwrap_or(opts.timeout);
	let t0 = Instant::now();
	// Held for the turn: the CLI's tool calls come back through it.
	let mut _host = None;
	let turn = match &opts.channel {
		Some((ch, model)) => {
			let Some(exe) = channel::find_exe(*ch) else {
				r.skipped = Some(format!("{} is not installed", ch.name()));
				return r;
			};
			let host = match ToolHost::start(doc.clone(), pdf.clone()) {
				Ok(h) => h,
				Err(e) => {
					r.skipped = Some(e);
					return r;
				}
			};
			let workdir = std::env::temp_dir().join(format!("takeoff-eval-{}", ch.key()));
			let _ = std::fs::create_dir_all(&workdir);
			let ct = ChannelTurn { channel: *ch, exe, model: model.clone(), workdir, workspace: opts.workspace.clone(), others: vec![], attachments: vec![], images: vec![], sweeps: None, current_page: case.page, visible_region: None, selected_ids: vec![] };
			let t = channel::start_turn(&host, session.clone(), ct, task.prompt.clone());
			_host = Some(host);
			t
		}
		None => {
			let ctx = AgentContext { doc: doc.clone(), pdf_path: pdf, current_page: case.page, visible_region: None, selected_ids: vec![], provider: opts.provider.clone(), workspace: opts.workspace.clone(), others: vec![], attachments: vec![], images: vec![] };
			start_turn(session.clone(), ctx, task.prompt.clone())
		}
	};
	let mut said = String::new();
	loop {
		if t0.elapsed() > timeout && !turn.is_cancelled() {
			turn.cancel();
			r.error = Some(format!("timed out after {}s", timeout.as_secs()));
		}
		let ev = match turn.events.recv_timeout(Duration::from_millis(500)) {
			Ok(e) => e,
			Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
			Err(_) => break,
		};
		on_event(&ev);
		match &ev {
			AgentEvent::TextDelta(t) => said.push_str(t),
			AgentEvent::ToolStarted { name, .. } => {
				r.tool_calls += 1;
				*r.tools.entry(name.clone()).or_default() += 1;
			}
			AgentEvent::ToolFinished { ok: false, .. } => r.tool_errors += 1,
			AgentEvent::Usage { prompt_tokens, completion_tokens } => {
				r.prompt_tokens += prompt_tokens;
				r.completion_tokens += completion_tokens;
			}
			AgentEvent::Error(e) if r.error.is_none() => r.error = Some(e.clone()),
			AgentEvent::Done { steps } => {
				r.steps = *steps;
				break;
			}
			_ => {}
		}
	}
	r.wall_s = t0.elapsed().as_secs_f64();
	r.answer = session.lock().messages.iter().rev().find_map(|m| match m {
		Msg::Assistant { text, .. } if !text.trim().is_empty() => Some(text.trim().chars().take(4000).collect()),
		_ => None,
	}).unwrap_or_default();
	let d = doc.lock();
	r.scores = score(&task.expect, &d, case.page, &said);
	r.passed = r.scores.iter().filter(|s| s.pass).count();
	if let Some(dir) = &opts.keep {
		let name: String = case.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
		let suffix = if r.repeat > 1 { format!("-{}", r.repeat) } else { String::new() };
		let _ = std::fs::create_dir_all(dir);
		if let Ok(json) = serde_json::to_string_pretty(&d.data) {
			let _ = std::fs::write(dir.join(format!("{name}{suffix}.takeoff.json")), json);
		}
	}
	r
}

fn page_sizes(pdf: &std::path::Path) -> Result<Vec<(f64, f64)>, String> {
	let d = takeoff_pdf::Drawing::open(pdf).map_err(|e| format!("could not open {}: {e}", pdf.display()))?;
	Ok((1..=d.page_count()).map(|p| d.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect())
}

/// Does a category name or label match the expectation's `item`?
fn matches(item: &Option<String>, names: &[Option<&str>]) -> bool {
	let Some(item) = item.as_deref().map(str::to_lowercase).filter(|s| !s.trim().is_empty()) else { return true };
	names.iter().flatten().any(|n| {
		let n = n.to_lowercase();
		!n.is_empty() && (n.contains(item.trim()) || item.contains(n.trim()))
	})
}

/// `value` in `from` units (ft, ft², sf, m2…) → `to`, if they are the same kind.
fn convert(value: f64, from: &str, to: &str) -> Option<f64> {
	let split = |u: &str| {
		let k = unit_key(u);
		match k.strip_suffix('2') {
			Some(b) => (b.to_string(), 2),
			None => (k, 1),
		}
	};
	let ((fb, fp), (tb, tp)) = (split(from), split(to));
	(fp == tp).then_some(())?;
	Some(value * length_factor(&fb, &tb)?.powi(fp))
}

fn err_pct(got: f64, want: f64) -> Option<f64> {
	(want != 0.0).then(|| (got - want) / want * 100.0)
}

fn normalize(s: &str) -> String {
	s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect()
}

/// Score expectations against the takeoff on `page` and what the agent said.
pub fn score(expect: &[Expect], doc: &Document, page: u32, said: &str) -> Vec<Score> {
	let d = &doc.data;
	let cat = |id: &Option<String>| id.as_deref().and_then(|id| d.category(id)).map(|c| c.name.as_str());
	let item_label = |item: &Option<String>| item.clone().unwrap_or_else(|| "all".into());
	expect
		.iter()
		.map(|e| match e {
			Expect::Count { item, value, tol } => {
				let n = d.marks.iter().filter(|m| m.page == page && matches(item, &[cat(&m.category_id)])).count() as f64;
				Score { what: format!("count {}", item_label(item)), expected: format!("{value}"), got: format!("{n}"), err_pct: err_pct(n, *value), pass: (n - value).abs() <= *tol + 1e-9 }
			}
			Expect::Length { item, value, unit, tol_pct } | Expect::Area { item, value, unit, tol_pct } => {
				let kind = if matches!(e, Expect::Length { .. }) { MeasurementType::Length } else { MeasurementType::Area };
				let mut total = 0.0;
				let mut n = 0;
				let mut unscaled = 0;
				for m in d.measurements.iter().filter(|m| m.page == page && m.kind == kind && matches(item, &[cat(&m.category_id), m.label.as_deref()])) {
					match (m.value, m.unit.as_deref()) {
						(Some(v), Some(u)) => match convert(v, u, unit) {
							Some(v) => {
								total += v;
								n += 1;
							}
							None => unscaled += 1,
						},
						_ => unscaled += 1,
					}
				}
				let word = if kind == MeasurementType::Length { "length" } else { "area" };
				let got = match (n, unscaled) {
					(0, 0) => "nothing matching".into(),
					(0, u) => format!("{u} unscaled"),
					_ => format!("{total:.1} {unit}"),
				};
				let err = err_pct(total, *value);
				Score { what: format!("{word} {}", item_label(item)), expected: format!("{value} {unit}"), got, err_pct: err, pass: n > 0 && err.is_some_and(|e| e.abs() <= *tol_pct) }
			}
			Expect::Scale { ft_per_in, preset, tol_pct } => {
				let want = ft_per_in.or_else(|| preset.as_deref().and_then(|l| ScalePreset::all().find(|p| p.label == l)).and_then(|p| Some(p.real / p.paper_in * length_factor(p.unit, "ft")?)));
				let (w, h) = doc.page_size_pt(page);
				let got = d.scale_for(page).and_then(|s| Some(scale_units_per_px(s, w, h)? * 72.0 * length_factor(&s.unit, "ft")?));
				let label = preset.clone().unwrap_or_else(|| format!("{} ft/in", want.unwrap_or(0.0)));
				match (want, got) {
					(Some(want), Some(got)) => {
						let err = err_pct(got, want);
						Score { what: "scale".into(), expected: label, got: format!("{got:.3} ft/in"), err_pct: err, pass: err.is_some_and(|e| e.abs() <= *tol_pct) }
					}
					(None, _) => Score { what: "scale".into(), expected: "?".into(), got: "bad expectation: give ft_per_in or a known preset".into(), err_pct: None, pass: false },
					(_, None) => Score { what: "scale".into(), expected: label, got: "no scale".into(), err_pct: None, pass: false },
				}
			}
			Expect::Answer { field, value } => {
				let pass = normalize(said).contains(&normalize(value));
				Score { what: format!("answer {field}"), expected: value.clone(), got: if pass { value.clone() } else { "not mentioned".into() }, err_pct: None, pass }
			}
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use takeoff_core::{Category, CategoryKind, Mark, Measurement, PageScale, Point};

	fn doc() -> Document {
		let mut data = TakeoffData::default();
		data.categories.push(Category::new("c1", "Duplex Receptacles", "#f00", CategoryKind::Count));
		data.categories.push(Category::new("c2", "Office", "#0f0", CategoryKind::Area));
		let mark = |id: &str, page: u32| -> Mark { serde_json::from_value(serde_json::json!({"id": id, "categoryId": "c1", "page": page, "x": 0.1, "y": 0.1, "w": 0.01, "h": 0.01})).unwrap() };
		for id in ["m1", "m2", "m3"] {
			data.marks.push(mark(id, 1));
		}
		data.marks.push(mark("m9", 2));
		// 1 ft = 72 pt on a 720×720 page: a 0.2×0.1 rectangle is 2 ft × 1 ft.
		data.page_scales.push(PageScale { page: 1, p1: Point { x: 0.0, y: 0.0 }, p2: Point { x: 0.1, y: 0.0 }, distance: 1.0, unit: "ft".into() });
		let mut m = Measurement::new("a1", 1, MeasurementType::Area, vec![Point { x: 0.0, y: 0.0 }, Point { x: 0.2, y: 0.0 }, Point { x: 0.2, y: 0.1 }, Point { x: 0.0, y: 0.1 }]);
		m.category_id = Some("c2".into());
		data.measurements.push(m);
		let l = Measurement::new("l1", 1, MeasurementType::Length, vec![Point { x: 0.0, y: 0.5 }, Point { x: 0.5, y: 0.5 }]);
		data.measurements.push(Measurement { label: Some("Wall run".into()), ..l });
		let mut d = Document::new(data, vec![(720.0, 720.0), (720.0, 720.0)], "t");
		d.recompute();
		d
	}

	#[test]
	fn scores_counts_quantities_scale_and_answers() {
		let d = doc();
		let ex: Vec<Expect> = serde_json::from_value(serde_json::json!([
			{"kind": "count", "item": "receptacle", "value": 3},
			{"kind": "count", "item": "receptacle", "value": 4},
			{"kind": "count", "item": "light", "value": 0},
			{"kind": "area", "item": "office", "value": 2, "unit": "sf"},
			{"kind": "area", "item": "office", "value": 288, "unit": "in2", "tol_pct": 1},
			{"kind": "length", "item": "wall", "value": 5.2, "unit": "ft"},
			{"kind": "length", "value": 60, "unit": "in", "tol_pct": 0.5},
			{"kind": "scale", "ft_per_in": 1.0},
			{"kind": "scale", "preset": "1/4\" = 1'-0\""},
			{"kind": "answer", "field": "sheet", "value": "A-101"}
		]))
		.unwrap();
		let s = score(&ex, &d, 1, "This is sheet a - 101, FIRST FLOOR.");
		let pass: Vec<bool> = s.iter().map(|s| s.pass).collect();
		assert_eq!(pass, [true, false, true, true, true, true, true, true, false, true], "{s:#?}");
		assert_eq!(s[1].err_pct.map(|e| e.round()), Some(-25.0));
		assert_eq!(s[3].got, "2.0 sf");
		assert!((s[5].err_pct.unwrap() + 3.846).abs() < 0.01);
		assert_eq!(s[8].got, "1.000 ft/in");
		// No scale: measurements are unscaled and fail.
		let mut d2 = doc();
		d2.data.page_scales.clear();
		d2.recompute();
		let s = score(&ex[3..4], &d2, 1, "");
		assert!(!s[0].pass && s[0].got == "1 unscaled", "{s:?}");
	}

	#[test]
	fn units_convert_within_a_kind() {
		assert_eq!(convert(1.0, "ft²", "in2").map(f64::round), Some(144.0));
		assert_eq!(convert(10.0, "ft", "m").map(|v| (v * 1000.0).round()), Some(3048.0));
		assert_eq!(convert(1.0, "ft", "sf"), None);
	}
}
