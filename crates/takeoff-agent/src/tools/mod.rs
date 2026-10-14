//! The drawing tools: what the agent (or any MCP client) can do to the ONE
//! drawing that is loaded. Each tool has its own JSON schema. Locations are
//! always handed in relative to a view id (the render they were read from),
//! so the model never converts coordinates itself; `view` is the only tool
//! that takes page fractions, and only to say where to look.

mod defs;
mod items;
mod pages;
mod render;
mod scale;
pub mod vision;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crossbeam_channel::Sender;
use parking_lot::Mutex;
use serde_json::Value;
use takeoff_core::coords::{dialect_for, region_to_page, Dialect, RawBox};
use takeoff_core::snap::SnapIndex;
use takeoff_core::{Document, Point};
use takeoff_pdf::{Drawing, FracRect};

use crate::{AgentEvent, ProviderConfig};

pub use defs::{definitions, openai_tools};

pub struct ToolDef {
	pub name: String,
	pub description: String,
	pub parameters: Value,
}

#[derive(Debug)]
pub struct ToolOutput {
	pub text: String,
	pub images: Vec<ViewImage>,
}

impl ToolOutput {
	fn text(t: impl Into<String>) -> Self {
		Self { text: t.into(), images: vec![] }
	}
}

#[derive(Debug, Clone)]
pub struct ViewImage {
	pub png: Vec<u8>,
	pub width: u32,
	pub height: u32,
	pub view_id: String,
	pub caption: String,
}

/// A registered render: what part of which page the model saw, at what size.
#[derive(Debug, Clone)]
pub struct View {
	pub id: String,
	pub page: u32,
	pub region: FracRect,
	pub seen_w: u32,
	pub seen_h: u32,
	pub dialect: Dialect,
}

pub struct ToolCtx {
	pub doc: Arc<Mutex<Document>>,
	pub drawing: Drawing,
	pub model_id: String,
	pub dialect: Dialect,
	/// Model used for vision sweeps (detect / calibrate / trace / analyze).
	pub llm: Option<ProviderConfig>,
	/// Live events (presence, doc changes) when run inside the agent loop.
	pub events: Option<Sender<AgentEvent>>,
	/// The page the user is looking at (default for page arguments).
	pub current_page: u32,
	views: Vec<View>,
	next_view: u32,
	snap: HashMap<u32, Arc<SnapIndex>>,
	/// Cancellation for long sweeps.
	pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}

const MAX_VIEWS: usize = 64;

impl ToolCtx {
	pub fn new(doc: Arc<Mutex<Document>>, drawing: Drawing, model_id: &str, llm: Option<ProviderConfig>) -> Self {
		Self {
			doc,
			drawing,
			model_id: model_id.to_string(),
			dialect: dialect_for(model_id),
			llm,
			events: None,
			current_page: 1,
			views: Vec::new(),
			next_view: 1,
			snap: HashMap::new(),
			cancel: None,
		}
	}

	fn emit(&self, e: AgentEvent) {
		if let Some(tx) = &self.events {
			let _ = tx.send(e);
		}
	}

	fn cancelled(&self) -> bool {
		self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
	}

	fn register_view(&mut self, page: u32, region: FracRect, w: u32, h: u32) -> View {
		let v = View { id: format!("v{}", self.next_view), page, region, seen_w: w, seen_h: h, dialect: self.dialect };
		self.next_view += 1;
		self.views.push(v.clone());
		if self.views.len() > MAX_VIEWS {
			self.views.remove(0);
		}
		v
	}

	fn view(&self, id: &str) -> Result<View, String> {
		let id = id.trim();
		let id = if id.starts_with('v') { id.to_string() } else { format!("v{id}") };
		self.views.iter().rev().find(|v| v.id == id).cloned().ok_or_else(|| {
			let known: Vec<&str> = self.views.iter().rev().take(6).map(|v| v.id.as_str()).collect();
			format!("unknown view '{id}' (recent views: {}). Render with `view` first and pass the id shown in its caption.", known.join(", "))
		})
	}

	fn page_count(&self) -> u32 {
		self.drawing.page_count()
	}

	fn page_or_current(&self, page: Option<u32>) -> Result<u32, String> {
		let p = page.unwrap_or(self.current_page).max(1);
		let n = self.page_count();
		if p > n {
			return Err(format!("page {p} does not exist (the drawing has {n} page{})", if n == 1 { "" } else { "s" }));
		}
		Ok(p)
	}

	/// The PDF's own line work on a page, indexed for snapping (cached).
	fn snap_index(&mut self, page: u32) -> Result<Arc<SnapIndex>, String> {
		if let Some(s) = self.snap.get(&page) {
			return Ok(s.clone());
		}
		let segs = self.drawing.segments(page).map_err(|e| e.to_string())?;
		let (w, h) = self.doc.lock().page_size_pt(page);
		let idx = Arc::new(SnapIndex::new(segs, w, h));
		self.snap.insert(page, idx.clone());
		Ok(idx)
	}

	/// Keep the renderer's layer state in step with the document's.
	fn sync_layers(&mut self, override_hidden: Option<&[String]>) -> Result<(), String> {
		let hidden: HashSet<String> = match override_hidden {
			Some(h) => h.iter().cloned().collect(),
			None => self.doc.lock().data.hidden_layers.iter().cloned().collect(),
		};
		self.drawing.set_hidden_layers(&hidden).map_err(|e| e.to_string())
	}
}

// ── Location conversion (view frame → page fractions) ─────────────────────

fn nums(v: &Value, n: usize, what: &str) -> Result<Vec<f64>, String> {
	let arr = v.as_array().ok_or_else(|| format!("{what} must be an array of {n} numbers"))?;
	let out: Vec<f64> = arr
		.iter()
		.filter_map(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
		.collect();
	if out.len() != n || out.iter().any(|x| !x.is_finite()) {
		return Err(format!("{what} must be an array of {n} numbers, got {v}"));
	}
	Ok(out)
}

/// A box as read off view `v`, in its dialect → page fractions (x, y, w, h).
pub(crate) fn view_box_to_page(v: &View, b: &Value) -> Result<(f64, f64, f64, f64), String> {
	let n = nums(b, 4, "box")?;
	let raw = if v.dialect == Dialect::Grid1000Yx {
		RawBox { x1: n[1], y1: n[0], x2: n[3], y2: n[2], label: None }
	} else {
		RawBox { x1: n[0], y1: n[1], x2: n[2], y2: n[3], label: None }
	};
	let raw = RawBox { x1: raw.x1.min(raw.x2), y1: raw.y1.min(raw.y2), x2: raw.x1.max(raw.x2), y2: raw.y1.max(raw.y2), label: None };
	if !v.dialect.box_in_bounds(raw.clone(), v.seen_w as f64, v.seen_h as f64) {
		let frame = if v.dialect.is_grid() { "0-1000".to_string() } else { format!("{}x{} px", v.seen_w, v.seen_h) };
		return Err(format!("box {b} is outside view {} ({frame}). Read locations off that view's image.", v.id));
	}
	let f = v.dialect.box_to_fractions(raw, v.seen_w as f64, v.seen_h as f64);
	let p = region_to_page((v.region.x, v.region.y, v.region.w, v.region.h), f);
	Ok((p.0.clamp(0.0, 1.0), p.1.clamp(0.0, 1.0), p.2.max(0.0005), p.3.max(0.0005)))
}

/// A point `[a, b]` as read off view `v` → page fraction point.
pub(crate) fn view_point_to_page(v: &View, p: &Value) -> Result<Point, String> {
	let (x, y) = v.dialect.pair_to_point(p).ok_or_else(|| format!("point must be {} — got {p}", v.dialect.pair_spec()))?;
	if !v.dialect.point_in_bounds(x, y, v.seen_w as f64, v.seen_h as f64) {
		let frame = if v.dialect.is_grid() { format!("its 0-1000 grid, pairs {}", v.dialect.pair_spec()) } else { format!("{}x{} px", v.seen_w, v.seen_h) };
		return Err(format!("point {p} is outside view {} ({frame}). Read both points off that view's image; if the span runs off it, zoom out and view again.", v.id));
	}
	let (fx, fy) = v.dialect.point_to_fractions(x, y, v.seen_w as f64, v.seen_h as f64);
	Ok(Point { x: (v.region.x + fx * v.region.w).clamp(0.0, 1.0), y: (v.region.y + fy * v.region.h).clamp(0.0, 1.0) })
}

/// Page fractions → this view's dialect units (to describe items back to the model).
pub(crate) fn page_box_to_view(v: &View, b: (f64, f64, f64, f64)) -> Option<[f64; 4]> {
	let fx = (b.0 - v.region.x) / v.region.w;
	let fy = (b.1 - v.region.y) / v.region.h;
	let fw = b.2 / v.region.w;
	let fh = b.3 / v.region.h;
	if fx + fw < 0.0 || fy + fh < 0.0 || fx > 1.0 || fy > 1.0 {
		return None;
	}
	let (sx, sy) = if v.dialect.is_grid() { (1000.0, 1000.0) } else { (v.seen_w as f64, v.seen_h as f64) };
	let (x1, y1, x2, y2) = ((fx * sx).round(), (fy * sy).round(), ((fx + fw) * sx).round(), ((fy + fh) * sy).round());
	Some(if v.dialect == Dialect::Grid1000Yx { [y1, x1, y2, x2] } else { [x1, y1, x2, y2] })
}

fn parse<T: serde::de::DeserializeOwned>(name: &str, args: Value) -> Result<T, String> {
	let args = if args.is_null() { Value::Object(Default::default()) } else { args };
	serde_json::from_value(args).map_err(|e| format!("bad arguments for {name}: {e}"))
}

/// Run one tool.
pub fn call(ctx: &mut ToolCtx, name: &str, args: Value) -> Result<ToolOutput, String> {
	// Models sometimes send the arguments as a JSON string.
	let args = match args {
		Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
		other => other,
	};
	match name {
		"get_drawing" => pages::get_drawing(ctx),
		"view" => render::view(ctx, parse(name, args)?),
		"read_items" => items::read_items(ctx, parse(name, args)?),
		"find_items_at" => items::find_items_at(ctx, parse(name, args)?),
		"add_marks" => items::add_marks(ctx, parse(name, args)?),
		"add_measurements" => items::add_measurements(ctx, parse(name, args)?),
		"update_items" => items::update_items(ctx, parse(name, args)?),
		"delete_items" => items::delete_items(ctx, parse(name, args)?),
		"create_category" => items::create_category(ctx, parse(name, args)?),
		"update_category" => items::update_category(ctx, parse(name, args)?),
		"summary" => items::summary(ctx, parse(name, args)?),
		"add_annotation" => items::add_annotation(ctx, parse(name, args)?),
		"set_scale" => scale::set_scale(ctx, parse(name, args)?),
		"set_scale_preset" => scale::set_scale_preset(ctx, parse(name, args)?),
		"check_scale" => scale::check_scale(ctx, parse(name, args)?),
		"calibrate" => vision::calibrate(ctx, parse(name, args)?),
		"detect" => vision::detect(ctx, parse(name, args)?),
		"trace" => vision::trace(ctx, parse(name, args)?),
		"read_text" => pages::read_text(ctx, parse(name, args)?),
		"search_text" => pages::search_text(ctx, parse(name, args)?),
		"set_hidden_layers" => pages::set_hidden_layers(ctx, parse(name, args)?),
		"follow_line" => scale::follow_line(ctx, parse(name, args)?),
		"set_viewport" => pages::set_viewport(ctx, parse(name, args)?),
		other => Err(format!("unknown tool '{other}'")),
	}
}
