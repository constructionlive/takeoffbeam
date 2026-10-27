//! Scale: set from two points + a printed dimension, from a printed scale
//! preset, measure-back checks, and following drawing lines.

use serde::Deserialize;
use serde_json::Value;
use takeoff_core::geometry::{point_distance_px, scale_units_per_px};
use takeoff_core::units::{length_factor, ScalePreset};
use takeoff_core::*;

use super::items::{ai_meta, resolve_category};
use super::render::evidence;
use super::{view_point_to_page, ToolCtx, ToolOutput};
use crate::AgentEvent;

#[derive(Deserialize)]
pub struct SetScaleArgs {
	pub view: String,
	pub p1: Value,
	pub p2: Value,
	pub distance: f64,
	/// ft, in, m, cm, mm
	pub unit: String,
	/// The printed dimension text (provenance), e.g. 22'-6"
	pub label: Option<String>,
	/// Snap the two points onto the drawing's line work (default true).
	pub snap: Option<bool>,
}

fn check_unit(u: &str) -> Result<String, String> {
	let u = u.trim().to_lowercase();
	let u = match u.as_str() {
		"feet" | "foot" | "'" => "ft".to_string(),
		"inch" | "inches" | "\"" => "in".to_string(),
		"meter" | "meters" | "metre" | "metres" => "m".to_string(),
		other => other.to_string(),
	};
	if MEASURE_UNITS.contains(&u.as_str()) {
		Ok(u)
	} else {
		Err(format!("unit must be one of {MEASURE_UNITS:?}"))
	}
}

pub(crate) fn snap_pair(ctx: &mut ToolCtx, page: u32, p1: Point, p2: Point) -> (Point, Point, String) {
	match ctx.snap_index(page) {
		Ok(idx) if !idx.is_empty() => {
			let (pts, moved) = takeoff_core::snap::fit_points(&idx, &[p1, p2], 6.0);
			let note = if moved.iter().any(|m| *m > 0.0) { format!(" Endpoints snapped to line work (moved {:.1} / {:.1} pt).", moved[0], moved[1]) } else { String::new() };
			(pts[0], pts[1], note)
		}
		_ => (p1, p2, String::new()),
	}
}

pub fn set_scale(ctx: &mut ToolCtx, a: SetScaleArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	let unit = check_unit(&a.unit)?;
	#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN is refused too
	if !(a.distance > 0.0) {
		return Err("distance must be positive".into());
	}
	let mut p1 = view_point_to_page(&v, &a.p1)?;
	let mut p2 = view_point_to_page(&v, &a.p2)?;
	let mut note = String::new();
	if a.snap.unwrap_or(true) {
		(p1, p2, note) = snap_pair(ctx, v.page, p1, p2);
	}
	let (w, h) = ctx.doc.lock().page_size_pt(v.page);
	let span_pt = point_distance_px(p1, p2, w, h);
	if span_pt < 10.0 {
		return Err(format!("the two points are only {span_pt:.1} pt apart — pick the dimension's two arrowhead/tick points, not the text"));
	}
	let scale = PageScale { page: v.page, p1, p2, distance: a.distance, unit: unit.clone() };
	let replaced = ctx.doc.lock().data.scale_for(v.page).is_some();
	ctx.doc.lock().edit("AI: set scale", |d| {
		d.page_scales.retain(|s| s.page != v.page);
		d.page_scales.push(scale);
	});
	ctx.emit(AgentEvent::DocChanged);
	// Paper scale, for a sanity check against the title block.
	let paper_in = span_pt / 72.0;
	let real_ft = a.distance * length_factor(&unit, "ft").unwrap_or(1.0);
	let ratio = if paper_in > 0.0 { real_ft / paper_in } else { 0.0 };
	let mut out = ToolOutput::text(format!(
		"Scale {} on page {}: {} {} over {:.1} pt of paper ≈ 1\" = {:.1}' ({}).{note} Now check it: measure a DIFFERENT printed dimension with check_scale — agreement within ~3% means it holds.",
		if replaced { "replaced" } else { "set" },
		v.page,
		a.distance,
		unit,
		span_pt,
		ratio,
		a.label.as_deref().unwrap_or("no label"),
	));
	// Caliper evidence: a temporary length measurement shown and removed.
	let probe = Measurement::new(new_id("probe"), v.page, MeasurementType::Length, vec![p1, p2]);
	let pid = probe.id.clone();
	ctx.doc.lock().data.measurements.push(probe);
	let img = evidence(ctx, v.page, std::slice::from_ref(&pid), "set_scale");
	ctx.doc.lock().data.measurements.retain(|m| m.id != pid);
	if let Some(img) = img {
		out.images.push(img);
	}
	Ok(out)
}

#[derive(Deserialize)]
pub struct PresetArgs {
	pub page: Option<u32>,
	/// e.g. 1/4" = 1'-0", 1" = 20', 1:100
	pub preset: String,
	/// Apply to every page.
	pub all_pages: Option<bool>,
}

fn norm(s: &str) -> String {
	s.chars().filter(|c| !c.is_whitespace()).map(|c| match c { '″' | '”' | '“' => '"', '′' | '’' | '‘' => '\'', c => c }).collect::<String>().to_lowercase()
}

pub fn set_scale_preset(ctx: &mut ToolCtx, a: PresetArgs) -> Result<ToolOutput, String> {
	let want = norm(&a.preset);
	let preset = ScalePreset::all()
		.find(|p| norm(p.label) == want || norm(p.label).replace("-0\"", "") == want.replace("-0\"", ""))
		.copied()
		.ok_or_else(|| format!("unknown preset '{}'. Known: {}", a.preset, ScalePreset::all().map(|p| p.label).collect::<Vec<_>>().join(", ")))?;
	let pages: Vec<u32> = if a.all_pages.unwrap_or(false) { (1..=ctx.page_count()).collect() } else { vec![ctx.page_or_current(a.page)?] };
	let scales: Vec<PageScale> = {
		let doc = ctx.doc.lock();
		pages.iter().map(|p| preset.to_page_scale(*p, doc.page_size_pt(*p).0)).collect()
	};
	ctx.doc.lock().edit("AI: scale preset", |d| {
		d.page_scales.retain(|s| !pages.contains(&s.page));
		d.page_scales.extend(scales);
	});
	ctx.emit(AgentEvent::DocChanged);
	Ok(ToolOutput::text(format!(
		"Set {} on {} page(s). A printed scale only holds if the sheet is printed at full size — verify with check_scale on a printed dimension.",
		preset.label,
		pages.len()
	)))
}

#[derive(Deserialize)]
pub struct CheckScaleArgs {
	pub view: String,
	pub p1: Value,
	pub p2: Value,
	/// The printed value you expect (same unit as the scale), to get the error %.
	pub expected: Option<f64>,
	pub snap: Option<bool>,
}

pub fn check_scale(ctx: &mut ToolCtx, a: CheckScaleArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	let mut p1 = view_point_to_page(&v, &a.p1)?;
	let mut p2 = view_point_to_page(&v, &a.p2)?;
	let mut note = String::new();
	if a.snap.unwrap_or(true) {
		(p1, p2, note) = snap_pair(ctx, v.page, p1, p2);
	}
	let doc = ctx.doc.lock();
	let (w, h) = doc.page_size_pt(v.page);
	let scale = doc.data.scale_at(v.page, Point { x: (p1.x + p2.x) / 2.0, y: (p1.y + p2.y) / 2.0 }).ok_or("this page has no scale yet")?;
	let upp = scale_units_per_px(scale, w, h).ok_or("the page scale is degenerate")?;
	let measured = point_distance_px(p1, p2, w, h) * upp;
	let mut text = format!("Measured {measured:.3} {} under the current scale.{note}", scale.unit);
	if let Some(e) = a.expected.filter(|e| *e > 0.0) {
		let err = (measured - e).abs() / e * 100.0;
		text.push_str(&format!(" Expected {e} → error {err:.1}% ({}).", if err <= 3.0 { "scale verified" } else { "DOES NOT AGREE — recheck the points or the scale" }));
	}
	Ok(ToolOutput::text(text))
}

#[derive(Deserialize)]
pub struct FollowArgs {
	pub view: String,
	pub point: Value,
	/// Save the run as a length measurement in this category.
	pub add_to: Option<String>,
	pub label: Option<String>,
}

pub fn follow_line(ctx: &mut ToolCtx, a: FollowArgs) -> Result<ToolOutput, String> {
	let v = ctx.view(&a.view)?;
	let p = view_point_to_page(&v, &a.point)?;
	let idx = ctx.snap_index(v.page)?;
	if idx.is_empty() {
		return Err("no drawing lines on this page (scanned sheet) — trace the run with add_measurements instead".into());
	}
	let snap = idx.nearest(p, 12.0).ok_or("no drawing line within 12 pt of that point")?;
	let run = idx.follow(snap.segment, 0.75);
	let doc = ctx.doc.lock();
	let (w, h) = doc.page_size_pt(v.page);
	let len_pt = takeoff_core::geometry::polyline_length_px(&run, w, h);
	let real = doc.data.scale_at(v.page, p).and_then(|s| scale_units_per_px(s, w, h).map(|u| (len_pt * u, s.unit.clone())));
	drop(doc);
	let mut text = format!(
		"Followed {} segment(s) from that point until a fork or gap: {:.1} pt of paper{}.",
		run.len().saturating_sub(1),
		len_pt,
		real.as_ref().map(|(r, u)| format!(" = {r:.2} {u}")).unwrap_or_else(|| " (no scale yet)".into())
	);
	let mut out = ToolOutput::text(String::new());
	if let Some(cat) = &a.add_to {
		let cid = resolve_category(ctx, cat, CategoryKind::Length, true, None)?;
		let mut m = Measurement::new(new_id("ms"), v.page, MeasurementType::Length, run);
		m.category_id = Some(cid);
		m.label = a.label.clone();
		m.source = Some(Source::Ai);
		m.meta = ai_meta();
		let id = m.id.clone();
		ctx.doc.lock().edit("AI: follow line", |d| d.measurements.push(m));
		ctx.emit(AgentEvent::DocChanged);
		text.push_str(&format!(" Saved as {id} in {cat}."));
		if let Some(img) = evidence(ctx, v.page, std::slice::from_ref(&id), "follow_line") {
			out.images.push(img);
		}
	}
	out.text = text;
	Ok(out)
}
