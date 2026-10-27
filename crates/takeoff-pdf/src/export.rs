//! Export a marked-up PDF: the takeoff drawn into the pages as real vector
//! content (Bluebeam "Flatten"), so any viewer or printer shows it. The
//! source file is never modified.

use std::path::Path;

use pdfium_render::prelude::*;
use takeoff_core::numbering::{mark_seq, measurement_seq};
use takeoff_core::units::format_quantity;
use takeoff_core::*;

use crate::burn::parse_color;
use crate::vectors::PageAffine;
use crate::{pdfium, PdfError};

/// Fractions → PDF user space (inverse of the page affine).
struct Inv {
	a: f64,
	b: f64,
	c: f64,
	d: f64,
	e: f64,
	f: f64,
}

impl Inv {
	fn of(page: &PdfPage) -> Result<Self, PdfiumError> {
		// Probe the forward map at three user-space points and invert it.
		let fwd = PageAffine::for_page(page)?;
		let o = fwd.apply(0.0, 0.0);
		let xu = fwd.apply(1.0, 0.0);
		let yu = fwd.apply(0.0, 1.0);
		let (a, b, c, d) = (xu.x - o.x, xu.y - o.y, yu.x - o.x, yu.y - o.y);
		let det = a * d - b * c;
		Ok(Self { a: d / det, b: -b / det, c: -c / det, d: a / det, e: (c * o.y - d * o.x) / det, f: (b * o.x - a * o.y) / det })
	}
	fn pt(&self, p: Point) -> (PdfPoints, PdfPoints) {
		(PdfPoints::new((self.a * p.x + self.c * p.y + self.e) as f32), PdfPoints::new((self.b * p.x + self.d * p.y + self.f) as f32))
	}
	/// Page points per fraction along x (for sizes).
	fn scale(&self) -> f64 {
		self.a.hypot(self.b)
	}
}

fn color(hex: &str, alpha: f64) -> PdfColor {
	let (r, g, b) = parse_color(hex);
	PdfColor::new(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

struct Painter<'a> {
	doc: &'a PdfDocument<'static>,
	page: PdfPage<'static>,
	inv: Inv,
	font: PdfFontToken,
}

impl Painter<'_> {
	fn shape(&mut self, pts: &[Point], closed: bool, stroke: Option<(PdfColor, f32)>, fill: Option<PdfColor>, dash: Option<&[f32]>) -> Result<(), PdfiumError> {
		if pts.len() < 2 {
			return Ok(());
		}
		let (x0, y0) = self.inv.pt(pts[0]);
		let mut path = PdfPagePathObject::new(self.doc, x0, y0, stroke.map(|s| s.0), stroke.map(|s| PdfPoints::new(s.1)), fill)?;
		for p in &pts[1..] {
			let (x, y) = self.inv.pt(*p);
			path.line_to(x, y)?;
		}
		if closed {
			path.close_path()?;
		}
		path.set_fill_and_stroke_mode(if fill.is_some() { PdfPathFillMode::EvenOdd } else { PdfPathFillMode::None }, stroke.is_some())?;
		if let (Some(d), Some((_, w))) = (dash, stroke) {
			let arr: Vec<PdfPoints> = d.iter().map(|v| PdfPoints::new(v * w)).collect();
			path.set_dash_array(&arr, PdfPoints::ZERO)?;
		}
		self.page.objects_mut().add_path_object(path)?;
		Ok(())
	}

	/// Text whose top-left is at `at` (fractions), upright as the page is displayed.
	fn text(&mut self, at: Point, size_pt: f32, text: &str, rgb: &str) -> Result<(), PdfiumError> {
		let mut t = PdfPageTextObject::new(self.doc, text, self.font, PdfPoints::new(size_pt))?;
		t.set_fill_color(color(rgb, 1.0))?;
		// Display "right" and "down" in user space: the text baseline follows "right".
		let angle = self.inv.b.atan2(self.inv.a);
		t.rotate_counter_clockwise_radians(angle as f32)?;
		let down = (self.inv.c, self.inv.d);
		let down_len = down.0.hypot(down.1);
		let (x, y) = self.inv.pt(at);
		let k = size_pt as f64 * 0.8 / down_len.max(1e-9);
		t.translate(x + PdfPoints::new((down.0 * k) as f32), y + PdfPoints::new((down.1 * k) as f32))?;
		self.page.objects_mut().add_text_object(t)?;
		Ok(())
	}

	fn rect(&self, a: Point, b: Point) -> Vec<Point> {
		vec![a, Point { x: b.x, y: a.y }, b, Point { x: a.x, y: b.y }]
	}
}

fn ellipse_points(a: Point, b: Point) -> Vec<Point> {
	let (cx, cy, rx, ry) = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, (a.x - b.x).abs() / 2.0, (a.y - b.y).abs() / 2.0);
	(0..64).map(|i| {
		let t = i as f64 / 64.0 * std::f64::consts::TAU;
		Point { x: cx + rx * t.cos(), y: cy + ry * t.sin() }
	}).collect()
}

fn cloud_points(pts: &[Point], aspect: f64) -> Vec<Point> {
	let poly: Vec<Point> = if pts.len() == 2 { vec![pts[0], Point { x: pts[1].x, y: pts[0].y }, pts[1], Point { x: pts[0].x, y: pts[1].y }] } else { pts.to_vec() };
	let area: f64 = (0..poly.len()).map(|i| { let (a, b) = (poly[i], poly[(i + 1) % poly.len()]); a.x * b.y - b.x * a.y }).sum();
	let sign = if area >= 0.0 { -1.0 } else { 1.0 };
	let arc = 0.006;
	let mut out = Vec::new();
	for i in 0..poly.len() {
		let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
		let (dx, dy) = (b.x - a.x, (b.y - a.y) * aspect);
		let len = dx.hypot(dy).max(1e-9);
		let n = (len / arc).ceil().clamp(1.0, 10_000.0) as usize;
		let (nx, ny) = (-dy / len * sign, dx / len * sign / aspect);
		for k in 0..n {
			for s in 0..8 {
				let t = (k as f64 + s as f64 / 8.0) / n as f64;
				let bulge = (s as f64 / 8.0 * std::f64::consts::PI).sin() * len / n as f64 * 0.45;
				out.push(Point { x: a.x + (b.x - a.x) * t + nx * bulge, y: a.y + (b.y - a.y) * t + ny * bulge });
			}
		}
	}
	out
}

/// A legend box drawn as vector content, from the same layout as the canvas.
fn legend(p: &mut Painter, l: &Legend, data: &TakeoffData, aspect: f64, page_h_pt: f64) -> Result<(), PdfiumError> {
	use takeoff_core::legend::{hatch_lines, layout, rows_for, text_width, Swatch};
	let rows = rows_for(data, l);
	let lay = layout(l, &rows, aspect);
	let ink = "#282828";
	let frame = p.rect(Point { x: l.x, y: l.y }, Point { x: l.x + l.w, y: l.y + l.h });
	p.shape(&frame, true, Some((color(ink, 1.0), 0.8)), Some(color("#FFFFFF", 0.94)), None)?;
	for (a, b) in &lay.rules {
		p.shape(&[*a, *b], false, Some((color("#969696", 1.0), 0.5)), None, None)?;
	}
	for ((x, y, w, h), k) in &lay.swatches {
		let row = &rows[*k];
		let r = p.rect(Point { x: *x, y: *y }, Point { x: x + w, y: y + h });
		match row.swatch {
			Swatch::Count => p.shape(&r, true, Some((color(&row.color, 1.0), 1.0)), Some(color(&row.color, 0.18 * row.opacity)), None)?,
			Swatch::Length(style) => {
				let dash: Option<&[f32]> = match style { Some(LineStyle::Dashed) => Some(&[4.0, 3.0]), Some(LineStyle::Dotted) => Some(&[0.5, 2.0]), _ => None };
				p.shape(&[Point { x: *x, y: y + h / 2.0 }, Point { x: x + w, y: y + h / 2.0 }], false, Some((color(&row.color, 1.0), 1.5)), None, dash)?;
			}
			Swatch::Area(hk) => {
				p.shape(&r, true, None, Some(color(&row.color, 0.25 * row.opacity)), None)?;
				for (a, b) in hk.map(|hk| hatch_lines((*x, *y, *w, *h), hk, aspect, 6)).unwrap_or_default() {
					p.shape(&[a, b], false, Some((color(&row.color, 0.8), 0.5)), None, None)?;
				}
				p.shape(&r, true, Some((color(&row.color, 1.0), 0.8)), None, None)?;
			}
			Swatch::Markup(_) => p.shape(&r, true, Some((color(&row.color, row.opacity), 1.0)), None, None)?,
		}
	}
	for t in &lay.texts {
		let mut at = t.at;
		if t.right {
			at.x -= text_width(&t.text, t.size, aspect);
		}
		p.text(at, (t.size * page_h_pt) as f32, &t.text, if t.muted { "#6E6E6E" } else { ink })?;
	}
	Ok(())
}

/// Write `src` with the takeoff drawn in to `dst`.
pub fn export_flattened(src: &Path, data: &TakeoffData, dst: &Path) -> Result<u32, PdfError> {
	let mut doc = pdfium()?.load_pdf_from_file(src, None)?;
	let font = doc.fonts_mut().helvetica_bold();
	let n = doc.pages().len() as u32;
	let mseq = mark_seq(&data.marks);
	let sseq = measurement_seq(&data.measurements);
	let disp = &data.display;
	let mut pages_done = 0;
	for page_no in 1..=n {
		let has = data.marks.iter().any(|m| m.page == page_no) || data.measurements.iter().any(|m| m.page == page_no) || data.annotations.iter().any(|a| a.page == page_no) || data.legends.iter().any(|l| l.page == page_no);
		if !has {
			continue;
		}
		let page = doc.pages().get((page_no - 1) as i32)?;
		let inv = Inv::of(&page)?;
		let aspect = page.height().value as f64 / page.width().value as f64;
		let pt = 1.0 / inv.scale(); // one PDF point in x-fractions
		let mut p = Painter { doc: &doc, page, inv, font };
		for a in data.annotations.iter().filter(|a| a.page == page_no) {
			let st = &a.style;
			let stroke = Some((color(&st.stroke, st.opacity), st.width as f32));
			let fill = st.fill.as_deref().map(|f| color(f, 0.5 * st.opacity));
			let dash: Option<&[f32]> = match st.line_style { Some(LineStyle::Dashed) => Some(&[4.0, 3.0]), Some(LineStyle::Dotted) => Some(&[0.5, 2.0]), _ => None };
			let pts = &a.points;
			match a.kind {
				AnnotationKind::Rectangle | AnnotationKind::Stamp if pts.len() >= 2 => p.shape(&p.rect(pts[0], pts[1]), true, stroke, fill, dash)?,
				AnnotationKind::Highlight if pts.len() >= 2 => p.shape(&p.rect(pts[0], pts[1]), true, None, Some(color(&st.stroke, 0.35)), None)?,
				AnnotationKind::Ellipse if pts.len() >= 2 => p.shape(&ellipse_points(pts[0], pts[1]), true, stroke, fill, dash)?,
				AnnotationKind::Text | AnnotationKind::Callout if pts.len() >= 2 => {
					p.shape(&p.rect(pts[0], pts[1]), true, stroke, Some(color("#FFFFFF", 0.9)), None)?;
					if a.kind == AnnotationKind::Callout && pts.len() >= 3 {
						let (l, t, r, b) = (pts[0].x.min(pts[1].x), pts[0].y.min(pts[1].y), pts[0].x.max(pts[1].x), pts[0].y.max(pts[1].y));
						let from = Point { x: pts[2].x.clamp(l, r), y: pts[2].y.clamp(t, b) };
						p.shape(&[from, pts[2]], false, stroke, None, None)?;
					}
				}
				AnnotationKind::Cloud => p.shape(&cloud_points(pts, aspect), true, stroke, fill, None)?,
				AnnotationKind::Polygon => p.shape(pts, true, stroke, fill, dash)?,
				AnnotationKind::Line | AnnotationKind::Polyline | AnnotationKind::Ink | AnnotationKind::Arrow => {
					p.shape(pts, false, stroke, None, dash)?;
					if a.kind == AnnotationKind::Arrow && pts.len() >= 2 {
						let (f, t) = (pts[pts.len() - 2], pts[pts.len() - 1]);
						let (dx, dy) = (t.x - f.x, (t.y - f.y) * aspect);
						let len = dx.hypot(dy).max(1e-9);
						let (ux, uy) = (dx / len, dy / len);
						let s = pt * st.width.max(1.0) * 5.0;
						let head = [t, Point { x: t.x - ux * s - uy * s * 0.5, y: t.y - (uy * s - ux * s * 0.5) / aspect }, Point { x: t.x - ux * s + uy * s * 0.5, y: t.y - (uy * s + ux * s * 0.5) / aspect }];
						p.shape(&head, true, None, Some(color(&st.stroke, st.opacity)), None)?;
					}
				}
				_ => {}
			}
			if let Some(text) = a.text.as_deref().filter(|t| !t.is_empty()) {
				if pts.len() >= 2 {
					let size = st.font_size as f32;
					let mut at = Point { x: pts[0].x.min(pts[1].x) + pt * 3.0, y: pts[0].y.min(pts[1].y) + pt * 3.0 / aspect };
					for line in text.lines() {
						p.text(at, size, line, if a.kind == AnnotationKind::Stamp { &st.stroke } else { "#141414" })?;
						at.y += pt * size as f64 * 1.25 / aspect;
					}
				}
			}
		}
		let cats: std::collections::HashMap<&str, &Category> = data.categories.iter().map(|c| (c.id.as_str(), c)).collect();
		for m in data.measurements.iter().filter(|m| m.page == page_no) {
			let cat = m.category_id.as_deref().and_then(|c| cats.get(c));
			let hex = m.color.clone().or(cat.map(|c| c.color.clone())).unwrap_or_else(|| "#22D3EE".into());
			let closed = m.kind == MeasurementType::Area || m.closed;
			let fill = (m.kind == MeasurementType::Area).then(|| color(&hex, 0.2));
			let mut pts = m.points.clone();
			if m.kind == MeasurementType::Area {
				p.shape(&pts, true, Some((color(&hex, 1.0), 1.5)), fill, None)?;
				for cut in &m.cutouts {
					p.shape(cut, true, Some((color(&hex, 1.0), 0.8)), Some(color("#FFFFFF", 0.9)), Some(&[3.0, 2.0]))?;
				}
			} else {
				if closed {
					pts.push(pts[0]);
				}
				p.shape(&pts, false, Some((color(&hex, 1.0), 2.0)), None, None)?;
			}
			let n = sseq.get(&m.id).copied().unwrap_or(0);
			let label = match (m.value, &m.unit) {
				(Some(v), Some(u)) => format!("{n}  {}", format_quantity(v, u, disp)),
				_ => n.to_string(),
			};
			p.text(Point { x: m.points[0].x + pt * 3.0, y: m.points[0].y - pt * 10.0 / aspect }, 7.0, &label, &hex)?;
		}
		for m in data.marks.iter().filter(|m| m.page == page_no) {
			let cat = m.category_id.as_deref().and_then(|c| cats.get(c));
			let hex = cat.map(|c| c.color.clone()).unwrap_or_else(|| "#22D3EE".into());
			let r = p.rect(Point { x: m.x, y: m.y }, Point { x: m.x + m.w, y: m.y + m.h });
			p.shape(&r, true, Some((color(&hex, 1.0), 1.0)), Some(color(&hex, 0.15)), None)?;
			let n = mseq.get(&m.id).copied().unwrap_or(0);
			p.text(Point { x: m.x, y: m.y - pt * 7.0 / aspect }, 6.0, &n.to_string(), &hex)?;
		}
		let page_h_pt = p.page.height().value as f64;
		for l in data.legends.iter().filter(|l| l.page == page_no) {
			legend(&mut p, l, data, aspect, page_h_pt)?;
		}
		p.page.regenerate_content()?;
		pages_done += 1;
	}
	let tmp = dst.with_extension("pdf.tmp");
	doc.save_to_file(&tmp)?;
	std::fs::rename(&tmp, dst).map_err(|e| PdfError::Io(e.to_string()))?;
	Ok(pages_done)
}
