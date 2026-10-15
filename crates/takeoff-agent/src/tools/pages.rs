//! Drawing-level tools: the overview, text, layers, presence.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};
use takeoff_pdf::FracRect;

use super::render::RegionArg;
use super::{view_box_to_page, ToolCtx, ToolOutput};
use crate::AgentEvent;

pub fn get_drawing(ctx: &mut ToolCtx) -> Result<ToolOutput, String> {
	let n = ctx.page_count();
	let file = ctx.drawing.path().file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
	let doc = ctx.doc.lock();
	let d = &doc.data;
	let mut pages = Vec::new();
	for p in 1..=n.min(400) {
		let (w, h) = doc.page_size_pt(p);
		let label = ctx.drawing.page_label(p).ok().flatten().filter(|l| *l != p.to_string());
		let marks = d.marks.iter().filter(|m| m.page == p).count();
		let meas = d.measurements.iter().filter(|m| m.page == p).count();
		let scale = d.scale_for(p).map(|s| format!("{} {} ref", trim(s.distance), s.unit));
		pages.push(json!({"page": p, "label": label, "size_in": format!("{:.1}x{:.1}", w / 72.0, h / 72.0), "scale": scale, "marks": marks, "measurements": meas}));
	}
	let cats: Vec<Value> = d
		.categories
		.iter()
		.map(|c| {
			let count = d.marks.iter().filter(|m| m.category_id.as_deref() == Some(&c.id)).count();
			let meas = d.measurements.iter().filter(|m| m.category_id.as_deref() == Some(&c.id)).count();
			json!({"id": c.id, "name": c.name, "kind": format!("{:?}", c.kind()).to_lowercase(), "color": c.color, "description": c.description, "marks": count, "measurements": meas})
		})
		.collect();
	let layers = ctx.drawing.layer_names();
	let open: Vec<Value> = ctx.drawing_names().into_iter().enumerate().map(|(i, (name, active))| json!({"number": i + 1, "file": name, "active": active})).collect();
	let mut info = json!({
		"file": file,
		"pages": n,
		"current_page": ctx.current_page,
		"page_list": pages,
		"categories": cats,
		"layers": layers,
		"hidden_layers": d.hidden_layers,
		"annotations": d.annotations.len(),
		"viewports": d.viewports.iter().map(|v| json!({"page": v.page, "name": v.name})).collect::<Vec<_>>(),
		"overlays": d.overlays.iter().map(|o| json!({
			"id": o.id, "page": o.page, "label": o.label, "opacity": o.opacity, "hidden": o.hidden,
			"kind": if o.svg.is_some() { "svg" } else { "image" },
			"region": {"x": o.x, "y": o.y, "w": o.w, "h": o.h}
		})).collect::<Vec<_>>(),
	});
	if open.len() > 1 {
		info["open_drawings"] = Value::Array(open);
	}
	let mut text = serde_json::to_string(&info).unwrap_or_default();
	if n > 400 {
		text.push_str(&format!("\n(page list truncated to 400 of {n})"));
	}
	Ok(ToolOutput::text(text))
}

fn trim(v: f64) -> String {
	let s = format!("{v:.3}");
	s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Deserialize)]
pub struct ReadTextArgs {
	pub page: Option<u32>,
	pub region: Option<RegionArg>,
	pub view: Option<String>,
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	pub limit: Option<usize>,
}

pub fn read_text(ctx: &mut ToolCtx, a: ReadTextArgs) -> Result<ToolOutput, String> {
	let (page, region) = match (&a.view, &a.bbox) {
		(Some(v), Some(b)) => {
			let view = ctx.view(v)?;
			let (x, y, w, h) = view_box_to_page(&view, b)?;
			(view.page, Some(FracRect { x, y, w, h }))
		}
		(Some(v), None) => {
			let view = ctx.view(v)?;
			(view.page, Some(view.region))
		}
		_ => (ctx.page_or_current(a.page)?, a.region.map(|r| FracRect { x: r.x, y: r.y, w: r.w, h: r.h })),
	};
	let runs = ctx.drawing.text_runs(page, region, a.limit.unwrap_or(400).min(2000)).map_err(|e| e.to_string())?;
	if runs.is_empty() {
		return Ok(ToolOutput::text(format!(
			"No extractable text on page {page}{}. Many CAD sheets draw text as line strokes (SHX fonts): read it from a zoomed view instead.",
			if region.is_some() { " in that region" } else { "" }
		)));
	}
	let mut out = format!("{} text run(s) on page {page} (positions are page fractions x,y of the run's top-left):\n", runs.len());
	for r in &runs {
		out.push_str(&format!("[{:.3},{:.3}] {}\n", r.rect.0, r.rect.1, r.text));
	}
	Ok(ToolOutput::text(out))
}

#[derive(Deserialize)]
pub struct SearchArgs {
	pub query: String,
	/// e.g. "1-5, 9" (default: all pages)
	pub pages: Option<String>,
	pub match_case: Option<bool>,
	pub whole_word: Option<bool>,
	pub limit: Option<usize>,
}

pub fn search_text(ctx: &mut ToolCtx, a: SearchArgs) -> Result<ToolOutput, String> {
	let n = ctx.page_count();
	let pages = match &a.pages {
		Some(spec) => takeoff_core::pages::parse_page_ranges(spec, n)?,
		None => (1..=n).collect(),
	};
	let limit = a.limit.unwrap_or(60).min(500);
	let mut hits = Vec::new();
	for p in pages {
		let left = limit.saturating_sub(hits.len());
		if left == 0 {
			break;
		}
		hits.extend(ctx.drawing.search_page(p, &a.query, a.match_case.unwrap_or(false), a.whole_word.unwrap_or(false), left).map_err(|e| e.to_string())?);
	}
	if hits.is_empty() {
		return Ok(ToolOutput::text(format!("'{}' not found in the text layer (text drawn as strokes can't be searched).", a.query)));
	}
	let mut out = format!("{} hit(s) for '{}':\n", hits.len(), a.query);
	for h in &hits {
		out.push_str(&format!(
			"page {} at x={:.3} y={:.3} w={:.3} h={:.3}: {}\n",
			h.page, h.rect.0, h.rect.1, h.rect.2, h.rect.3, h.context
		));
	}
	Ok(ToolOutput::text(out))
}

#[derive(Deserialize)]
pub struct HiddenLayersArgs {
	/// The complete list of layer names to hide ([] = show all).
	pub hidden: Vec<String>,
}

pub fn set_hidden_layers(ctx: &mut ToolCtx, a: HiddenLayersArgs) -> Result<ToolOutput, String> {
	let known: HashSet<String> = ctx.drawing.layer_names().into_iter().collect();
	if known.is_empty() {
		return Err("this PDF has no layers".into());
	}
	let unknown: Vec<&String> = a.hidden.iter().filter(|l| !known.contains(*l)).collect();
	if !unknown.is_empty() {
		return Err(format!("unknown layer(s): {unknown:?}. Layers: {:?}", ctx.drawing.layer_names()));
	}
	let hidden = a.hidden.clone();
	ctx.doc.lock().edit("AI: layers", |d| d.hidden_layers = hidden);
	ctx.sync_layers(None)?;
	ctx.emit(AgentEvent::DocChanged);
	Ok(ToolOutput::text(format!(
		"Hidden layers set to {:?}. This is shared: the user sees the same layers.",
		a.hidden
	)))
}

#[derive(Deserialize)]
pub struct ViewportArgs {
	pub view: Option<String>,
	pub page: Option<u32>,
	pub region: Option<RegionArg>,
	pub label: Option<String>,
}

pub fn set_viewport(ctx: &mut ToolCtx, a: ViewportArgs) -> Result<ToolOutput, String> {
	let (page, r) = match &a.view {
		Some(v) => {
			let v = ctx.view(v)?;
			(v.page, v.region)
		}
		None => (ctx.page_or_current(a.page)?, a.region.map(|r| FracRect { x: r.x, y: r.y, w: r.w, h: r.h }).unwrap_or(FracRect::FULL)),
	};
	ctx.emit(AgentEvent::Viewport { page, region: (r.x, r.y, r.w, r.h), label: a.label.unwrap_or_else(|| "Working here".into()), drawing: None });
	Ok(ToolOutput::text("Shown to the user."))
}
