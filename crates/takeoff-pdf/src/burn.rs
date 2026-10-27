//! Burn takeoff markups into a rendered page image: what the agent "sees"
//! (numbered badges it can count and refer to), PNG exports, and previews.
//! Positions are page fractions; `region` is the part of the page `img` shows.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::RgbaImage;
use resvg::tiny_skia::{self, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};
use takeoff_core::numbering::{mark_seq, measurement_seq};
use takeoff_core::units::format_quantity;
use takeoff_core::*;

use crate::FracRect;

static FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

fn font() -> &'static FontRef<'static> {
	static F: OnceLock<FontRef<'static>> = OnceLock::new();
	F.get_or_init(|| FontRef::try_from_slice(FONT).expect("bundled font"))
}

#[derive(Debug, Clone)]
pub struct BurnOptions {
	pub marks: bool,
	pub measurements: bool,
	pub annotations: bool,
	pub scales: bool,
	/// Only these category ids (None = all).
	pub categories: Option<HashSet<String>>,
	/// Number badges on marks and measurements.
	pub badges: bool,
	/// Items drawn emphasized (thicker, full opacity); others dimmed when non-empty.
	pub highlight: HashSet<String>,
	/// Global stroke scale (1 = tuned for ~150 DPI renders).
	pub scale: f32,
	/// Blend the page's overlays in under the markups (the agent's renders; not exports).
	pub overlays: bool,
}

impl Default for BurnOptions {
	fn default() -> Self {
		Self { marks: true, measurements: true, annotations: true, scales: true, categories: None, badges: true, highlight: HashSet::new(), scale: 1.0, overlays: false }
	}
}

pub fn parse_color(hex: &str) -> (u8, u8, u8) {
	let h = hex.trim_start_matches('#');
	let v = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
	if h.len() == 3 {
		let c = |i: usize| u8::from_str_radix(&h.get(i..i + 1).unwrap_or("f").repeat(2), 16).unwrap_or(255);
		return (c(0), c(1), c(2));
	}
	(v(0), v(2), v(4))
}

fn paint(rgb: (u8, u8, u8), alpha: f32) -> Paint<'static> {
	let mut p = Paint::default();
	p.set_color_rgba8(rgb.0, rgb.1, rgb.2, (alpha.clamp(0.0, 1.0) * 255.0) as u8);
	p.anti_alias = true;
	p
}

struct Canvas {
	pix: Pixmap,
	region: FracRect,
	opts_scale: f32,
}

impl Canvas {
	fn to_px(&self, p: Point) -> (f32, f32) {
		(
			((p.x - self.region.x) / self.region.w * self.pix.width() as f64) as f32,
			((p.y - self.region.y) / self.region.h * self.pix.height() as f64) as f32,
		)
	}
	/// Base stroke width: readable at any render size.
	fn unit(&self) -> f32 {
		(self.pix.width().max(self.pix.height()) as f32 / 900.0).clamp(1.0, 6.0) * self.opts_scale
	}

	fn path(&self, pts: &[Point], close: bool) -> Option<tiny_skia::Path> {
		let mut pb = PathBuilder::new();
		let (x, y) = self.to_px(*pts.first()?);
		pb.move_to(x, y);
		for p in &pts[1..] {
			let (x, y) = self.to_px(*p);
			pb.line_to(x, y);
		}
		if close {
			pb.close();
		}
		pb.finish()
	}

	fn stroke(&mut self, path: &tiny_skia::Path, rgb: (u8, u8, u8), alpha: f32, width: f32, dash: Option<&[f32]>) {
		let mut s = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
		if let Some(d) = dash {
			s.dash = StrokeDash::new(d.iter().map(|v| v * width).collect(), 0.0);
		}
		self.pix.stroke_path(path, &paint(rgb, alpha), &s, Transform::identity(), None);
	}

	fn fill(&mut self, path: &tiny_skia::Path, rgb: (u8, u8, u8), alpha: f32) {
		self.pix.fill_path(path, &paint(rgb, alpha), FillRule::EvenOdd, Transform::identity(), None);
	}

	fn rect_path(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<tiny_skia::Path> {
		tiny_skia::Rect::from_ltrb(x0.min(x1), y0.min(y1), x0.max(x1).max(x0.min(x1) + 0.5), y0.max(y1).max(y0.min(y1) + 0.5)).map(PathBuilder::from_rect)
	}

	/// Text with a rounded pill behind it (badges, labels). `anchor` = top-left.
	fn label(&mut self, x: f32, y: f32, size: f32, text: &str, fg: (u8, u8, u8), bg: (u8, u8, u8)) -> (f32, f32) {
		let f = font().as_scaled(PxScale::from(size));
		let w: f32 = text.chars().map(|c| f.h_advance(f.glyph_id(c))).sum();
		let (pad_x, pad_y) = (size * 0.35, size * 0.15);
		let (bw, bh) = (w + 2.0 * pad_x, f.ascent() - f.descent() + 2.0 * pad_y);
		let r = bh / 2.0;
		let mut pb = PathBuilder::new();
		// None off at infinity (a point far off the page in a hand-edited file).
		if let Some(rect) = tiny_skia::Rect::from_xywh(x + r.min(bw / 2.0), y, (bw - 2.0 * r).max(0.1), bh) {
			pb.push_rect(rect);
		}
		pb.push_circle(x + r, y + r, r);
		pb.push_circle(x + bw - r, y + r, r);
		if let Some(p) = pb.finish() {
			self.pix.fill_path(&p, &paint(bg, 0.95), FillRule::Winding, Transform::identity(), None);
		}
		self.text(x + pad_x, y + pad_y + f.ascent(), size, text, fg);
		(bw, bh)
	}

	/// Raw glyphs; `baseline_y` is the baseline.
	fn text(&mut self, x: f32, baseline_y: f32, size: f32, text: &str, rgb: (u8, u8, u8)) {
		let f = font().as_scaled(PxScale::from(size));
		let mut pen = x;
		let (w, h) = (self.pix.width() as i32, self.pix.height() as i32);
		let data = self.pix.data_mut();
		for c in text.chars() {
			let id = f.glyph_id(c);
			let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, baseline_y));
			pen += f.h_advance(id);
			if let Some(outlined) = font().outline_glyph(glyph) {
				let b = outlined.px_bounds();
				outlined.draw(|gx, gy, cov| {
					let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
					if px < 0 || py < 0 || px >= w || py >= h || cov <= 0.0 {
						return;
					}
					let i = ((py * w + px) * 4) as usize;
					for (k, ch) in [rgb.0, rgb.1, rgb.2].iter().enumerate() {
						let dst = data[i + k] as f32;
						data[i + k] = (dst + (*ch as f32 - dst) * cov.min(1.0)) as u8;
					}
					data[i + 3] = 255;
				});
			}
		}
	}

	fn text_width(size: f32, text: &str) -> f32 {
		let f = font().as_scaled(PxScale::from(size));
		text.chars().map(|c| f.h_advance(f.glyph_id(c))).sum()
	}
}

fn hatch(c: &mut Canvas, poly: &tiny_skia::Path, rgb: (u8, u8, u8), kind: Hatch, alpha: f32) {
	let mut mask = tiny_skia::Mask::new(c.pix.width(), c.pix.height()).unwrap();
	mask.fill_path(poly, FillRule::EvenOdd, true, Transform::identity());
	let b = poly.bounds();
	let gap = 8.0 * c.unit();
	let mut pb = PathBuilder::new();
	let span = (b.width() + b.height()).max(1.0);
	match kind {
		Hatch::Diagonal | Hatch::Cross => {
			let mut t = -span;
			while t < span {
				pb.move_to(b.left() + t, b.bottom());
				pb.line_to(b.left() + t + b.height(), b.top());
				if kind == Hatch::Cross {
					pb.move_to(b.left() + t, b.top());
					pb.line_to(b.left() + t + b.height(), b.bottom());
				}
				t += gap;
			}
		}
		Hatch::Grid => {
			let mut x = b.left();
			while x < b.right() {
				pb.move_to(x, b.top());
				pb.line_to(x, b.bottom());
				x += gap;
			}
			let mut y = b.top();
			while y < b.bottom() {
				pb.move_to(b.left(), y);
				pb.line_to(b.right(), y);
				y += gap;
			}
		}
		Hatch::Dots => {
			let mut y = b.top();
			while y < b.bottom() {
				let mut x = b.left();
				while x < b.right() {
					pb.push_circle(x, y, c.unit() * 0.8);
					x += gap;
				}
				y += gap;
			}
		}
	}
	if let Some(p) = pb.finish() {
		let s = Stroke { width: c.unit() * 0.8, ..Default::default() };
		if kind == Hatch::Dots {
			c.pix.fill_path(&p, &paint(rgb, alpha), FillRule::Winding, Transform::identity(), Some(&mask));
		} else {
			c.pix.stroke_path(&p, &paint(rgb, alpha), &s, Transform::identity(), Some(&mask));
		}
	}
}

fn dash_for(style: Option<LineStyle>) -> Option<&'static [f32]> {
	match style {
		Some(LineStyle::Dashed) => Some(&[4.0, 3.0]),
		Some(LineStyle::Dotted) => Some(&[0.1, 2.5]),
		_ => None,
	}
}

/// Draw the takeoff for `page` onto `img`, which shows `region` of the page.
pub fn burn(img: &mut RgbaImage, region: FracRect, page_size_pt: (f64, f64), data: &TakeoffData, page: u32, opts: &BurnOptions) {
	if opts.overlays {
		crate::overlay::composite(img, region, data, page);
	}
	let (w, h) = img.dimensions();
	let Some(mut pix) = Pixmap::new(w, h) else { return };
	pix.data_mut().copy_from_slice(img.as_raw());
	let mut c = Canvas { pix, region, opts_scale: opts.scale };
	let u = c.unit();
	let cats: HashMap<&str, &Category> = data.categories.iter().map(|c| (c.id.as_str(), c)).collect();
	let wanted = |cid: &Option<String>| match &opts.categories {
		Some(set) => cid.as_ref().is_some_and(|c| set.contains(c)),
		None => !cid.as_deref().and_then(|c| cats.get(c)).is_some_and(|c| c.hidden),
	};
	let dim = |id: &str| if opts.highlight.is_empty() || opts.highlight.contains(id) { 1.0 } else { 0.35 };
	let disp = &data.display;

	if opts.annotations {
		for a in data.annotations.iter().filter(|a| a.page == page) {
			draw_annotation(&mut c, a, dim(&a.id));
		}
	}

	if opts.measurements {
		let seq = measurement_seq(&data.measurements);
		for m in data.measurements.iter().filter(|m| m.page == page && wanted(&m.category_id)) {
			let cat = m.category_id.as_deref().and_then(|id| cats.get(id));
			let rgb = parse_color(m.color.as_deref().or(cat.map(|c| c.color.as_str())).unwrap_or(if m.kind == MeasurementType::Area { "#A78BFA" } else { "#22D3EE" }));
			let a = dim(&m.id);
			let width = if opts.highlight.contains(&m.id) { 3.0 * u } else { 2.0 * u };
			let closed = m.kind == MeasurementType::Area || m.closed;
			let Some(path) = c.path(&m.points, closed) else { continue };
			if m.kind == MeasurementType::Area {
				let mut pb = PathBuilder::new();
				let mut push = |pts: &[Point]| {
					if let Some((first, rest)) = pts.split_first() {
						let (x, y) = c.to_px(*first);
						pb.move_to(x, y);
						for p in rest {
							let (x, y) = c.to_px(*p);
							pb.line_to(x, y);
						}
						pb.close();
					}
				};
				push(&m.points);
				for cut in &m.cutouts {
					push(cut);
				}
				if let Some(fill) = pb.finish() {
					c.fill(&fill, rgb, 0.18 * cat.and_then(|c| c.opacity).unwrap_or(1.0) as f32 * a);
					if let Some(hk) = cat.and_then(|c| c.hatch) {
						hatch(&mut c, &fill, rgb, hk, 0.55 * a);
					}
				}
				for cut in &m.cutouts {
					if let Some(p) = c.path(cut, true) {
						c.stroke(&p, rgb, a, u, Some(&[3.0, 2.0]));
					}
				}
			}
			c.stroke(&path, rgb, a, width, dash_for(cat.and_then(|c| c.line_style)));
			for p in &m.points {
				let (x, y) = c.to_px(*p);
				if let Some(dot) = PathBuilder::from_circle(x, y, 1.6 * u) {
					c.fill(&dot, rgb, a);
				}
			}
			if opts.badges {
				let n = seq.get(&m.id).copied().unwrap_or(0);
				let value = match (m.value, &m.unit) {
					(Some(v), Some(unit)) => format_quantity(v, unit, disp),
					_ if m.kind == MeasurementType::Angle => String::new(),
					_ => "no scale".into(),
				};
				let text = if value.is_empty() { format!("{n}") } else { format!("{n} · {value}") };
				let (x, y) = c.to_px(m.points[0]);
				c.label(x + 2.0 * u, y - 9.0 * u, 7.0 * u, &text, (255, 255, 255), rgb);
			}
		}
	}

	if opts.marks {
		let seq = mark_seq(&data.marks);
		for m in data.marks.iter().filter(|m| m.page == page && wanted(&m.category_id)) {
			let cat = m.category_id.as_deref().and_then(|id| cats.get(id));
			let rgb = parse_color(cat.map(|c| c.color.as_str()).unwrap_or("#22D3EE"));
			let a = dim(&m.id);
			let (x0, y0) = c.to_px(Point { x: m.x, y: m.y });
			let (x1, y1) = c.to_px(Point { x: m.x + m.w, y: m.y + m.h });
			let Some(r) = c.rect_path(x0, y0, x1, y1) else { continue };
			c.fill(&r, rgb, 0.16 * a * cat.and_then(|c| c.opacity).unwrap_or(1.0) as f32);
			let pending = m.status == MarkStatus::Pending;
			c.stroke(&r, rgb, a, if opts.highlight.contains(&m.id) { 3.0 * u } else { 1.8 * u }, if pending { Some(&[2.0, 1.5]) } else { None });
			if opts.badges {
				let n = seq.get(&m.id).copied().unwrap_or(0).to_string();
				let size = 7.0 * u;
				let bw = Canvas::text_width(size, &n) + size * 0.7;
				c.label(x0 - bw * 0.35, y0 - size * 1.15, size, &n, (255, 255, 255), rgb);
			}
		}
	}

	if opts.annotations {
		for l in data.legends.iter().filter(|l| l.page == page) {
			draw_legend(&mut c, l, data);
		}
	}

	if opts.scales {
		for s in data.page_scales.iter().filter(|s| s.page == page) {
			if s.p1 == (Point { x: 0.0, y: 0.0 }) && s.p2.y == 0.0 {
				continue; // preset scales have no drawn reference
			}
			if let Some(p) = c.path(&[s.p1, s.p2], false) {
				c.stroke(&p, (251, 191, 36), 0.95, 2.0 * u, Some(&[3.0, 2.0]));
				let (x, y) = c.to_px(s.p1);
				c.label(x, y + 4.0 * u, 7.0 * u, &format!("scale {} {}", trim_num(s.distance), s.unit), (30, 30, 30), (251, 191, 36));
			}
		}
		for v in data.viewports.iter().filter(|v| v.page == page) {
			let (x0, y0) = c.to_px(Point { x: v.x, y: v.y });
			let (x1, y1) = c.to_px(Point { x: v.x + v.w, y: v.y + v.h });
			if let Some(r) = c.rect_path(x0, y0, x1, y1) {
				c.stroke(&r, (251, 191, 36), 0.8, 1.2 * u, Some(&[5.0, 3.0]));
				c.label(x0 + 3.0 * u, y0 + 3.0 * u, 6.5 * u, &format!("viewport: {}", v.name), (30, 30, 30), (251, 191, 36));
			}
		}
	}
	let _ = page_size_pt;
	img.copy_from_slice(c.pix.data());
}

fn trim_num(v: f64) -> String {
	let s = format!("{v:.3}");
	s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn draw_annotation(c: &mut Canvas, a: &Annotation, dim: f32) {
	let rgb = parse_color(&a.style.stroke);
	let alpha = a.style.opacity as f32 * dim;
	// Stroke width is in PDF points; convert with the render's px-per-point.
	let u = c.unit();
	let width = (a.style.width as f32 * u * 0.9).max(1.0);
	let dash = dash_for(a.style.line_style);
	let fill = a.style.fill.as_deref().map(parse_color);
	let pts = &a.points;
	match a.kind {
		AnnotationKind::Rectangle | AnnotationKind::Highlight | AnnotationKind::Text | AnnotationKind::Stamp | AnnotationKind::Ellipse | AnnotationKind::Callout => {
			if pts.len() < 2 {
				return;
			}
			let (x0, y0) = c.to_px(pts[0]);
			let (x1, y1) = c.to_px(pts[1]);
			let (l, t, r, b) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
			let shape = if a.kind == AnnotationKind::Ellipse {
				tiny_skia::Rect::from_ltrb(l, t, r.max(l + 1.0), b.max(t + 1.0)).and_then(PathBuilder::from_oval)
			} else {
				c.rect_path(l, t, r, b)
			};
			let Some(shape) = shape else { return };
			match a.kind {
				AnnotationKind::Highlight => c.fill(&shape, rgb, 0.35 * dim),
				_ => {
					if let Some(f) = fill {
						c.fill(&shape, f, 0.85 * alpha);
					} else if matches!(a.kind, AnnotationKind::Text | AnnotationKind::Callout) {
						c.fill(&shape, (255, 255, 255), 0.85 * alpha);
					}
					if a.kind != AnnotationKind::Text || a.style.width > 0.0 {
						c.stroke(&shape, rgb, alpha, width, dash);
					}
				}
			}
			if a.kind == AnnotationKind::Callout && pts.len() >= 3 {
				let (tx, ty) = c.to_px(pts[2]);
				let (cx, cy) = (tx.clamp(l, r), ty.clamp(t, b));
				let mut pb = PathBuilder::new();
				pb.move_to(cx, cy);
				pb.line_to(tx, ty);
				if let Some(p) = pb.finish() {
					c.stroke(&p, rgb, alpha, width, None);
				}
				arrow_head(c, (cx, cy), (tx, ty), rgb, alpha, width);
			}
			if let Some(text) = a.text.as_deref().filter(|t| !t.is_empty()) {
				let size = (a.style.font_size as f32 * u * 0.9).max(6.0);
				let color = if a.kind == AnnotationKind::Stamp { rgb } else { (20, 20, 20) };
				let mut y = t + size * 1.1;
				for line in wrap(text, size, (r - l - size * 0.6).max(size)) {
					if y > b + size * 0.2 {
						break;
					}
					c.text(l + size * 0.3, y, size, &line, color);
					y += size * 1.25;
				}
			}
		}
		AnnotationKind::Line | AnnotationKind::Arrow | AnnotationKind::Polyline | AnnotationKind::Ink => {
			let Some(p) = c.path(pts, false) else { return };
			c.stroke(&p, rgb, alpha, width, dash);
			if a.kind == AnnotationKind::Arrow && pts.len() >= 2 {
				let (x0, y0) = c.to_px(pts[pts.len() - 2]);
				let (x1, y1) = c.to_px(pts[pts.len() - 1]);
				arrow_head(c, (x0, y0), (x1, y1), rgb, alpha, width);
			}
		}
		AnnotationKind::Polygon => {
			let Some(p) = c.path(pts, true) else { return };
			if let Some(f) = fill {
				c.fill(&p, f, 0.5 * alpha);
			}
			c.stroke(&p, rgb, alpha, width, dash);
		}
		AnnotationKind::Cloud => {
			let Some(p) = cloud_path(c, pts) else { return };
			if let Some(f) = fill {
				c.fill(&p, f, 0.4 * alpha);
			}
			c.stroke(&p, rgb, alpha, width, None);
		}
	}
}

fn arrow_head(c: &mut Canvas, from: (f32, f32), to: (f32, f32), rgb: (u8, u8, u8), alpha: f32, width: f32) {
	let (dx, dy) = (to.0 - from.0, to.1 - from.1);
	let len = (dx * dx + dy * dy).sqrt().max(0.001);
	let (ux, uy) = (dx / len, dy / len);
	let size = (width * 4.0).max(8.0);
	let mut pb = PathBuilder::new();
	pb.move_to(to.0, to.1);
	pb.line_to(to.0 - ux * size - uy * size * 0.5, to.1 - uy * size + ux * size * 0.5);
	pb.line_to(to.0 - ux * size + uy * size * 0.5, to.1 - uy * size - ux * size * 0.5);
	pb.close();
	if let Some(p) = pb.finish() {
		c.fill(&p, rgb, alpha);
	}
}

/// Scalloped revision cloud along a polygon (or a rectangle given by 2 corners).
fn cloud_path(c: &Canvas, pts: &[Point]) -> Option<tiny_skia::Path> {
	let poly: Vec<(f32, f32)> = if pts.len() == 2 {
		let (a, b) = (c.to_px(pts[0]), c.to_px(pts[1]));
		vec![(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)]
	} else {
		pts.iter().map(|p| c.to_px(*p)).collect()
	};
	if poly.len() < 3 {
		return None;
	}
	// Scallops bulge outward: orient using the polygon's signed area.
	let area: f32 = (0..poly.len()).map(|i| {
		let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
		a.0 * b.1 - b.0 * a.1
	}).sum();
	let sign = if area >= 0.0 { -1.0 } else { 1.0 };
	let arc = 10.0 * c.unit();
	let mut pb = PathBuilder::new();
	pb.move_to(poly[0].0, poly[0].1);
	for i in 0..poly.len() {
		let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
		let (dx, dy) = (b.0 - a.0, b.1 - a.1);
		let len = (dx * dx + dy * dy).sqrt();
		let n = (len / arc).ceil().clamp(1.0, 10_000.0) as usize;
		let (nx, ny) = (-dy / len * sign, dx / len * sign);
		for k in 0..n {
			let t0 = k as f32 / n as f32;
			let t1 = (k + 1) as f32 / n as f32;
			let tm = (t0 + t1) / 2.0;
			let bulge = len / n as f32 * 0.6;
			pb.quad_to(a.0 + dx * tm + nx * bulge, a.1 + dy * tm + ny * bulge, a.0 + dx * t1, a.1 + dy * t1);
		}
	}
	pb.close();
	pb.finish()
}

fn wrap(text: &str, size: f32, max_w: f32) -> Vec<String> {
	let mut lines = Vec::new();
	for para in text.lines() {
		let mut line = String::new();
		for word in para.split_whitespace() {
			let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
			if Canvas::text_width(size, &cand) > max_w && !line.is_empty() {
				lines.push(std::mem::take(&mut line));
				line = word.to_string();
			} else {
				line = cand;
			}
		}
		lines.push(line);
	}
	lines
}

/// A legend box: frame, title, header, and a swatch, name and totals per row.
fn draw_legend(c: &mut Canvas, l: &Legend, data: &TakeoffData) {
	use takeoff_core::legend::{hatch_lines, layout, rows_for, Swatch};
	// The page's shape as rendered (height / width).
	let aspect = (c.pix.height() as f64 / c.region.h) / (c.pix.width() as f64 / c.region.w);
	let rows = rows_for(data, l);
	let lay = layout(l, &rows, aspect);
	let u = c.unit();
	let ink = (40, 40, 40);
	let (x0, y0) = c.to_px(Point { x: l.x, y: l.y });
	let (x1, y1) = c.to_px(Point { x: l.x + l.w, y: l.y + l.h });
	let Some(frame) = c.rect_path(x0, y0, x1, y1) else { return };
	c.fill(&frame, (255, 255, 255), 0.94);
	c.stroke(&frame, ink, 0.9, u, None);
	for (a, b) in &lay.rules {
		if let Some(p) = c.path(&[*a, *b], false) {
			c.stroke(&p, (150, 150, 150), 0.9, u * 0.6, None);
		}
	}
	for ((x, y, w, h), k) in &lay.swatches {
		let row = &rows[*k];
		let rgb = parse_color(&row.color);
		let (sx0, sy0) = c.to_px(Point { x: *x, y: *y });
		let (sx1, sy1) = c.to_px(Point { x: x + w, y: y + h });
		let Some(r) = c.rect_path(sx0, sy0, sx1, sy1) else { continue };
		match row.swatch {
			Swatch::Count => {
				c.fill(&r, rgb, 0.18 * row.opacity as f32);
				c.stroke(&r, rgb, 1.0, 1.5 * u, None);
			}
			Swatch::Length(style) => {
				let mid = y + h / 2.0;
				if let Some(p) = c.path(&[Point { x: *x, y: mid }, Point { x: x + w, y: mid }], false) {
					c.stroke(&p, rgb, 1.0, 2.0 * u, dash_for(style));
				}
			}
			Swatch::Area(hk) => {
				c.fill(&r, rgb, 0.25 * row.opacity as f32);
				for (a, b) in hk.map(|hk| hatch_lines((*x, *y, *w, *h), hk, aspect, 6)).unwrap_or_default() {
					if let Some(p) = c.path(&[a, b], false) {
						c.stroke(&p, rgb, 0.7, 0.8 * u, None);
					}
				}
				c.stroke(&r, rgb, 1.0, 1.2 * u, None);
			}
			Swatch::Markup(_) => c.stroke(&r, rgb, row.opacity as f32, 1.5 * u, None),
		}
	}
	for t in &lay.texts {
		let size = (t.size / c.region.h * c.pix.height() as f64) as f32;
		if size < 2.0 {
			continue;
		}
		let (mut x, y) = c.to_px(t.at);
		if t.right {
			x -= Canvas::text_width(size, &t.text);
		}
		c.text(x, y + size * 0.8, size, &t.text, if t.muted { (110, 110, 110) } else { ink });
	}
}

/// Draw arbitrary segments (snap geometry debug / agent "show lines").
pub fn burn_segments(img: &mut RgbaImage, region: FracRect, segments: &[takeoff_core::snap::Segment], rgb: (u8, u8, u8)) {
	let (w, h) = img.dimensions();
	let Some(mut pix) = Pixmap::new(w, h) else { return };
	pix.data_mut().copy_from_slice(img.as_raw());
	let mut c = Canvas { pix, region, opts_scale: 1.0 };
	let mut pb = PathBuilder::new();
	for s in segments {
		let (a, b) = (c.to_px(s.a), c.to_px(s.b));
		pb.move_to(a.0, a.1);
		pb.line_to(b.0, b.1);
	}
	if let Some(p) = pb.finish() {
		c.stroke(&p, rgb, 0.8, 1.0, None);
	}
	img.copy_from_slice(c.pix.data());
}

/// Render an SVG (agent overlay, viewBox 0 0 1000 1000 over the page) onto `img` showing `region`.
pub fn burn_svg(img: &mut RgbaImage, region: FracRect, svg: &str) -> Result<(), String> {
	let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
	let (w, h) = img.dimensions();
	let mut pix = Pixmap::new(w, h).ok_or("empty image")?;
	pix.data_mut().copy_from_slice(img.as_raw());
	let size = tree.size();
	// SVG units → page fractions → this image's px.
	let sx = w as f32 / (region.w as f32 * size.width());
	let sy = h as f32 / (region.h as f32 * size.height());
	let t = Transform::from_row(sx, 0.0, 0.0, sy, -(region.x as f32) * size.width() * sx, -(region.y as f32) * size.height() * sy);
	resvg::render(&tree, t, &mut pix.as_mut());
	img.copy_from_slice(pix.data());
	Ok(())
}

/// A box (x, y, w, h in page fractions), its label and its colour.
pub type LabelledBox = ((f64, f64, f64, f64), String, (u8, u8, u8));

/// Outlined boxes (page fractions within `region`), each with an optional
/// label pill at its top-left: the map tools' check renders.
pub fn burn_boxes(img: &mut RgbaImage, region: FracRect, boxes: &[LabelledBox]) {
	let (w, h) = img.dimensions();
	let Some(pix) = Pixmap::new(w, h) else { return };
	let mut c = Canvas { pix, region, opts_scale: 1.0 };
	c.pix.data_mut().copy_from_slice(img.as_raw());
	let size = (w.max(h) as f32 / 70.0).clamp(11.0, 22.0);
	for ((x, y, bw, bh), label, rgb) in boxes {
		let (x0, y0) = c.to_px(Point { x: *x, y: *y });
		let (x1, y1) = c.to_px(Point { x: x + bw, y: y + bh });
		if let Some(p) = c.rect_path(x0, y0, x1, y1) {
			c.stroke(&p, *rgb, 0.95, (size / 5.0).max(2.0), None);
		}
		if !label.is_empty() {
			c.label(x0, (y0 - size * 1.4).max(0.0), size, label, (255, 255, 255), *rgb);
		}
	}
	img.copy_from_slice(c.pix.data());
}

/// A text pill at image pixel (x, y), top-left anchored.
pub fn burn_label(img: &mut RgbaImage, x: f32, y: f32, size: f32, text: &str, fg: (u8, u8, u8), bg: (u8, u8, u8)) {
	let (w, h) = img.dimensions();
	let Some(pix) = Pixmap::new(w, h) else { return };
	let mut c = Canvas { pix, region: FracRect::FULL, opts_scale: 1.0 };
	c.pix.data_mut().copy_from_slice(img.as_raw());
	c.label(x, y, size, text, fg, bg);
	img.copy_from_slice(c.pix.data());
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn odd_colours_and_far_off_labels_dont_panic() {
		assert_eq!(parse_color("#é1"), (255, 255, 17));
		assert_eq!(parse_color("#f80"), (255, 136, 0));
		let mut img = RgbaImage::new(64, 64);
		burn_label(&mut img, f32::INFINITY, 0.0, 12.0, "7", (255, 255, 255), (0, 0, 0));
		burn_label(&mut img, f32::NAN, f32::NAN, 12.0, "7", (255, 255, 255), (0, 0, 0));
	}
}
