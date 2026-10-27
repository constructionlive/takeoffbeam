//! Reading and writing takeoff items: marks (counts), measurements (length,
//! area, angle), annotations, categories, and the summary.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};
use takeoff_core::geometry::{bounds, iou, point_segment_distance_px, scale_units_per_px};
use takeoff_core::numbering::{mark_seq, measurement_seq};
use takeoff_core::*;

use super::render::evidence;
use super::{view_box_to_page, view_point_to_page, ToolCtx, ToolOutput};
use crate::AgentEvent;

pub(crate) fn ai_meta() -> ItemMeta {
	let now = now_ms();
	ItemMeta { author: Some("AI".into()), created_at: Some(now), modified_at: Some(now), ..Default::default() }
}

/// Find a category by id or name; create it (next colour) when `create` is set.
pub(crate) fn resolve_category(ctx: &ToolCtx, key: &str, kind: CategoryKind, create: bool, description: Option<&str>) -> Result<String, String> {
	let mut doc = ctx.doc.lock();
	if let Some(c) = doc.data.categories.iter().find(|c| c.id == key || c.name.eq_ignore_ascii_case(key.trim())) {
		return Ok(c.id.clone());
	}
	if !create {
		let names: Vec<String> = doc.data.categories.iter().map(|c| format!("{} ({})", c.name, c.id)).collect();
		return Err(format!("no category '{key}'. Existing: {}", if names.is_empty() { "none".into() } else { names.join(", ") }));
	}
	let color = doc.data.next_category_color().to_string();
	let id = new_id("c");
	let mut c = Category::new(id.clone(), key.trim(), color, kind);
	c.description = description.map(str::to_string);
	doc.edit(&format!("AI: new item {}", key.trim()), |d| d.categories.push(c));
	Ok(id)
}

/// A space by id or name (any case).
fn resolve_space(d: &TakeoffData, key: &str) -> Result<String, String> {
	if let Some(s) = d.spaces.iter().find(|s| s.id == key || s.name.eq_ignore_ascii_case(key.trim())) {
		return Ok(s.id.clone());
	}
	let names: Vec<String> = d.spaces.iter().map(|s| format!("{} ({}, p{})", s.name, s.id, s.page)).collect();
	Err(format!("no space '{key}'. Existing: {}", if names.is_empty() { "none".into() } else { names.join(", ") }))
}

fn changed(ctx: &ToolCtx) {
	ctx.emit(AgentEvent::DocChanged);
}

// ── read_items / find_items_at ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReadItemsArgs {
	pub page: Option<u32>,
	pub category: Option<String>,
	/// mark | measurement | annotation
	pub kind: Option<String>,
	/// Space id or name: only items in it (or in spaces inside it).
	pub space: Option<String>,
	pub offset: Option<usize>,
}

pub fn read_items(ctx: &mut ToolCtx, a: ReadItemsArgs) -> Result<ToolOutput, String> {
	let doc = ctx.doc.lock();
	let d = &doc.data;
	let cat = a.category.as_deref().and_then(|k| d.categories.iter().find(|c| c.id == k || c.name.eq_ignore_ascii_case(k)).map(|c| c.id.clone()));
	if let (Some(k), None) = (&a.category, &cat) {
		return Err(format!("no category '{k}'"));
	}
	let cat_ok = |c: &Option<String>| cat.as_ref().is_none_or(|w| c.as_ref() == Some(w));
	let page_ok = |p: u32| a.page.is_none_or(|w| w == p);
	let kind = a.kind.as_deref().unwrap_or("");
	let mseq = mark_seq(&d.marks);
	let sseq = measurement_seq(&d.measurements);
	let cname = |c: &Option<String>| c.as_deref().and_then(|id| d.category(id)).map(|c| c.name.as_str()).unwrap_or("ungrouped").to_string();
	let spaces = takeoff_core::spaces::Spaces::new(d);
	let within = a.space.as_deref().map(|k| resolve_space(d, k)).transpose()?;
	let space_ok = |s: Option<&Space>| within.as_deref().is_none_or(|w| spaces.is_within(s.map(|s| s.id.as_str()), w));
	let path = |s: Option<&Space>| s.map(|s| spaces.path_name(&s.id));
	let mut rows: Vec<Value> = Vec::new();
	if kind.is_empty() || kind.starts_with("mark") {
		for m in d.marks.iter().filter(|m| page_ok(m.page) && cat_ok(&m.category_id)) {
			let s = spaces.of_mark(m);
			if !space_ok(s) {
				continue;
			}
			rows.push(json!({"id": m.id, "kind": "mark", "badge": mseq.get(&m.id), "category": cname(&m.category_id), "page": m.page, "space": path(s),
				"at": [r3(m.x), r3(m.y), r3(m.w), r3(m.h)], "status": format!("{:?}", m.status).to_lowercase(), "source": format!("{:?}", m.source).to_lowercase(), "note": m.note}));
		}
	}
	if kind.is_empty() || kind.starts_with("measure") {
		for m in d.measurements.iter().filter(|m| page_ok(m.page) && cat_ok(&m.category_id)) {
			let s = spaces.of_measurement(m);
			if !space_ok(s) {
				continue;
			}
			rows.push(json!({"id": m.id, "kind": format!("{:?}", m.kind).to_lowercase(), "badge": sseq.get(&m.id), "category": cname(&m.category_id), "page": m.page, "space": path(s),
				"points": m.points.len(), "value": m.value.map(r3), "unit": m.unit, "label": m.label, "note": m.note}));
		}
	}
	if kind.is_empty() || kind.starts_with("annot") {
		for an in d.annotations.iter().filter(|x| page_ok(x.page)) {
			let s = spaces.of_annotation(an);
			if !space_ok(s) {
				continue;
			}
			rows.push(json!({"id": an.id, "kind": an.kind.label(), "page": an.page, "space": path(s), "text": an.text}));
		}
	}
	let total = rows.len();
	let off = a.offset.unwrap_or(0).min(total);
	let page_rows: Vec<Value> = rows.into_iter().skip(off).take(200).collect();
	let more = if off + page_rows.len() < total { format!(" — {} more, call again with offset {}", total - off - page_rows.len(), off + page_rows.len()) } else { String::new() };
	Ok(ToolOutput::text(format!(
		"{total} item(s){more}. Positions are page fractions (x, y, w, h) for reference only; to locate or change an item use a view.\n{}",
		serde_json::to_string(&page_rows).unwrap_or_default()
	)))
}

fn r3(v: f64) -> f64 {
	(v * 1000.0).round() / 1000.0
}

#[derive(Deserialize)]
pub struct FindArgs {
	pub view: String,
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	pub point: Option<Value>,
}

pub fn find_items_at(ctx: &mut ToolCtx, a: FindArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	let target = match (&a.bbox, &a.point) {
		(Some(b), _) => view_box_to_page(&v, b)?,
		(None, Some(p)) => {
			let p = view_point_to_page(&v, p)?;
			(p.x - 0.002, p.y - 0.002, 0.004, 0.004)
		}
		_ => return Err("give box or point (read off the view)".into()),
	};
	let doc = ctx.doc.lock();
	let (pw, ph) = doc.page_size_pt(v.page);
	let d = &doc.data;
	let mseq = mark_seq(&d.marks);
	let centre = (target.0 + target.2 / 2.0, target.1 + target.3 / 2.0);
	let dist = |b: (f64, f64, f64, f64)| ((b.0 + b.2 / 2.0 - centre.0) * pw).hypot((b.1 + b.3 / 2.0 - centre.1) * ph);
	let mut hits: Vec<(f64, Value)> = Vec::new();
	for m in d.marks.iter().filter(|m| m.page == v.page) {
		let b = (m.x, m.y, m.w, m.h);
		let o = iou(b, target);
		let dd = dist(b);
		if o > 0.0 || dd < 40.0 {
			let name = m.category_id.as_deref().and_then(|c| d.category(c)).map(|c| c.name.clone());
			hits.push((if o > 0.0 { -o } else { dd }, json!({"id": m.id, "kind": "mark", "category": name, "badge": mseq.get(&m.id), "overlap": r3(o)})));
		}
	}
	for m in d.measurements.iter().filter(|m| m.page == v.page) {
		let p = takeoff_core::Point { x: centre.0, y: centre.1 };
		let close = m.points.windows(2).map(|w| point_segment_distance_px(p, w[0], w[1], pw, ph)).fold(f64::INFINITY, f64::min);
		let inside = m.kind == MeasurementType::Area && takeoff_core::geometry::point_in_polygon(p, &m.points);
		if close < 40.0 || inside {
			hits.push((if inside { 0.0 } else { close }, json!({"id": m.id, "kind": format!("{:?}", m.kind).to_lowercase(), "distance_pt": r3(close)})));
		}
	}
	for an in d.annotations.iter().filter(|x| x.page == v.page) {
		if let Some(b) = bounds(&an.points) {
			let o = iou(b, target);
			if o > 0.0 || dist(b) < 40.0 {
				hits.push((if o > 0.0 { -o } else { dist(b) }, json!({"id": an.id, "kind": an.kind.label()})));
			}
		}
	}
	hits.sort_by(|a, b| a.0.total_cmp(&b.0));
	if hits.is_empty() {
		return Ok(ToolOutput::text("No items at that location."));
	}
	let list: Vec<Value> = hits.into_iter().take(12).map(|h| h.1).collect();
	Ok(ToolOutput::text(format!("Items at that location, best match first:\n{}", serde_json::to_string(&list).unwrap_or_default())))
}

// ── add_marks ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddMarksArgs {
	pub view: String,
	pub boxes: Vec<Value>,
	/// Category id or name (created if new).
	pub category: String,
	pub description: Option<String>,
	pub note: Option<String>,
}

/// Same-category duplicate: IoU > 0.5 or centres within 0.004 of the page.
pub(crate) fn is_duplicate(existing: &[Mark], page: u32, cat: &str, b: (f64, f64, f64, f64)) -> bool {
	existing.iter().any(|m| {
		m.page == page
			&& m.category_id.as_deref() == Some(cat)
			&& (iou((m.x, m.y, m.w, m.h), b) > 0.5
				|| ((m.x + m.w / 2.0) - (b.0 + b.2 / 2.0)).hypot((m.y + m.h / 2.0) - (b.1 + b.3 / 2.0)) < 0.004)
	})
}

pub fn add_marks(ctx: &mut ToolCtx, a: AddMarksArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	if a.boxes.is_empty() {
		return Err("boxes is empty".into());
	}
	let mut boxes = Vec::new();
	for b in &a.boxes {
		// Accept [..4] or {"box": [..4]}.
		let b = b.get("box").or_else(|| b.get("box_2d")).or_else(|| b.get("bbox_2d")).unwrap_or(b);
		boxes.push(view_box_to_page(&v, b)?);
	}
	let cat = resolve_category(ctx, &a.category, CategoryKind::Count, true, a.description.as_deref())?;
	let added: Vec<String>;
	let mut skipped = 0;
	{
		let mut doc = ctx.doc.lock();
		let existing = doc.data.marks.clone();
		let mut fresh: Vec<Mark> = Vec::new();
		for b in boxes {
			if is_duplicate(&existing, v.page, &cat, b) || is_duplicate(&fresh, v.page, &cat, b) {
				skipped += 1;
				continue;
			}
			fresh.push(Mark {
				id: new_id("m"),
				category_id: Some(cat.clone()),
				page: v.page,
				x: b.0,
				y: b.1,
				w: b.2,
				h: b.3,
				source: Source::Ai,
				status: MarkStatus::Pending,
				tile: None,
				note: a.note.clone(),
				fields: None,
				meta: ai_meta(),
			});
		}
		added = fresh.iter().map(|m| m.id.clone()).collect();
		if !fresh.is_empty() {
			doc.edit(&format!("AI: add {} mark(s)", fresh.len()), |d| d.marks.extend(fresh));
		}
	}
	changed(ctx);
	let count = ctx.doc.lock().data.marks.iter().filter(|m| m.category_id.as_deref() == Some(&cat) && m.page == v.page).count();
	let mut out = ToolOutput::text(format!(
		"Added {} pending mark(s){} — {} now has {count} on page {}. Check the render below; fix misplaced marks by location (find_items_at), not by badge.",
		added.len(),
		if skipped > 0 { format!(", skipped {skipped} duplicate(s) of existing marks") } else { String::new() },
		a.category,
		v.page
	));
	if let Some(img) = evidence(ctx, v.page, &added, "add_marks") {
		out.images.push(img);
	}
	Ok(out)
}

// ── add_measurements ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MeasurementArg {
	/// length | area | angle | perimeter
	#[serde(rename = "type")]
	pub kind: String,
	pub points: Vec<Value>,
	pub label: Option<String>,
	pub note: Option<String>,
	pub depth: Option<f64>,
	pub closed: Option<bool>,
	/// straight (default) | smooth | follow — how to join the points.
	pub path: Option<String>,
}

/// Join the points the way asked: straight; `smooth`, a curve through them;
/// `follow`, along the drawing's own line between each pair (dashes and
/// crossings included). Returns the points to store and a word on what happened.
pub(super) fn shape_path(ctx: &mut ToolCtx, page: u32, pts: Vec<Point>, closed: bool, mode: Option<&str>) -> Result<(Vec<Point>, String), String> {
	let mode = mode.map(|m| m.trim().to_lowercase()).unwrap_or_default();
	let (w, h) = ctx.doc.lock().page_size_pt(page);
	match mode.as_str() {
		"" | "straight" | "lines" => Ok((pts, String::new())),
		"smooth" | "curve" | "spline" | "arc" => {
			let out = takeoff_core::geometry::smooth_path(&pts, closed, w, h);
			Ok((out, format!(" Smoothed into a curve through the {} points.", pts.len())))
		}
		"follow" | "trace" | "along" => {
			// The drawing's own vector lines first; where there are none (a scan,
			// a flattened sheet, lines over a photo), the line in the picture.
			let idx = ctx.snap_index(page)?;
			let mut ends: Vec<(Point, Point)> = pts.windows(2).map(|p| (p[0], p[1])).collect();
			if closed && pts.len() > 2 {
				ends.push((pts[pts.len() - 1], pts[0]));
			}
			let mut out: Vec<Point> = Vec::new();
			let (mut vector, mut picture) = (0, 0);
			let mut missed = Vec::new();
			for (k, (a, b)) in ends.iter().enumerate() {
				let run = match (!idx.is_empty()).then(|| idx.track(*a, *b)).flatten() {
					Some(r) => {
						vector += 1;
						r
					}
					None => match ctx.drawing.track_raster(page, *a, *b).ok().flatten() {
						Some(r) => {
							picture += 1;
							r
						}
						None => {
							missed.push(format!("{}→{}", k + 1, (k + 1) % pts.len() + 1));
							vec![*a, *b]
						}
					},
				};
				let skip = usize::from(!out.is_empty());
				out.extend(run.into_iter().skip(skip));
			}
			if closed && out.len() > 2 && out.first() == out.last() {
				out.pop();
			}
			let mut note = format!(" Followed the drawn line on {} of {} span(s)", vector + picture, ends.len());
			if picture > 0 {
				note.push_str(&format!(" ({picture} in the picture, as there are no vector lines there)"));
			}
			note.push_str(&format!(", {} vertices.", out.len()));
			if !missed.is_empty() {
				note.push_str(&format!(" No single drawn line joins points {}: those spans are straight. Put both points ON one drawn line (not between two, e.g. a track centreline that isn't drawn), and add a point where it bends sharply or forks; or use path \"smooth\".", missed.join(", ")));
			}
			Ok((out, note))
		}
		other => Err(format!("unknown path '{other}' (straight, smooth, follow)")),
	}
}

#[derive(Deserialize)]
pub struct AddMeasurementsArgs {
	pub view: String,
	pub items: Vec<MeasurementArg>,
	pub category: Option<String>,
	/// Fit vertices onto the drawing's own line work (vector PDFs).
	pub snap: Option<bool>,
}

pub fn add_measurements(ctx: &mut ToolCtx, a: AddMeasurementsArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	let mut shapes = Vec::new();
	for it in &a.items {
		let (kind, closed) = match it.kind.to_lowercase().as_str() {
			"length" | "linear" | "polyline" => (MeasurementType::Length, it.closed.unwrap_or(false)),
			"perimeter" => (MeasurementType::Length, true),
			"area" | "polygon" => (MeasurementType::Area, false),
			"angle" => (MeasurementType::Angle, false),
			other => return Err(format!("unknown measurement type '{other}' (length, perimeter, area, angle)")),
		};
		let pts: Result<Vec<Point>, String> = it.points.iter().map(|p| view_point_to_page(&v, p)).collect();
		let pts = pts?;
		let min = match kind {
			MeasurementType::Length => 2,
			_ => 3,
		};
		if pts.len() < min {
			return Err(format!("{} needs at least {min} points", it.kind));
		}
		if kind == MeasurementType::Angle && pts.len() != 3 {
			return Err("angle needs exactly 3 points: arm end, vertex, arm end".into());
		}
		shapes.push((kind, closed, pts, it));
	}
	let mut moved_note = String::new();
	if a.snap.unwrap_or(false) {
		match ctx.snap_index(v.page) {
			Ok(idx) if !idx.is_empty() => {
				let mut moves = Vec::new();
				for s in shapes.iter_mut() {
					let (pts, moved) = takeoff_core::snap::fit_points(&idx, &s.2, 6.0);
					s.2 = pts;
					moves.extend(moved);
				}
				let snapped = moves.iter().filter(|m| **m > 0.0).count();
				let max = moves.iter().cloned().fold(0.0, f64::max);
				moved_note = format!(" Snapped {snapped}/{} vertices to drawing lines (max move {max:.1} pt).", moves.len());
			}
			_ => moved_note = " No drawing lines here (scanned sheet?) — vertices kept as given.".into(),
		}
	}
	// Curves: follow the drawn line, or smooth through the points.
	let mut path_notes = String::new();
	for s in shapes.iter_mut() {
		if s.0 == MeasurementType::Angle || s.3.path.is_none() {
			continue;
		}
		let closed = s.1 || s.0 == MeasurementType::Area;
		let (pts, note) = shape_path(ctx, v.page, std::mem::take(&mut s.2), closed, s.3.path.as_deref())?;
		s.2 = pts;
		path_notes.push_str(&note);
	}
	let moved_note = moved_note + &path_notes;
	let default_kind = shapes.first().map(|s| if s.0 == MeasurementType::Area { CategoryKind::Area } else { CategoryKind::Length }).unwrap_or(CategoryKind::Length);
	let cat = match &a.category {
		Some(c) => Some(resolve_category(ctx, c, default_kind, true, None)?),
		None => None,
	};
	let mut ids = Vec::new();
	let mut lines = Vec::new();
	{
		let mut doc = ctx.doc.lock();
		let mut new = Vec::new();
		for (kind, closed, pts, it) in shapes {
			let mut m = Measurement::new(new_id("ms"), v.page, kind, pts);
			m.closed = closed;
			m.category_id = cat.clone();
			m.label = it.label.clone();
			m.note = it.note.clone();
			m.depth = it.depth;
			m.source = Some(Source::Ai);
			m.meta = ai_meta();
			ids.push(m.id.clone());
			new.push(m);
		}
		doc.edit(&format!("AI: add {} measurement(s)", new.len()), |d| d.measurements.extend(new));
		for id in &ids {
			if let Some(m) = doc.data.measurements.iter().find(|m| &m.id == id) {
				lines.push(match (m.value, &m.unit) {
					(Some(val), Some(u)) => format!("{id}: {val:.2} {u}"),
					_ => format!("{id}: no value (page {} has no scale yet)", v.page),
				});
			}
		}
	}
	changed(ctx);
	let mut out = ToolOutput::text(format!("Added {} measurement(s).{moved_note}\n{}", ids.len(), lines.join("\n")));
	if let Some(img) = evidence(ctx, v.page, &ids, "add_measurements") {
		out.images.push(img);
	}
	Ok(out)
}

// ── fill_area ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FillItem {
	/// One point inside the space, or several that fill together as one.
	pub points: Vec<Value>,
	/// area (default) | perimeter
	#[serde(rename = "type")]
	pub kind: Option<String>,
	/// Widest opening to close, in the page's scale unit; default: found.
	pub gap: Option<f64>,
	pub label: Option<String>,
	pub note: Option<String>,
	pub depth: Option<f64>,
}

#[derive(Deserialize)]
pub struct FillArgs {
	pub view: String,
	pub items: Vec<FillItem>,
	pub category: Option<String>,
}

/// Measure spaces by a point inside each: the outline the drawn lines make
/// around it, doorways and other openings closed.
pub fn fill_area(ctx: &mut ToolCtx, a: FillArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	if a.items.is_empty() || a.items.len() > 30 {
		return Err("give 1-30 items".into());
	}
	let (w, h) = ctx.doc.lock().page_size_pt(v.page);
	let mut shapes = Vec::new();
	let mut lines = Vec::new();
	for (k, it) in a.items.iter().enumerate() {
		let perimeter = match it.kind.as_deref().map(str::to_lowercase).as_deref() {
			None | Some("area") => false,
			Some("perimeter") => true,
			Some(other) => return Err(format!("unknown type '{other}' (area, perimeter)")),
		};
		let pts: Vec<Point> = it.points.iter().map(|p| view_point_to_page(&v, p)).collect::<Result<_, _>>()?;
		if pts.is_empty() {
			return Err(format!("item {}: give a point inside the space", k + 1));
		}
		// Openings to try: real door widths at the page's scale; or the one asked for.
		let pt_per_unit = {
			let doc = ctx.doc.lock();
			doc.data.scale_at(v.page, pts[0]).and_then(|s| scale_units_per_px(s, w, h).map(|u| (1.0 / u, s.unit.clone())))
		};
		let gaps = match (it.gap, &pt_per_unit) {
			(Some(g), Some((p, _))) => vec![g * p],
			(Some(_), None) => return Err("gap is in the scale unit, and this page has no scale yet: leave it out".into()),
			(None, Some((p, unit))) => takeoff_pdf::fill::gaps(takeoff_core::units::length_factor("m", unit).map(|f| f * p)),
			(None, None) => takeoff_pdf::fill::gaps(None),
		};
		match ctx.drawing.fill_area(v.page, &pts, &gaps).map_err(|e| e.to_string())? {
			Ok(f) => {
				let how = if f.gap_pt > 0.0 { format!(", openings up to {:.0} pt of paper closed", f.gap_pt) } else { String::new() };
				shapes.push((k, perimeter, f.points, it, how));
			}
			Err(takeoff_pdf::fill::NoFill::OnLine) => lines.push(format!("item {}: that point is on the line work, not in a space; put it in the open paper inside", k + 1)),
			Err(takeoff_pdf::fill::NoFill::Open) => lines.push(format!("item {}: no closed outline around that point (the lines leave it open to the page); outline it with add_measurements, or give gap", k + 1)),
		}
	}
	let cat = match (&a.category, shapes.is_empty()) {
		(Some(c), false) => Some(resolve_category(ctx, c, if shapes.iter().all(|s| s.1) { CategoryKind::Length } else { CategoryKind::Area }, true, None)?),
		_ => None,
	};
	let mut ids = Vec::new();
	{
		let mut doc = ctx.doc.lock();
		let mut new = Vec::new();
		let mut hows = Vec::new();
		for (k, perimeter, pts, it, how) in shapes {
			let n = pts.len();
			let mut m = Measurement::new(new_id("ms"), v.page, if perimeter { MeasurementType::Length } else { MeasurementType::Area }, pts);
			m.closed = perimeter;
			m.category_id = cat.clone();
			m.label = it.label.clone();
			m.note = it.note.clone();
			m.depth = it.depth;
			m.source = Some(Source::Ai);
			m.meta = ai_meta();
			ids.push(m.id.clone());
			hows.push((k, n, how));
			new.push(m);
		}
		if !new.is_empty() {
			doc.edit(&format!("AI: fill {} area(s)", new.len()), |d| d.measurements.extend(new));
		}
		for (id, (k, n, how)) in ids.iter().zip(hows) {
			if let Some(m) = doc.data.measurements.iter().find(|m| &m.id == id) {
				let value = match (m.value, &m.unit) {
					(Some(val), Some(u)) => format!("{val:.2} {u}"),
					_ => format!("no value (page {} has no scale yet)", v.page),
				};
				lines.push(format!("item {}: {id} = {value} ({n} vertices{how})", k + 1));
			}
		}
	}
	lines.sort();
	if !ids.is_empty() {
		changed(ctx);
	}
	let mut out = ToolOutput::text(format!("Filled {} of {} space(s). Check the render: a fill that leaked through an opening shows far bigger than the space; delete it and give gap, or outline it.\n{}", ids.len(), a.items.len(), lines.join("\n")));
	if let Some(img) = evidence(ctx, v.page, &ids, "fill_area") {
		out.images.push(img);
	}
	Ok(out)
}

// ── update / delete ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ItemPatch {
	pub id: String,
	/// View the new box/points were read from.
	pub view: Option<String>,
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	pub points: Option<Vec<Value>>,
	pub category: Option<String>,
	/// pending | confirmed
	pub status: Option<String>,
	pub note: Option<String>,
	pub label: Option<String>,
	pub text: Option<String>,
	pub depth: Option<f64>,
	/// With points: straight | smooth | follow (as in add_measurements).
	pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateItemsArgs {
	pub items: Vec<ItemPatch>,
}

pub fn update_items(ctx: &mut ToolCtx, a: UpdateItemsArgs) -> Result<ToolOutput, String> {
	if a.items.len() > 50 {
		return Err("at most 50 items per call".into());
	}
	// Resolve all geometry first so a bad item changes nothing.
	struct Resolved {
		id: String,
		bx: Option<(f64, f64, f64, f64)>,
		pts: Option<Vec<Point>>,
		cat: Option<Option<String>>,
	}
	let mut res = Vec::new();
	let mut page = None;
	for p in &a.items {
		let item = ctx.doc.lock().data.find(&p.id).ok_or_else(|| format!("no item with id {}", p.id))?;
		page = page.or(ctx.doc.lock().data.item_page(&item));
		let view = match &p.view {
			Some(vid) => Some(ctx.view(vid)?),
			None => None,
		};
		let bx = match (&p.bbox, &view) {
			(Some(b), Some(v)) => Some(view_box_to_page(v, b)?),
			(Some(_), None) => return Err(format!("{}: a new box needs the view it was read from", p.id)),
			_ => None,
		};
		let mut pts = match (&p.points, &view) {
			(Some(ps), Some(v)) => Some(ps.iter().map(|x| view_point_to_page(v, x)).collect::<Result<Vec<_>, _>>()?),
			(Some(_), None) => return Err(format!("{}: new points need the view they were read from", p.id)),
			_ => None,
		};
		if let (Some(mode), Some(v)) = (p.path.as_deref(), &view) {
			if let Some(ps) = pts.take() {
				let closed = ctx.doc.lock().data.measurements.iter().find(|m| m.id == p.id).map(|m| m.closed || m.kind == MeasurementType::Area).unwrap_or(false);
				pts = Some(shape_path(ctx, v.page, ps, closed, Some(mode))?.0);
			}
		}
		let cat = match &p.category {
			Some(c) if c.is_empty() || c == "none" => Some(None),
			Some(c) => Some(Some(resolve_category(ctx, c, CategoryKind::Count, false, None)?)),
			None => None,
		};
		res.push(Resolved { id: p.id.clone(), bx, pts, cat });
	}
	let now = now_ms();
	ctx.doc.lock().edit(&format!("AI: update {} item(s)", a.items.len()), |d| {
		for (p, r) in a.items.iter().zip(&res) {
			if let Some(m) = d.marks.iter_mut().find(|m| m.id == r.id) {
				if let Some(b) = r.bx {
					(m.x, m.y, m.w, m.h) = b;
				}
				if let Some(c) = &r.cat {
					m.category_id = c.clone();
				}
				match p.status.as_deref() {
					Some("confirmed") => m.status = MarkStatus::Confirmed,
					Some("pending") => m.status = MarkStatus::Pending,
					_ => {}
				}
				if p.note.is_some() {
					m.note = p.note.clone();
				}
				m.meta.modified_at = Some(now);
			} else if let Some(m) = d.measurements.iter_mut().find(|m| m.id == r.id) {
				if let Some(pts) = &r.pts {
					m.points = pts.clone();
				}
				if let Some(c) = &r.cat {
					m.category_id = c.clone();
				}
				if p.note.is_some() {
					m.note = p.note.clone();
				}
				if p.label.is_some() {
					m.label = p.label.clone();
				}
				if p.depth.is_some() {
					m.depth = p.depth;
				}
				m.meta.modified_at = Some(now);
			} else if let Some(an) = d.annotations.iter_mut().find(|x| x.id == r.id) {
				if let Some(pts) = &r.pts {
					an.points = pts.clone();
				} else if let Some(b) = r.bx {
					an.points = vec![Point { x: b.0, y: b.1 }, Point { x: b.0 + b.2, y: b.1 + b.3 }];
				}
				if p.text.is_some() {
					an.text = p.text.clone();
				}
				if p.note.is_some() {
					an.note = p.note.clone();
				}
				an.meta.modified_at = Some(now);
			}
		}
	});
	changed(ctx);
	let ids: Vec<String> = res.iter().map(|r| r.id.clone()).collect();
	let mut out = ToolOutput::text(format!("Updated {} item(s).", ids.len()));
	if let Some(p) = page {
		if let Some(img) = evidence(ctx, p, &ids, "update_items") {
			out.images.push(img);
		}
	}
	Ok(out)
}

#[derive(Deserialize)]
pub struct DeleteArgs {
	pub ids: Vec<String>,
}

pub fn delete_items(ctx: &mut ToolCtx, a: DeleteArgs) -> Result<ToolOutput, String> {
	let set: HashSet<String> = a.ids.iter().cloned().collect();
	let (removed, missing, locked) = {
		let mut doc = ctx.doc.lock();
		// Once each: a model that names a missing id twice must not count it twice.
		let mut seen = HashSet::new();
		let missing: Vec<String> = a.ids.iter().filter(|id| doc.data.find(id).is_none() && seen.insert(id.as_str())).cloned().collect();
		let n = doc.edit(&format!("AI: delete {} item(s)", set.len()), |d| d.delete_ids(&set));
		let locked = set.len().saturating_sub(missing.len() + n);
		(n, missing, locked)
	};
	changed(ctx);
	let mut t = format!("Deleted {removed} item(s).");
	if !missing.is_empty() {
		t.push_str(&format!(" Not found: {}.", missing.join(", ")));
	}
	if locked > 0 {
		t.push_str(&format!(" {locked} locked by the user and kept."));
	}
	Ok(ToolOutput::text(t))
}

// ── categories ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateCategoryArgs {
	pub name: String,
	/// count | length | area
	pub kind: Option<String>,
	pub color: Option<String>,
	pub icon: Option<String>,
	pub description: Option<String>,
	/// Where the reference symbol is drawn (usually the legend), read off a view.
	pub legend_view: Option<String>,
	pub legend_box: Option<Value>,
}

fn parse_kind(k: Option<&str>) -> Result<CategoryKind, String> {
	match k.unwrap_or("count").to_lowercase().as_str() {
		"count" => Ok(CategoryKind::Count),
		"length" | "linear" => Ok(CategoryKind::Length),
		"area" => Ok(CategoryKind::Area),
		o => Err(format!("unknown kind '{o}' (count, length, area)")),
	}
}

pub fn create_category(ctx: &mut ToolCtx, a: CreateCategoryArgs) -> Result<ToolOutput, String> {
	let kind = parse_kind(a.kind.as_deref())?;
	if ctx.doc.lock().data.categories.iter().any(|c| c.name.eq_ignore_ascii_case(a.name.trim())) {
		return Err(format!("a category named '{}' already exists — use update_category", a.name));
	}
	let legend = match (&a.legend_view, &a.legend_box) {
		(Some(v), Some(b)) => {
			let view = ctx.view(v)?;
			let (x, y, w, h) = view_box_to_page(&view, b)?;
			Some(Region { page: view.page, x, y, w, h })
		}
		_ => None,
	};
	let id = new_id("c");
	let mut doc = ctx.doc.lock();
	let mut c = Category::new(id.clone(), a.name.trim(), a.color.unwrap_or_else(|| doc.data.next_category_color().to_string()), kind);
	c.icon = a.icon;
	c.description = a.description;
	c.legend_region = legend;
	doc.edit(&format!("AI: new item {}", a.name.trim()), |d| d.categories.push(c));
	drop(doc);
	changed(ctx);
	Ok(ToolOutput::text(format!("Created {kind:?} item '{}' (id {id}).", a.name.trim())))
}

#[derive(Deserialize)]
pub struct UpdateCategoryArgs {
	/// Id or current name.
	pub category: String,
	pub name: Option<String>,
	pub color: Option<String>,
	pub icon: Option<String>,
	pub description: Option<String>,
	pub legend_view: Option<String>,
	pub legend_box: Option<Value>,
}

pub fn update_category(ctx: &mut ToolCtx, a: UpdateCategoryArgs) -> Result<ToolOutput, String> {
	let id = resolve_category(ctx, &a.category, CategoryKind::Count, false, None)?;
	let legend = match (&a.legend_view, &a.legend_box) {
		(Some(v), Some(b)) => {
			let view = ctx.view(v)?;
			let (x, y, w, h) = view_box_to_page(&view, b)?;
			Some(Region { page: view.page, x, y, w, h })
		}
		_ => None,
	};
	ctx.doc.lock().edit("AI: update item", |d| {
		if let Some(c) = d.categories.iter_mut().find(|c| c.id == id) {
			if let Some(n) = &a.name {
				c.name = n.trim().to_string();
			}
			if let Some(col) = &a.color {
				c.color = col.clone();
			}
			if a.icon.is_some() {
				c.icon = a.icon.clone();
			}
			if a.description.is_some() {
				c.description = a.description.clone();
			}
			if legend.is_some() {
				c.legend_region = legend;
			}
		}
	});
	changed(ctx);
	Ok(ToolOutput::text(format!("Updated item {id}.")))
}

// ── summary ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SummaryArgs {
	pub page: Option<u32>,
	/// Space id or name: only what is in it (or in spaces inside it).
	pub space: Option<String>,
}

pub fn summary(ctx: &mut ToolCtx, a: SummaryArgs) -> Result<ToolOutput, String> {
	let doc = ctx.doc.lock();
	let d = &doc.data;
	let mut out = String::new();
	let spaces = takeoff_core::spaces::Spaces::new(d);
	let of = spaces.attribute(d);
	let within = a.space.as_deref().map(|k| resolve_space(d, k)).transpose()?;
	let keep = |id: &str| within.as_deref().is_none_or(|w| spaces.is_within(of.get(id).map(String::as_str), w));
	let lines: Vec<takeoff_core::list::SummaryLine> = match &within {
		Some(_) => takeoff_core::list::summary_where(&doc, |id, _| keep(id)).into_iter().filter(|l| l.items > 0).collect(),
		None => takeoff_core::list::summary(&doc),
	};
	match &within {
		Some(w) => out.push_str(&format!("Takeoff summary (in {}):\n", spaces.path_name(w))),
		None => out.push_str("Takeoff summary (whole drawing):\n"),
	}
	if lines.is_empty() {
		out.push_str("  nothing taken off yet\n");
	}
	for l in &lines {
		out.push_str(&format!(
			"  {} [{}]: {} ({} item{}{}){}\n",
			l.name,
			format!("{:?}", l.kind).to_lowercase(),
			l.total_text,
			l.items,
			if l.items == 1 { "" } else { "s" },
			if l.pending > 0 { format!(", {} pending review", l.pending) } else { String::new() },
			l.warning.as_ref().map(|w| format!(" — {w}")).unwrap_or_default()
		));
	}
	// Per space, when there are spaces and no space was asked for.
	if within.is_none() && !spaces.is_empty() {
		out.push_str("By space (each quantity once, in the deepest space it is in):\n");
		for g in takeoff_core::list::summary_by_space(&doc) {
			let items: Vec<String> = g.lines.iter().map(|l| format!("{} {}", l.name, l.total_text)).collect();
			out.push_str(&format!("  {}{}: {}\n", g.name, g.page.map(|p| format!(" (p{p})")).unwrap_or_default(), items.join(", ")));
		}
	}
	// Per page × category.
	let mut pages: Vec<u32> = d.marks.iter().filter(|m| keep(&m.id)).map(|m| m.page).chain(d.measurements.iter().filter(|m| keep(&m.id)).map(|m| m.page)).collect();
	pages.sort_unstable();
	pages.dedup();
	if let Some(p) = a.page {
		pages.retain(|x| *x == p);
	}
	for p in &pages {
		out.push_str(&format!("Page {p}{}:\n", if d.scale_for(*p).is_none() { " (NO SCALE — measurements have no values)" } else { "" }));
		for c in d.categories.iter().map(|c| Some(c.id.clone())).chain(std::iter::once(None)) {
			let marks: Vec<&Mark> = d.marks.iter().filter(|m| m.page == *p && m.category_id == c && keep(&m.id)).collect();
			let meas: Vec<&Measurement> = d.measurements.iter().filter(|m| m.page == *p && m.category_id == c && keep(&m.id)).collect();
			if marks.is_empty() && meas.is_empty() {
				continue;
			}
			let name = c.as_deref().and_then(|id| d.category(id)).map(|c| c.name.as_str()).unwrap_or("ungrouped");
			if !marks.is_empty() {
				let pending = marks.iter().filter(|m| m.status == MarkStatus::Pending).count();
				out.push_str(&format!("  {name}: {} ea ({pending} pending, {} confirmed)\n", marks.len(), marks.len() - pending));
			}
			if !meas.is_empty() {
				let mut sum = 0.0;
				let mut unit = String::new();
				for m in &meas {
					if let (Some(v), Some(u)) = (m.value, &m.unit) {
						sum += v;
						unit = u.clone();
					}
				}
				out.push_str(&format!("  {name}: {} measurement(s), {sum:.2} {unit}\n", meas.len()));
			}
		}
	}
	let unscaled: Vec<String> = (1..=doc.page_count()).filter(|p| d.scale_for(*p).is_none()).map(|p| p.to_string()).collect();
	if !unscaled.is_empty() && unscaled.len() <= 40 {
		out.push_str(&format!("Pages with no scale: {}\n", unscaled.join(", ")));
	} else if !unscaled.is_empty() {
		out.push_str(&format!("{} pages have no scale\n", unscaled.len()));
	}
	Ok(ToolOutput::text(out))
}

// ── annotations ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AnnotationArgs {
	pub view: String,
	/// text | callout | cloud | rectangle | ellipse | line | arrow | polyline | polygon | highlight
	pub kind: String,
	/// For box-shaped kinds (text, cloud, rectangle, ellipse, highlight, callout box).
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	/// For line kinds, or the callout's leader tip as a single point.
	pub points: Option<Vec<Value>>,
	pub text: Option<String>,
	pub color: Option<String>,
}

pub fn add_annotation(ctx: &mut ToolCtx, a: AnnotationArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	let kind = AnnotationKind::ALL
		.iter()
		.copied()
		.find(|k| format!("{k:?}").eq_ignore_ascii_case(&a.kind) || k.label().eq_ignore_ascii_case(&a.kind))
		.ok_or_else(|| format!("unknown annotation kind '{}'", a.kind))?;
	let pts_from = |ps: &Vec<Value>| ps.iter().map(|p| view_point_to_page(&v, p)).collect::<Result<Vec<_>, _>>();
	let mut points = match &a.bbox {
		Some(b) => {
			let (x, y, w, h) = view_box_to_page(&v, b)?;
			vec![Point { x, y }, Point { x: x + w, y: y + h }]
		}
		None => Vec::new(),
	};
	match kind {
		AnnotationKind::Callout => {
			if points.is_empty() {
				return Err("callout needs box (the text box) and points [[tip]]".into());
			}
			if let Some(ps) = &a.points {
				points.extend(pts_from(ps)?.into_iter().take(1));
			}
		}
		AnnotationKind::Line | AnnotationKind::Arrow | AnnotationKind::Polyline | AnnotationKind::Polygon | AnnotationKind::Ink => {
			points = pts_from(a.points.as_ref().ok_or("this kind needs points")?)?;
			if points.len() < 2 {
				return Err("needs at least 2 points".into());
			}
		}
		_ => {
			if points.is_empty() {
				return Err("this kind needs a box".into());
			}
		}
	}
	let mut style = AnnotationStyle::default();
	if kind == AnnotationKind::Highlight {
		style.stroke = "#FDE047".into();
	}
	if let Some(c) = a.color {
		style.stroke = c;
	}
	let id = new_id("an");
	let an = Annotation { id: id.clone(), page: v.page, kind, points, text: a.text, style, subject: None, note: None, source: Some(Source::Ai), meta: ai_meta() };
	ctx.doc.lock().edit(&format!("AI: add {}", kind.label()), |d| d.annotations.push(an));
	changed(ctx);
	let mut out = ToolOutput::text(format!("Added {} {id}.", kind.label()));
	if let Some(img) = evidence(ctx, v.page, std::slice::from_ref(&id), "add_annotation") {
		out.images.push(img);
	}
	Ok(out)
}
