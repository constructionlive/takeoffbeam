//! `view`: render part of a page with the takeoff burned in, register it as
//! a view id, and describe what is on it. Also the evidence renders every
//! write attaches.

use std::collections::HashSet;
use std::io::Cursor;

use serde::Deserialize;
use takeoff_core::coords::{image_fit, safe_max_edge};
use takeoff_core::numbering::{mark_seq, measurement_seq};
use takeoff_core::{ItemRef, Point};
use takeoff_pdf::burn::{burn, BurnOptions};
use takeoff_pdf::FracRect;

use super::{page_box_to_view, view_box_to_page, ToolCtx, ToolOutput, View, ViewImage};
use crate::AgentEvent;

#[derive(Deserialize, Default)]
pub struct RegionArg {
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

#[derive(Deserialize, Default)]
pub struct ViewArgs {
	pub page: Option<u32>,
	/// Page fractions of the area to show (omit = whole page).
	pub region: Option<RegionArg>,
	/// Zoom into a box of an earlier view instead of giving fractions.
	pub zoom_view: Option<String>,
	pub zoom_box: Option<serde_json::Value>,
	/// Category ids or names to draw (omit = all).
	pub categories: Option<Vec<String>>,
	pub show_marks: Option<bool>,
	pub show_measurements: Option<bool>,
	pub show_annotations: Option<bool>,
	pub show_overlays: Option<bool>,
	/// Hide these PDF layers for this render only.
	pub hidden_layers: Option<Vec<String>>,
	pub max_px: Option<u32>,
}

const VIEW_DEFAULT_MAX_PX: u32 = 2000;
const INLINE_INDEX_LIMIT: usize = 120;

pub(crate) struct RenderSpec {
	pub page: u32,
	pub region: FracRect,
	pub burn: BurnOptions,
	pub preferred_max: u32,
	pub hidden_override: Option<Vec<String>>,
}

/// Render + burn + register. Returns the image and its view.
pub(crate) fn render_view(ctx: &mut ToolCtx, spec: RenderSpec) -> Result<(ViewImage, View), String> {
	ctx.sync_layers(spec.hidden_override.as_deref())?;
	let max = safe_max_edge(&ctx.model_id, spec.preferred_max.clamp(256, 6000));
	let r = ctx.drawing.render(spec.page, 400.0, spec.region, Some(max)).map_err(|e| e.to_string())?;
	let mut img = r.image;
	let (fw, fh) = image_fit(&ctx.model_id, img.width(), img.height());
	if (fw, fh) != img.dimensions() {
		img = image::imageops::resize(&img, fw, fh, image::imageops::FilterType::Triangle);
	}
	{
		let doc = ctx.doc.lock();
		let size = doc.page_size_pt(spec.page);
		burn(&mut img, r.region, size, &doc.data, spec.page, &spec.burn);
	}
	if spec.hidden_override.is_some() {
		ctx.sync_layers(None)?;
	}
	let view = ctx.register_view(spec.page, r.region, img.width(), img.height());
	let mut png = Vec::new();
	img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
	let label = ctx.drawing.page_label(spec.page).ok().flatten().filter(|l| *l != spec.page.to_string());
	let caption = format!(
		"View {} · page {}{} · region x={:.3} y={:.3} w={:.3} h={:.3} of the page · {}x{} px. {}. This render is a SNAPSHOT: after any write, look again.",
		view.id,
		spec.page,
		label.map(|l| format!(" ({l})")).unwrap_or_default(),
		r.region.x,
		r.region.y,
		r.region.w,
		r.region.h,
		img.width(),
		img.height(),
		view.dialect.reading_rule(img.width(), img.height()),
	);
	// With several drawings open, say which one this is.
	let names = ctx.drawing_names();
	let caption = if names.len() > 1 {
		let active = names.iter().find(|n| n.1).map(|n| n.0.clone()).unwrap_or_default();
		format!("[{active}] {caption}")
	} else {
		caption
	};
	ctx.emit(AgentEvent::Viewport {
		page: spec.page,
		region: (r.region.x, r.region.y, r.region.w, r.region.h),
		label: format!("Looking at {}", view.id),
		drawing: None,
	});
	Ok((ViewImage { png, width: img.width(), height: img.height(), view_id: view.id.clone(), caption }, view))
}

/// Badge → id index of what is visible in a view, in the view's own units.
pub(crate) fn index_for(ctx: &ToolCtx, v: &View, cats: Option<&HashSet<String>>) -> String {
	let doc = ctx.doc.lock();
	let d = &doc.data;
	let inside = |b: (f64, f64, f64, f64)| {
		b.0 + b.2 >= v.region.x && b.1 + b.3 >= v.region.y && b.0 <= v.region.x + v.region.w && b.1 <= v.region.y + v.region.h
	};
	let want = |c: &Option<String>| cats.is_none_or(|s| c.as_ref().is_some_and(|c| s.contains(c)));
	let mseq = mark_seq(&d.marks);
	let sseq = measurement_seq(&d.measurements);
	let name = |c: &Option<String>| c.as_deref().and_then(|id| d.category(id)).map(|c| c.name.clone()).unwrap_or_else(|| "ungrouped".into());
	let mut lines: Vec<String> = Vec::new();
	let mut total = 0;
	for m in d.marks.iter().filter(|m| m.page == v.page && want(&m.category_id) && inside((m.x, m.y, m.w, m.h))) {
		total += 1;
		if total <= INLINE_INDEX_LIMIT {
			let b = page_box_to_view(v, (m.x, m.y, m.w, m.h)).map(|b| format!(" at {b:?}")).unwrap_or_default();
			lines.push(format!("  mark #{} {} id={} {}{}", mseq.get(&m.id).unwrap_or(&0), name(&m.category_id), m.id, if m.status == takeoff_core::MarkStatus::Pending { "pending" } else { "confirmed" }, b));
		}
	}
	for m in d.measurements.iter().filter(|m| m.page == v.page && want(&m.category_id)) {
		let Some(bb) = takeoff_core::geometry::bounds(&m.points) else { continue };
		if !inside(bb) {
			continue;
		}
		total += 1;
		if total <= INLINE_INDEX_LIMIT {
			let val = match (m.value, &m.unit) {
				(Some(x), Some(u)) => format!("{x:.2} {u}"),
				_ => "no scale".into(),
			};
			lines.push(format!("  {} #{} {} id={} = {val}", format!("{:?}", m.kind).to_lowercase(), sseq.get(&m.id).unwrap_or(&0), name(&m.category_id), m.id));
		}
	}
	for a in d.annotations.iter().filter(|a| a.page == v.page) {
		if let Some(bb) = takeoff_core::geometry::bounds(&a.points) {
			if inside(bb) {
				total += 1;
				if total <= INLINE_INDEX_LIMIT {
					lines.push(format!("  annotation {} id={}{}", a.kind.label(), a.id, a.text.as_deref().map(|t| format!(" \"{}\"", t.chars().take(40).collect::<String>())).unwrap_or_default()));
				}
			}
		}
	}
	if total == 0 {
		return "Nothing marked in this view yet.".into();
	}
	if total > INLINE_INDEX_LIMIT {
		lines.push(format!("  … {} more (use find_items_at or read_items)", total - INLINE_INDEX_LIMIT));
	}
	format!("Items in this view ({total}; boxes in this view's units):\n{}", lines.join("\n"))
}

pub fn view(ctx: &mut ToolCtx, a: ViewArgs) -> Result<ToolOutput, String> {
	let (page, region) = match (&a.zoom_view, &a.zoom_box) {
		(Some(vid), Some(b)) => {
			let v = ctx.view(vid)?;
			let (x, y, w, h) = view_box_to_page(&v, b)?;
			(v.page, FracRect { x, y, w, h })
		}
		_ => {
			let page = ctx.page_or_current(a.page)?;
			let region = a.region.map(|r| FracRect { x: r.x, y: r.y, w: r.w, h: r.h }).unwrap_or(FracRect::FULL);
			(page, region)
		}
	};
	if region.w <= 0.0 || region.h <= 0.0 {
		return Err("region must have positive w and h (page fractions 0-1)".into());
	}
	let region = usable_region(region, ctx.doc.lock().page_size_pt(page));
	let cats = resolve_category_filter(ctx, a.categories.as_deref());
	let burn = BurnOptions {
		marks: a.show_marks.unwrap_or(true),
		measurements: a.show_measurements.unwrap_or(true),
		annotations: a.show_annotations.unwrap_or(true),
		overlays: a.show_overlays.unwrap_or(true),
		categories: cats.clone(),
		..Default::default()
	};
	let (img, v) = render_view(
		ctx,
		RenderSpec { page, region, burn, preferred_max: a.max_px.unwrap_or(VIEW_DEFAULT_MAX_PX), hidden_override: a.hidden_layers.clone() },
	)?;
	let mut text = format!("{}\n{}", img.caption, index_for(ctx, &v, cats.as_ref()));
	if region.w * region.h > 0.5 && (img.width.max(img.height) as f64) < 2500.0 {
		text.push_str("\nTip: at whole-page scale small text and symbols are unreadable — zoom with view(zoom_view, zoom_box) or a region before reading or marking.");
	}
	Ok(ToolOutput { text, images: vec![img] })
}

/// Zooms narrower than a few percent of the page, or thinner than 1:3, show
/// too little to read: grow them around their centre (in paper terms).
fn usable_region(r: FracRect, (w_pt, h_pt): (f64, f64)) -> FracRect {
	const MIN_PT: f64 = 36.0; // half an inch of paper
	let (mut w, mut h) = ((r.w * w_pt).max(MIN_PT), (r.h * h_pt).max(MIN_PT));
	if w > 3.0 * h {
		h = w / 3.0;
	} else if h > 3.0 * w {
		w = h / 3.0;
	}
	let (fw, fh) = ((w / w_pt).min(1.0), (h / h_pt).min(1.0));
	let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
	FracRect { x: (cx - fw / 2.0).clamp(0.0, 1.0 - fw), y: (cy - fh / 2.0).clamp(0.0, 1.0 - fh), w: fw, h: fh }
}

pub(crate) fn resolve_category_filter(ctx: &ToolCtx, names: Option<&[String]>) -> Option<HashSet<String>> {
	let names = names?;
	let doc = ctx.doc.lock();
	Some(
		names
			.iter()
			.filter_map(|n| {
				doc.data
					.categories
					.iter()
					.find(|c| c.id == *n || c.name.eq_ignore_ascii_case(n))
					.map(|c| c.id.clone())
			})
			.collect(),
	)
}

/// Render the area around changed items so the model can check its write.
pub(crate) fn evidence(ctx: &mut ToolCtx, page: u32, ids: &[String], what: &str) -> Option<ViewImage> {
	if ids.is_empty() {
		return None;
	}
	let bbox = {
		let doc = ctx.doc.lock();
		let mut pts: Vec<Point> = Vec::new();
		for id in ids {
			match doc.data.find(id) {
				Some(ItemRef::Mark(_)) => {
					if let Some(m) = doc.data.marks.iter().find(|m| &m.id == id) {
						pts.push(Point { x: m.x, y: m.y });
						pts.push(Point { x: m.x + m.w, y: m.y + m.h });
					}
				}
				Some(ItemRef::Measurement(_)) => {
					if let Some(m) = doc.data.measurements.iter().find(|m| &m.id == id) {
						pts.extend_from_slice(&m.points);
					}
				}
				Some(ItemRef::Annotation(_)) => {
					if let Some(a) = doc.data.annotations.iter().find(|a| &a.id == id) {
						pts.extend_from_slice(&a.points);
					}
				}
				None => {
					if let Some(o) = doc.data.overlays.iter().find(|o| &o.id == id) {
						pts.push(Point { x: o.x, y: o.y });
						pts.push(Point { x: o.x + o.w, y: o.y + o.h });
					}
				}
			}
		}
		takeoff_core::geometry::bounds(&pts)?
	};
	let pad = (bbox.2.max(bbox.3) * 0.5).max(0.03);
	let region = FracRect { x: bbox.0 - pad, y: bbox.1 - pad, w: bbox.2 + 2.0 * pad, h: bbox.3 + 2.0 * pad };
	let burn = BurnOptions { highlight: ids.iter().cloned().collect(), overlays: true, ..Default::default() };
	let (mut img, _) = render_view(ctx, RenderSpec { page, region, burn, preferred_max: 1200, hidden_override: None }).ok()?;
	img.caption = format!("Check render after {what} (changed items drawn bold). {}", img.caption);
	Some(img)
}
