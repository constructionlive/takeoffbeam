//! Vision pipelines (port of the platform's takeoffPipeline / takeoffGeometry):
//! - detect: tiled symbol sweep → pending marks, deduplicated
//! - calibrate: find a printed dimension, pin its two endpoints, set the scale,
//!   cross-check against a second dimension
//! - trace: tiled polylines / polygons → measurements
//!
//! Every call is an OpenAI-format image chat call to `ctx.llm`, so it runs on
//! llama-server or a cloud model alike. Coordinates go through the dialect.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

use image::RgbaImage;
use serde::Deserialize;
use serde_json::{json, Value};
use takeoff_core::coords::{extract_array, image_fit, safe_max_edge, Dialect};
use takeoff_core::geometry::{iou, point_distance_px};
use takeoff_core::*;
use takeoff_pdf::FracRect;

use super::items::{ai_meta, is_duplicate, resolve_category};
use super::render::{evidence, render_view, RegionArg, RenderSpec};
use super::{view_box_to_page, ToolCtx, ToolOutput};
use crate::{llm, AgentEvent, ProviderConfig};

const TILE_PX: f64 = 1000.0;
const TILE_STRIDE: f64 = 800.0;
const DETECT_DPI: f32 = 200.0;
const MAX_TILES: usize = 140;

fn png(img: &RgbaImage) -> Vec<u8> {
	let mut out = Vec::new();
	let _ = img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png);
	out
}

fn is_local(p: &ProviderConfig) -> bool {
	crate::is_local_endpoint(&p.base_url)
}

fn concurrency(p: &ProviderConfig) -> usize {
	if is_local(p) {
		2
	} else {
		6
	}
}

/// One image chat call: `text` then images. Returns the reply text.
fn ask(p: &ProviderConfig, text: &str, images: &[&[u8]]) -> Result<String, String> {
	let mut content = vec![json!({"type": "text", "text": text})];
	for img in images {
		content.push(llm::image_part(img, &p.model));
	}
	let msgs = vec![json!({"role": "user", "content": content})];
	llm::complete_with(p, &msgs, Some(4096), is_local(p)).map(|(t, _, _)| t)
}

/// Rendered tile of a page, with where it sits (fractions) and what the model sees.
struct Tile {
	region: FracRect,
	png: Vec<u8>,
	seen: (u32, u32),
}

fn tiles(ctx: &mut ToolCtx, page: u32, scope: FracRect, dpi: f32, tile_px: f64, stride_px: f64, max: usize) -> Result<Vec<Tile>, String> {
	let (w_pt, h_pt) = ctx.doc.lock().page_size_pt(page);
	let (pw, ph) = (w_pt * dpi as f64 / 72.0, h_pt * dpi as f64 / 72.0);
	let (x0, y0, x1, y1) = (scope.x * pw, scope.y * ph, (scope.x + scope.w) * pw, (scope.y + scope.h) * ph);
	let starts = |a: f64, b: f64| -> Vec<f64> {
		let mut v = vec![];
		let mut s = a;
		loop {
			v.push(s);
			if s + tile_px >= b {
				break;
			}
			s += stride_px;
		}
		v
	};
	let (xs, ys) = (starts(x0, x1), starts(y0, y1));
	if xs.len() * ys.len() > max {
		return Err(format!(
			"that scope needs {} tiles (max {max} per call). Narrow it: one region or page at a time.",
			xs.len() * ys.len()
		));
	}
	let mut out = Vec::new();
	for y in &ys {
		for x in &xs {
			let r = FracRect { x: x / pw, y: y / ph, w: (tile_px.min(x1 - x)) / pw, h: (tile_px.min(y1 - y)) / ph };
			let rendered = ctx.drawing.render(page, dpi, r, None).map_err(|e| e.to_string())?;
			let mut img = rendered.image;
			let seen = image_fit(&ctx.model_for_vision(), img.width(), img.height());
			if seen != img.dimensions() {
				img = image::imageops::resize(&img, seen.0, seen.1, image::imageops::FilterType::Triangle);
			}
			out.push(Tile { region: rendered.region, png: png(&img), seen });
		}
	}
	Ok(out)
}

impl ToolCtx {
	fn vision(&self) -> Result<ProviderConfig, String> {
		match &self.llm {
			Some(p) if p.vision => Ok(p.clone()),
			Some(_) => Err("the selected model has no vision, so it can't run this sweep — pick a vision model".into()),
			None => Err("no model is configured for vision sweeps".into()),
		}
	}
	fn model_for_vision(&self) -> String {
		self.llm.as_ref().map(|p| p.model.clone()).unwrap_or_else(|| self.model_id.clone())
	}
	fn vision_dialect(&self) -> Dialect {
		takeoff_core::coords::dialect_for(&self.model_for_vision())
	}
}

/// Run `f` over items with a few worker threads; results keep input order.
fn parallel<T: Sync, R: Send>(items: &[T], workers: usize, cancel: &(dyn Fn() -> bool + Sync), f: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
	let next = AtomicUsize::new(0);
	let out: StdMutex<Vec<Option<R>>> = StdMutex::new((0..items.len()).map(|_| None).collect());
	std::thread::scope(|s| {
		for _ in 0..workers.max(1).min(items.len().max(1)) {
			s.spawn(|| loop {
				let i = next.fetch_add(1, Ordering::Relaxed);
				if i >= items.len() || cancel() {
					break;
				}
				let r = f(&items[i]);
				out.lock().unwrap()[i] = Some(r);
			});
		}
	});
	out.into_inner().unwrap()
}

fn scope_of(ctx: &ToolCtx, page: Option<u32>, region: Option<&RegionArg>, view: Option<&String>, bbox: Option<&Value>) -> Result<(u32, FracRect), String> {
	if let Some(vid) = view {
		let v = ctx.view(vid)?;
		return Ok(match bbox {
			Some(b) => {
				let (x, y, w, h) = view_box_to_page(&v, b)?;
				(v.page, FracRect { x, y, w, h })
			}
			None => (v.page, v.region),
		});
	}
	let page = ctx.page_or_current(page)?;
	Ok((page, region.map(|r| FracRect { x: r.x, y: r.y, w: r.w, h: r.h }).unwrap_or(FracRect::FULL)))
}

// ── detect ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DetectTarget {
	pub category: String,
	pub hint: String,
}

#[derive(Deserialize)]
pub struct DetectArgs {
	/// Category id or name (created if new) — single-symbol sweep.
	pub category: Option<String>,
	/// What the symbol looks like, in words.
	pub symbol_hint: Option<String>,
	/// Up to 8 symbols in one sweep (each tile is read once for all).
	pub targets: Option<Vec<DetectTarget>>,
	/// The symbol's legend sample, read off a view.
	pub legend_view: Option<String>,
	pub legend_box: Option<Value>,
	pub page: Option<u32>,
	pub region: Option<RegionArg>,
	/// Scope = this view (optionally a box in it).
	pub view: Option<String>,
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	pub hidden_layers: Option<Vec<String>>,
}

const EMPTY_RULE: &str = "IMPORTANT: Many sections contain NO instances at all — for those, respond with exactly []. \
An empty array is a correct and expected answer; NEVER guess or invent boxes just to return something. \
Include a box ONLY when you can clearly and unambiguously see the symbol.";

struct Det {
	target: usize,
	b: (f64, f64, f64, f64),
	edge: bool,
	note: Option<String>,
}

pub fn detect(ctx: &mut ToolCtx, a: DetectArgs) -> Result<ToolOutput, String> {
	let p = ctx.vision()?;
	let (page, scope) = scope_of(ctx, a.page, a.region.as_ref(), a.view.as_ref(), a.bbox.as_ref())?;
	// Targets: one category with a hint/legend, or several described ones.
	let mut targets: Vec<(String, String)> = Vec::new();
	if let Some(ts) = &a.targets {
		if ts.len() > 8 {
			return Err("at most 8 targets per sweep".into());
		}
		for t in ts {
			targets.push((t.category.clone(), t.hint.clone()));
		}
	}
	if let Some(c) = &a.category {
		targets.push((c.clone(), a.symbol_hint.clone().unwrap_or_default()));
	}
	if targets.is_empty() {
		return Err("give category (+ symbol_hint or legend) or targets".into());
	}
	let mut cat_ids = Vec::new();
	for (name, hint) in &targets {
		cat_ids.push(resolve_category(ctx, name, CategoryKind::Count, true, Some(hint.as_str()).filter(|h| !h.is_empty()))?);
	}
	// Legend crop: explicit box, else the category's stored legend region.
	let legend_region = match (&a.legend_view, &a.legend_box) {
		(Some(v), Some(b)) => {
			let view = ctx.view(v)?;
			let (x, y, w, h) = view_box_to_page(&view, b)?;
			Some((view.page, FracRect { x, y, w, h }))
		}
		_ if targets.len() == 1 => ctx.doc.lock().data.category(&cat_ids[0]).and_then(|c| c.legend_region).map(|r| (r.page, FracRect { x: r.x, y: r.y, w: r.w, h: r.h })),
		_ => None,
	};
	if legend_region.is_none() && targets.iter().any(|t| t.1.trim().is_empty()) {
		return Err("describe the symbol (symbol_hint) or point at its legend sample (legend_view + legend_box)".into());
	}
	if legend_region.is_some() && targets.len() > 1 {
		return Err("a legend sample works for one target; run multi-target sweeps with text hints".into());
	}
	if legend_region.is_some() {
		let id = cat_ids[0].clone();
		let lr = legend_region.map(|(pg, r)| Region { page: pg, x: r.x, y: r.y, w: r.w, h: r.h });
		ctx.doc.lock().edit("AI: legend", |d| {
			if let Some(c) = d.categories.iter_mut().find(|c| c.id == id) {
				c.legend_region = lr;
			}
		});
	}
	ctx.sync_layers(a.hidden_layers.as_deref())?;
	let legend_png = match legend_region {
		Some((lp, r)) => {
			let pad_w = r.w * 0.15;
			let pad_h = r.h * 0.15;
			let rr = FracRect { x: r.x - pad_w, y: r.y - pad_h, w: r.w + 2.0 * pad_w, h: r.h + 2.0 * pad_h };
			let img = ctx.drawing.render(lp, DETECT_DPI, rr, Some(600)).map_err(|e| e.to_string())?.image;
			Some(png(&img))
		}
		None => None,
	};
	ctx.emit(AgentEvent::Viewport { page, region: (scope.x, scope.y, scope.w, scope.h), label: format!("Sweeping for {}", targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(", ")), drawing: None });
	let tiles = tiles(ctx, page, scope, DETECT_DPI, TILE_PX, TILE_STRIDE, MAX_TILES)?;
	if a.hidden_layers.is_some() {
		ctx.sync_layers(None)?;
	}
	let dialect = ctx.vision_dialect();
	let multi = targets.len() > 1;
	let prompt_for = |seen: (u32, u32)| -> String {
		let spec = format!(
			"Respond with ONLY a JSON array where each entry is a location plus a \"label\": \"<short note>\". {}",
			dialect.box_request(seen.0, seen.1)
		);
		if legend_png.is_some() {
			let hint = if targets[0].1.is_empty() { String::new() } else { format!(" ({})", targets[0].1) };
			format!("Image 1 shows a construction drawing symbol{hint}. Image 2 is a section of a construction drawing. Find EVERY instance of the image-1 symbol in image 2. {spec} Only report clear matches of this exact symbol. {EMPTY_RULE}")
		} else if multi {
			let list: Vec<String> = targets.iter().enumerate().map(|(i, t)| format!("{}. \"{}\": {}", i + 1, t.0, t.1)).collect();
			format!(
				"The image is a section of a construction drawing. Find EVERY instance of ANY of these items:\n{}\n{spec} Each entry's \"label\" MUST begin with the exact key of the item it is (e.g. \"{}\"), optionally followed by a short note. Only report clear matches. {EMPTY_RULE}",
				list.join("\n"),
				targets[0].0
			)
		} else {
			format!("The image is a section of a construction drawing. Find EVERY instance of this symbol/item: {}. {spec} Only report clear matches of the described symbol. {EMPTY_RULE}", targets[0].1)
		}
	};
	let cancel = ctx.cancel.clone();
	let is_cancelled = move || cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
	let replies = parallel(&tiles, concurrency(&p), &is_cancelled, |t| {
		let prompt = prompt_for(t.seen);
		let mut imgs: Vec<&[u8]> = Vec::new();
		if let Some(l) = &legend_png {
			imgs.push(l);
		}
		imgs.push(&t.png);
		ask(&p, &prompt, &imgs)
	});
	let mut dets: Vec<Det> = Vec::new();
	let (mut raw, mut failed) = (0usize, 0usize);
	let mut first_error = None;
	for (t, r) in tiles.iter().zip(replies) {
		let text = match r {
			Some(Ok(t)) => t,
			Some(Err(e)) => {
				failed += 1;
				first_error.get_or_insert(e);
				continue;
			}
			None => continue,
		};
		for bx in dialect.parse_boxes(&text) {
			if !dialect.box_in_bounds(bx.clone(), t.seen.0 as f64, t.seen.1 as f64) {
				continue;
			}
			raw += 1;
			let f = dialect.box_to_fractions(bx.clone(), t.seen.0 as f64, t.seen.1 as f64);
			if f.2 <= 0.0 || f.3 <= 0.0 || f.2 * f.3 > 0.35 {
				continue; // a "symbol" spanning most of the tile is a hallucination
			}
			let target = if multi {
				let l = bx.label.clone().unwrap_or_default().to_lowercase();
				match targets.iter().position(|tg| l.starts_with(&tg.0.to_lowercase())).or_else(|| targets.iter().position(|tg| l.contains(&tg.0.to_lowercase()))) {
					Some(i) => i,
					None => continue,
				}
			} else {
				0
			};
			let m = 0.004;
			let edge = f.0 <= m || f.1 <= m || f.0 + f.2 >= 1.0 - m || f.1 + f.3 >= 1.0 - m;
			let b = takeoff_core::coords::region_to_page((t.region.x, t.region.y, t.region.w, t.region.h), f);
			let note = bx.label.filter(|l| {
				let l = l.to_lowercase();
				["?", "maybe", "possibl", "unclear", "partial", "uncertain"].iter().any(|k| l.contains(k))
			});
			dets.push(Det { target, b, edge, note });
		}
	}
	if is_cancelled_now(ctx) {
		return Err("cancelled".into());
	}
	// Cross-tile dedup: tiles overlap by 200 px; keep interior boxes over edge-cut ones.
	dets.sort_by_key(|d| d.edge);
	let mut kept: Vec<Det> = Vec::new();
	for d in dets {
		let dup = kept.iter().any(|k| {
			k.target == d.target && (iou(k.b, d.b) > 0.3 || ((k.b.0 + k.b.2 / 2.0) - (d.b.0 + d.b.2 / 2.0)).hypot((k.b.1 + k.b.3 / 2.0) - (d.b.1 + d.b.3 / 2.0)) < 0.004)
		});
		if !dup {
			kept.push(d);
		}
	}
	let mut added: Vec<Vec<String>> = vec![Vec::new(); targets.len()];
	let mut skipped = 0;
	{
		let mut doc = ctx.doc.lock();
		let existing = doc.data.marks.clone();
		let mut fresh = Vec::new();
		for d in &kept {
			let cat = &cat_ids[d.target];
			if is_duplicate(&existing, page, cat, d.b) {
				skipped += 1;
				continue;
			}
			let id = new_id("m");
			added[d.target].push(id.clone());
			fresh.push(Mark {
				id,
				category_id: Some(cat.clone()),
				page,
				x: d.b.0,
				y: d.b.1,
				w: d.b.2,
				h: d.b.3,
				source: Source::Ai,
				status: MarkStatus::Pending,
				tile: None,
				note: d.note.clone(),
				fields: None,
				meta: ai_meta(),
			});
		}
		if !fresh.is_empty() {
			doc.edit(&format!("AI: detect {} mark(s)", fresh.len()), |dd| dd.marks.extend(fresh));
		}
	}
	ctx.emit(AgentEvent::DocChanged);
	let mut text = format!("Swept {} tile(s) on page {page}", tiles.len());
	if failed > 0 {
		text.push_str(&format!(" ({failed} failed: {})", first_error.unwrap_or_default()));
	}
	text.push_str(&format!(". {raw} raw find(s) → {} after dedup", kept.len()));
	if skipped > 0 {
		text.push_str(&format!(", {skipped} already marked"));
	}
	text.push_str(".\n");
	for (i, (name, _)) in targets.iter().enumerate() {
		text.push_str(&format!("  {name}: +{} pending mark(s)\n", added[i].len()));
	}
	text.push_str("A count you have not looked at is not a count: check the render below, zoom into dense areas, add what was missed and delete false finds (by location).");
	let mut out = ToolOutput::text(text);
	let filter: std::collections::HashSet<String> = cat_ids.iter().cloned().collect();
	let burn = takeoff_pdf::burn::BurnOptions { categories: Some(filter), ..Default::default() };
	if let Ok((img, _)) = render_view(ctx, RenderSpec { page, region: scope, burn, preferred_max: 2000, hidden_override: None }) {
		out.images.push(img);
	}
	Ok(out)
}

fn is_cancelled_now(ctx: &ToolCtx) -> bool {
	ctx.cancelled()
}

// ── calibrate ──────────────────────────────────────────────────────────────

/// A printed dimension, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedDimension {
	pub value: f64,
	pub unit: String,
	pub unit_known: bool,
}

fn round4(v: f64) -> f64 {
	(v * 10_000.0).round() / 10_000.0
}

/// "22'-6\"", "22' 6 1/2\"", "6\"", "3600 mm", "3.6m", or a bare number (unit guessed).
pub fn parse_dimension_text(raw: &str) -> Option<ParsedDimension> {
	let t: String = raw
		.replace(['‘', '’', '′'], "'")
		.replace(['“', '”', '″'], "\"")
		.replace("''", "\"")
		.trim()
		.to_string();
	let num_at = |s: &str| -> Option<(f64, usize)> {
		let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
		if end == 0 {
			return None;
		}
		s[..end].parse().ok().map(|v| (v, end))
	};
	// Feet with optional inches + fraction.
	if let Some(q) = t.find('\'') {
		let before = t[..q].trim_end();
		// Counted in bytes from the end: the char before the digits may be multibyte (Ø, ×).
		let start = before.len() - before.bytes().rev().take_while(|b| b.is_ascii_digit() || *b == b'.').count();
		let (mut value, _) = num_at(&before[start..])?;
		let rest = t[q + 1..].trim_start().trim_start_matches('-').trim_start();
		// inches: "6", "6 1/2", or "1/2" before a "
		if let Some(dq) = rest.find('"') {
			let inch_part = rest[..dq].trim();
			let mut inches = 0.0;
			let mut parts = inch_part.split_whitespace();
			if let Some(first) = parts.next() {
				if let Some((n, d)) = first.split_once('/') {
					inches += n.trim().parse::<f64>().ok()? / d.trim().parse::<f64>().ok()?;
				} else {
					inches += first.parse::<f64>().ok()?;
					if let Some(frac) = parts.next() {
						if let Some((n, d)) = frac.split_once('/') {
							inches += n.parse::<f64>().ok()? / d.parse::<f64>().ok()?;
						}
					}
				}
			}
			value += inches / 12.0;
		}
		return (value > 0.0).then(|| ParsedDimension { value: round4(value), unit: "ft".into(), unit_known: true });
	}
	if let Some(dq) = t.find('"') {
		let s = t[..dq].trim();
		let mut parts = s.split_whitespace().rev();
		let last = parts.next()?;
		let mut value = 0.0;
		if let Some((n, d)) = last.split_once('/') {
			value += n.parse::<f64>().ok()? / d.parse::<f64>().ok()?;
			if let Some(w) = parts.next() {
				value += w.parse::<f64>().unwrap_or(0.0);
			}
		} else {
			value = last.parse().ok()?;
		}
		return (value > 0.0).then(|| ParsedDimension { value: round4(value), unit: "in".into(), unit_known: true });
	}
	let lower = t.to_lowercase();
	for unit in ["mm", "cm", "m"] {
		if let Some(pos) = lower.find(unit) {
			let before = lower[..pos].trim_end();
			let start = before.len() - before.bytes().rev().take_while(|b| b.is_ascii_digit() || *b == b'.' || *b == b',').count();
			let after_ok = lower[pos + unit.len()..].chars().next().is_none_or(|c| !c.is_alphabetic());
			if start < before.len() && after_ok {
				let v: f64 = before[start..].replace(',', ".").parse().ok()?;
				return (v > 0.0).then(|| ParsedDimension { value: round4(v), unit: unit.into(), unit_known: true });
			}
		}
	}
	// Bare number (allow two non-digit chars around, like the platform).
	let digits: String = t.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
	let others = t.chars().filter(|c| !(c.is_ascii_digit() || *c == '.' || *c == ',')).count();
	if !digits.is_empty() && others <= 4 && t.len() <= 12 {
		let v: f64 = digits.replace(',', ".").parse().ok()?;
		if v <= 0.0 {
			return None;
		}
		return Some(ParsedDimension { value: round4(v), unit: if v >= 1000.0 { "mm".into() } else { "ft".into() }, unit_known: false });
	}
	None
}

/// Text that is nothing but a dimension (22'-6", 6015, 3.6 m, 4 1/2"),
/// not a callout that contains one ("20' AB @ 305 o/c", "SHEET 3").
pub fn is_pure_dimension(text: &str) -> bool {
	let t = text.trim();
	if t.is_empty() || t.len() > 16 || !t.chars().any(|c| c.is_ascii_digit()) {
		return false;
	}
	let lower = t.to_lowercase();
	let stripped = lower.trim_end_matches("mm").trim_end_matches("cm").trim_end_matches('m').trim();
	stripped.chars().all(|c| c.is_ascii_digit() || " '\"-/.,’”′″".contains(c))
}

fn comparable(d: &ParsedDimension) -> Option<(u8, f64)> {
	if !d.unit_known {
		return Some((2, d.value));
	}
	match d.unit.as_str() {
		"ft" => Some((0, d.value)),
		"in" => Some((0, d.value / 12.0)),
		"m" => Some((1, d.value)),
		"cm" => Some((1, d.value / 100.0)),
		"mm" => Some((1, d.value / 1000.0)),
		_ => None,
	}
}

#[derive(Deserialize)]
pub struct CalibrateArgs {
	pub page: Option<u32>,
	/// Replace an existing scale (default false: refuse).
	pub replace: Option<bool>,
	pub hidden_layers: Option<Vec<String>>,
}

struct DimCandidate {
	text: String,
	parsed: ParsedDimension,
	/// Text box centre + size, page fractions.
	cx: f64,
	cy: f64,
	w: f64,
	h: f64,
}

struct Resolved {
	c: DimCandidate,
	p1: Point,
	p2: Point,
	span_pt: f64,
}

pub fn calibrate(ctx: &mut ToolCtx, a: CalibrateArgs) -> Result<ToolOutput, String> {
	let p = ctx.vision()?;
	let page = ctx.page_or_current(a.page)?;
	if ctx.doc.lock().data.scale_for(page).is_some() && !a.replace.unwrap_or(false) {
		return Err(format!("page {page} already has a scale; pass replace: true to recalibrate"));
	}
	ctx.sync_layers(a.hidden_layers.as_deref())?;
	let (w_pt, h_pt) = ctx.doc.lock().page_size_pt(page);
	ctx.emit(AgentEvent::Viewport { page, region: (0.0, 0.0, 1.0, 1.0), label: "Reading dimensions".into(), drawing: None });
	let dialect = ctx.vision_dialect();
	// Stage A: dimension texts. The PDF text layer first (free and exact),
	// then a vision tile sweep when the sheet has no usable text.
	let mut cands: Vec<DimCandidate> = Vec::new();
	if let Ok(runs) = ctx.drawing.text_runs(page, None, 20_000) {
		for r in runs {
			if !is_pure_dimension(&r.text) || r.rect.2 <= 0.0 {
				continue;
			}
			if let Some(pd) = parse_dimension_text(&r.text) {
				cands.push(DimCandidate { text: r.text.clone(), parsed: pd, cx: r.rect.0 + r.rect.2 / 2.0, cy: r.rect.1 + r.rect.3 / 2.0, w: r.rect.2, h: r.rect.3 });
			}
		}
	}
	let known = cands.iter().filter(|c| c.parsed.unit_known).count();
	if known >= 2 {
		cands.retain(|c| c.parsed.unit_known);
	} else {
		// Unit-less numbers: keep plausible dimension values (grid / sheet numbers are small).
		cands.retain(|c| c.parsed.unit_known || c.parsed.value >= 100.0);
	}
	let mut source = "text layer";
	if cands.len() < 2 {
		source = "vision sweep";
		let ts = tiles(ctx, page, FracRect::FULL, 150.0, TILE_PX, TILE_PX, 60)?;
		let prompt = |seen: (u32, u32)| format!(
			"The image is a section of a construction drawing. Find every DIMENSION TEXT in it: a measurement number written on or next to a dimension line or dimensional callout (like 22'-6\" or 3600 or 24 — the number may be unit-less). Respond with ONLY a JSON array, one entry per dimension text, each a location plus \"text\": \"<the text exactly as written>\". {} The box tightly encloses JUST THE TEXT. Skip radius/diameter/angle callouts, elevation tags, sheet/grid/room numbers, and text cut off by the section edge. Many sections contain none — respond with exactly [] for those; never invent entries.",
			dialect.box_request(seen.0, seen.1)
		);
		let cancel = ctx.cancel.clone();
		let replies = parallel(&ts, concurrency(&p), &move || cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)), |t| ask(&p, &prompt(t.seen), &[&t.png]));
		for (t, r) in ts.iter().zip(replies) {
			let Some(Ok(text)) = r else { continue };
			for b in dialect.parse_boxes(&text) {
				let Some(label) = b.label.clone() else { continue };
				if !is_pure_dimension(&label) {
					continue;
				}
				let Some(pd) = parse_dimension_text(&label) else { continue };
				let f = dialect.box_to_fractions(b, t.seen.0 as f64, t.seen.1 as f64);
				let g = takeoff_core::coords::region_to_page((t.region.x, t.region.y, t.region.w, t.region.h), f);
				cands.push(DimCandidate { text: label, parsed: pd, cx: g.0 + g.2 / 2.0, cy: g.1 + g.3 / 2.0, w: g.2, h: g.3 });
			}
			if cands.len() >= 12 {
				break;
			}
		}
	}
	if cands.is_empty() {
		ctx.sync_layers(None)?;
		return Err(format!("No readable dimension annotations found on page {page}. Set the scale by hand: view a dimension, then set_scale with its two arrowhead points and value."));
	}
	// Prefer longer dimensions (better precision), spread over the sheet.
	cands.sort_by(|x, y| {
		let vx = comparable(&x.parsed).map(|c| c.1).unwrap_or(0.0);
		let vy = comparable(&y.parsed).map(|c| c.1).unwrap_or(0.0);
		vy.total_cmp(&vx)
	});
	let mut picked: Vec<DimCandidate> = Vec::new();
	for c in cands {
		if picked.len() >= 4 {
			break;
		}
		if picked.iter().any(|p| (p.cx - c.cx).hypot(p.cy - c.cy) < 0.05) {
			continue;
		}
		picked.push(c);
	}
	// Stage B: pin each candidate's two endpoints on a zoom window around it;
	// candidates not resolved on the tight window get one wider look.
	let max_edge = safe_max_edge(&p.model, 1400);
	ctx.sync_layers(a.hidden_layers.as_deref())?;
	let prompt_b = |text: &str, seen: (u32, u32)| -> String {
		let scale = if dialect.is_grid() { "on the 0-1000 scale of THIS image".to_string() } else { format!("in absolute pixel coordinates of THIS {}x{}-pixel image (origin top-left, x right, y down)", seen.0, seen.1) };
		format!(
			"Near the center of this construction-drawing crop is a linear dimension whose text reads approximately \"{text}\". A dimension is drawn |<---->| : two thin EXTENSION LINES project out from the structure, and the dimension line runs BETWEEN them, terminated by an arrowhead (or tick slash) AT each extension line; the text sits above or beside the line and its width is NOT the span. Locate the two ENDPOINTS of the span this number measures — the exact points where the arrowheads/ticks touch the two extension lines (for a leader callout, the two feature points it labels). The endpoints are NEVER the ends of the text, and the true span is usually much wider than the text. Respond with ONLY JSON: {{\"p1\": {pair}, \"p2\": {pair}, \"text\": \"<the dimension text exactly as written>\"}} with coordinates {scale}. If both endpoints are NOT fully visible in this image, respond with null.",
			pair = dialect.pair_spec()
		)
	};
	let snap = ctx.snap_index(page).ok();
	let mut resolved: Vec<Resolved> = Vec::new();
	let mut done = vec![false; picked.len()];
	for mult in [1.0, 2.5] {
		let todo: Vec<usize> = (0..picked.len()).filter(|i| !done[*i]).collect();
		if todo.is_empty() {
			break;
		}
		let mut windows: Vec<(usize, FracRect, Vec<u8>, (u32, u32))> = Vec::new();
		for i in todo {
			let c = &picked[i];
			// Window side in page fractions: wide enough for the whole dimension.
			let side = (c.w.max(c.h * w_pt / h_pt) * 10.0 * mult).clamp(0.06, 0.6);
			let (fw, fh) = (side, (side * w_pt / h_pt).min(1.0));
			let r = FracRect { x: c.cx - fw / 2.0, y: c.cy - fh / 2.0, w: fw, h: fh };
			let rendered = ctx.drawing.render(page, 400.0, r, Some(max_edge)).map_err(|e| e.to_string())?;
			let mut img = rendered.image;
			let seen = image_fit(&p.model, img.width(), img.height());
			if seen != img.dimensions() {
				img = image::imageops::resize(&img, seen.0, seen.1, image::imageops::FilterType::Triangle);
			}
			windows.push((i, rendered.region, png(&img), seen));
		}
		let cancel = ctx.cancel.clone();
		let replies = parallel(&windows, concurrency(&p), &move || cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)), |(i, _, img, seen)| ask(&p, &prompt_b(&picked[*i].text, *seen), &[img]));
		for ((i, region, _, seen), r) in windows.iter().zip(replies) {
			let Some(Ok(text)) = r else { continue };
			let Some(obj) = first_object(&text) else { continue };
			let (Some(a1), Some(a2)) = (obj.get("p1").and_then(|v| dialect.pair_to_point(v)), obj.get("p2").and_then(|v| dialect.pair_to_point(v))) else { continue };
			let to_page = |(x, y): (f64, f64)| {
				let (fx, fy) = dialect.point_to_fractions(x, y, seen.0 as f64, seen.1 as f64);
				Point { x: region.x + fx * region.w, y: region.y + fy * region.h }
			};
			let (mut p1, mut p2) = (to_page(a1), to_page(a2));
			if let Some(idx) = snap.as_ref().filter(|i| !i.is_empty()) {
				let (pts, _) = takeoff_core::snap::fit_points(idx, &[p1, p2], 5.0);
				p1 = pts[0];
				p2 = pts[1];
			}
			let c = &picked[*i];
			let span_pt = point_distance_px(p1, p2, w_pt, h_pt);
			let text_pt = c.w.max(c.h) * w_pt;
			if span_pt < text_pt * 1.1 || span_pt < 12.0 {
				continue; // the model gave the text ends, not the span
			}
			done[*i] = true;
			resolved.push(Resolved { c: DimCandidate { text: c.text.clone(), parsed: c.parsed.clone(), cx: c.cx, cy: c.cy, w: c.w, h: c.h }, p1, p2, span_pt });
		}
	}
	ctx.sync_layers(None)?;
	if resolved.is_empty() {
		return Err(format!(
			"Found {} dimension text(s) on page {page} ({source}) but couldn't pin down their endpoints. View a dimension and set_scale by hand.",
			picked.len()
		));
	}
	// Pick the dimension most others agree with (units per paper point).
	let upp = |r: &Resolved| comparable(&r.c.parsed).map(|(fam, v)| (fam, v / r.span_pt));
	let mut best = 0;
	let mut best_votes = 0;
	for (i, r) in resolved.iter().enumerate() {
		let Some((fam, u)) = upp(r) else { continue };
		let votes = resolved.iter().filter(|o| upp(o).is_some_and(|(f2, u2)| f2 == fam && ((u2 - u) / u).abs() <= 0.03)).count();
		if votes > best_votes {
			best = i;
			best_votes = votes;
		}
	}
	let chosen = &resolved[best];
	let check = resolved.iter().enumerate().find(|(i, o)| *i != best && upp(o).map(|x| x.0) == upp(chosen).map(|x| x.0)).map(|(_, o)| {
		let (u1, u2) = (upp(chosen).unwrap().1, upp(o).unwrap().1);
		(o.c.text.clone(), ((u2 - u1) / u1).abs() * 100.0)
	});
	let scale = PageScale { page, p1: chosen.p1, p2: chosen.p2, distance: chosen.c.parsed.value, unit: chosen.c.parsed.unit.clone() };
	ctx.doc.lock().edit("AI: calibrate", |d| {
		d.page_scales.retain(|s| s.page != page);
		d.page_scales.push(scale);
	});
	ctx.emit(AgentEvent::DocChanged);
	let mut text = format!(
		"Calibrated page {page} from \"{}\" = {} {} ({source}; {} candidate(s), {} resolved).",
		chosen.c.text,
		chosen.c.parsed.value,
		chosen.c.parsed.unit,
		picked.len(),
		resolved.len()
	);
	match &check {
		Some((t, e)) if *e <= 3.0 => text.push_str(&format!(" Cross-check against \"{t}\": {e:.1}% — agrees.")),
		Some((t, e)) => text.push_str(&format!(" Cross-check against \"{t}\": {e:.1}% — DOES NOT AGREE; verify with check_scale before measuring.")),
		None => text.push_str(" No second dimension to cross-check: verify with check_scale on another printed dimension."),
	}
	if !chosen.c.parsed.unit_known {
		text.push_str(&format!(" The dimension had no unit; assumed {} — correct it if wrong.", chosen.c.parsed.unit));
	}
	let mut out = ToolOutput::text(text);
	let probe = Measurement::new(new_id("probe"), page, MeasurementType::Length, vec![chosen.p1, chosen.p2]);
	let pid = probe.id.clone();
	ctx.doc.lock().data.measurements.push(probe);
	let img = evidence(ctx, page, std::slice::from_ref(&pid), "calibrate");
	ctx.doc.lock().data.measurements.retain(|m| m.id != pid);
	if let Some(img) = img {
		out.images.push(img);
	}
	Ok(out)
}

fn first_object(raw: &str) -> Option<serde_json::Map<String, Value>> {
	let (s, e) = (raw.find('{')?, raw.rfind('}')?);
	if e <= s {
		return None;
	}
	serde_json::from_str::<Value>(&raw[s..=e]).ok()?.as_object().cloned()
}

// ── trace ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TraceArgs {
	/// length | area
	pub kind: String,
	/// What to trace, in words ("curb line", "slab edge", "landscape area").
	pub target: String,
	pub category: Option<String>,
	pub page: Option<u32>,
	pub region: Option<RegionArg>,
	pub view: Option<String>,
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	pub hidden_layers: Option<Vec<String>>,
}

pub fn trace(ctx: &mut ToolCtx, a: TraceArgs) -> Result<ToolOutput, String> {
	let p = ctx.vision()?;
	let area = match a.kind.to_lowercase().as_str() {
		"area" | "polygon" => true,
		"length" | "linear" | "polyline" => false,
		o => return Err(format!("kind must be length or area, got {o}")),
	};
	let (page, scope) = scope_of(ctx, a.page, a.region.as_ref(), a.view.as_ref(), a.bbox.as_ref())?;
	ctx.sync_layers(a.hidden_layers.as_deref())?;
	ctx.emit(AgentEvent::Viewport { page, region: (scope.x, scope.y, scope.w, scope.h), label: format!("Tracing {}", a.target), drawing: None });
	let ts = tiles(ctx, page, scope, 150.0, 1600.0, 800.0, 24)?;
	if a.hidden_layers.is_some() {
		ctx.sync_layers(None)?;
	}
	let dialect = ctx.vision_dialect();
	let prompt = |seen: (u32, u32)| {
		let shape = if area {
			"For each one, give the ordered boundary vertices of its polygon, walking the outline once (do NOT repeat the first point)."
		} else {
			"For each one, give the ordered vertices of a polyline that follows it end to end."
		};
		let scale = if dialect.is_grid() { "on the 0-1000 scale of THIS image".to_string() } else { format!("in absolute pixel coordinates of THIS {}x{}-pixel image (origin top-left, x right, y down)", seen.0, seen.1) };
		format!(
			"The image is a section of a construction drawing. Find EVERY element matching: {}. {shape} Place vertices exactly on the corners — use as few vertices as capture each shape (a straight run is just 2 points). Respond with ONLY JSON: {{\"shapes\": [{{\"points\": [{}, ...]}}, ...]}} with all coordinates {scale}. Include a shape ONLY when you clearly see the element; if none match, respond {{\"shapes\": []}}. Never invent shapes.",
			a.target,
			dialect.pair_spec()
		)
	};
	let cancel = ctx.cancel.clone();
	let replies = parallel(&ts, concurrency(&p), &move || cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)), |t| ask(&p, &prompt(t.seen), &[&t.png]));
	let mut shapes: Vec<Vec<Point>> = Vec::new();
	for (t, r) in ts.iter().zip(replies) {
		let Some(Ok(text)) = r else { continue };
		let list: Vec<Value> = match first_object(&text) {
			Some(o) => o.get("shapes").and_then(Value::as_array).cloned().or_else(|| o.get("points").map(|p| vec![json!({"points": p})])).unwrap_or_default(),
			None => extract_array(&text),
		};
		for s in list {
			let pts_raw = s.get("points").cloned().unwrap_or(s);
			let Some(arr) = pts_raw.as_array() else { continue };
			let mut pts = Vec::new();
			for p in arr {
				if let Some((x, y)) = dialect.pair_to_point(p) {
					if dialect.point_in_bounds(x, y, t.seen.0 as f64, t.seen.1 as f64) {
						let (fx, fy) = dialect.point_to_fractions(x, y, t.seen.0 as f64, t.seen.1 as f64);
						pts.push(Point { x: (t.region.x + fx * t.region.w).clamp(0.0, 1.0), y: (t.region.y + fy * t.region.h).clamp(0.0, 1.0) });
					}
				}
			}
			pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4);
			if pts.len() >= if area { 3 } else { 2 } && pts.len() <= 100 {
				shapes.push(pts);
			}
		}
	}
	// Tiles overlap 50%: the same element is often traced twice.
	shapes.sort_by_key(|s| std::cmp::Reverse(s.len()));
	let mut kept: Vec<Vec<Point>> = Vec::new();
	for s in shapes {
		let b = takeoff_core::geometry::bounds(&s).unwrap();
		let b = (b.0, b.1, b.2.max(0.002), b.3.max(0.002));
		if kept.iter().any(|k| {
			let kb = takeoff_core::geometry::bounds(k).unwrap();
			iou((kb.0, kb.1, kb.2.max(0.002), kb.3.max(0.002)), b) > 0.6
		}) {
			continue;
		}
		kept.push(s);
	}
	let cat = match &a.category {
		Some(c) => Some(resolve_category(ctx, c, if area { CategoryKind::Area } else { CategoryKind::Length }, true, Some(&a.target))?),
		None => None,
	};
	let mut ids = Vec::new();
	{
		let mut doc = ctx.doc.lock();
		let mut new = Vec::new();
		for s in kept {
			let mut m = Measurement::new(new_id("ms"), page, if area { MeasurementType::Area } else { MeasurementType::Length }, s);
			m.category_id = cat.clone();
			m.source = Some(Source::Ai);
			m.meta = ai_meta();
			ids.push(m.id.clone());
			new.push(m);
		}
		if !new.is_empty() {
			doc.edit(&format!("AI: trace {} shape(s)", new.len()), |d| d.measurements.extend(new));
		}
	}
	ctx.emit(AgentEvent::DocChanged);
	let total: f64 = {
		let doc = ctx.doc.lock();
		doc.data.measurements.iter().filter(|m| ids.contains(&m.id)).filter_map(|m| m.value).sum()
	};
	let unit = ctx.doc.lock().data.scale_for(page).map(|s| if area { format!("{}²", s.unit) } else { s.unit.clone() });
	let mut out = ToolOutput::text(format!(
		"Traced {} shape(s) over {} tile(s){}. Check the render: traced vertices are approximate — adjust or snap them before relying on the total.",
		ids.len(),
		ts.len(),
		unit.map(|u| format!(", total {total:.2} {u}")).unwrap_or_else(|| " (page has no scale — no values yet)".into())
	));
	if !ids.is_empty() {
		if let Some(img) = evidence(ctx, page, &ids, "trace") {
			out.images.push(img);
		}
	}
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dimension_texts() {
		let p = |s: &str| parse_dimension_text(s).map(|d| (d.value, d.unit, d.unit_known));
		assert_eq!(p("22'-6\""), Some((22.5, "ft".into(), true)));
		assert_eq!(p("22' 6 1/2\""), Some((22.5417, "ft".into(), true)));
		assert_eq!(p("3'"), Some((3.0, "ft".into(), true)));
		assert_eq!(p("6\""), Some((6.0, "in".into(), true)));
		assert_eq!(p("4 1/2\""), Some((4.5, "in".into(), true)));
		assert_eq!(p("3600 mm"), Some((3600.0, "mm".into(), true)));
		assert_eq!(p("3.6m"), Some((3.6, "m".into(), true)));
		assert_eq!(p("6015"), Some((6015.0, "mm".into(), false)));
		assert_eq!(p("24"), Some((24.0, "ft".into(), false)));
		assert_eq!(p("SITE PLAN"), None);
		assert_eq!(p("12’-0”"), Some((12.0, "ft".into(), true)));
		// A multibyte character right before the number (page text is anything).
		assert_eq!(p("Ø12'-0\""), Some((12.0, "ft".into(), true)));
		assert_eq!(p("Ø100mm"), Some((100.0, "mm".into(), true)));
		assert_eq!(p("2×3.6 m"), Some((3.6, "m".into(), true)));
		assert!(is_pure_dimension("22'-6\""));
		assert!(is_pure_dimension("6015"));
		assert!(is_pure_dimension("3.6 m"));
		assert!(is_pure_dimension("4 1/2\""));
		assert!(!is_pure_dimension("20' AB @ 305 o/c"));
		assert!(!is_pure_dimension("SHEET 3"));
	}
}
