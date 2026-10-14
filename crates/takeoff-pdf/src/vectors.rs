//! The drawing's own line work, as straight segments in page fractions, for
//! snapping (see `takeoff_core::snap`). Curves are flattened. Filled-only
//! paths (hatches, solid fills, outlined glyphs) are skipped: you snap to
//! lines, not to the inside of a poché.

use pdfium_render::prelude::*;
use takeoff_core::snap::Segment;
use takeoff_core::Point;

/// Maps PDF user space (points, bottom-left origin) to page fractions as
/// displayed, rotation and crop box included. Found by asking pdfium where
/// three points land, so it agrees with the renderer exactly.
#[derive(Debug, Clone, Copy)]
pub struct PageAffine {
	a: f64,
	b: f64,
	c: f64,
	d: f64,
	e: f64,
	f: f64,
}

impl PageAffine {
	pub fn for_page(page: &PdfPage) -> Result<Self, PdfiumError> {
		const W: i32 = 1_000_000;
		let cfg = PdfRenderConfig::new().set_fixed_size(W, W);
		let at = |x: f32, y: f32| -> Result<(f64, f64), PdfiumError> {
			let (px, py) = page.points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &cfg)?;
			Ok((px as f64 / W as f64, py as f64 / W as f64))
		};
		let o = at(0.0, 0.0)?;
		let xu = at(1000.0, 0.0)?;
		let yu = at(0.0, 1000.0)?;
		Ok(Self { a: (xu.0 - o.0) / 1000.0, b: (xu.1 - o.1) / 1000.0, c: (yu.0 - o.0) / 1000.0, d: (yu.1 - o.1) / 1000.0, e: o.0, f: o.1 })
	}

	pub fn apply(&self, x: f64, y: f64) -> Point {
		Point { x: self.a * x + self.c * y + self.e, y: self.b * x + self.d * y + self.f }
	}
}

const MAX_SEGMENTS: usize = 2_000_000;

/// All stroked line work on the page.
pub fn page_segments(page: &PdfPage) -> Result<Vec<Segment>, PdfiumError> {
	let affine = PageAffine::for_page(page)?;
	let mut out = Vec::new();
	for obj in page.objects().iter() {
		collect(&obj, None, &affine, &mut out, 0);
		if out.len() >= MAX_SEGMENTS {
			break;
		}
	}
	Ok(out)
}

fn collect(obj: &PdfPageObject, parent: Option<PdfMatrix>, affine: &PageAffine, out: &mut Vec<Segment>, depth: u32) {
	match obj.object_type() {
		PdfPageObjectType::Path => {
			let Some(path) = obj.as_path_object() else { return };
			if !path.is_stroked().unwrap_or(true) {
				return;
			}
			let Ok(m) = path.matrix() else { return };
			let m = match parent {
				Some(p) => m.multiply(p),
				None => m,
			};
			let mut start: Option<(f64, f64)> = None;
			let mut last: Option<(f64, f64)> = None;
			let mut bez: Vec<(f64, f64)> = Vec::new();
			let segs = path.segments();
			for s in segs.raw().iter() {
				let (x, y) = (s.x().value as f64, s.y().value as f64);
				let (tx, ty) = apply(&m, x, y);
				match s.segment_type() {
					PdfPathSegmentType::MoveTo => {
						start = Some((tx, ty));
						last = Some((tx, ty));
						bez.clear();
					}
					PdfPathSegmentType::LineTo => {
						if let Some(l) = last {
							push(out, affine, l, (tx, ty));
						}
						last = Some((tx, ty));
					}
					PdfPathSegmentType::BezierTo => {
						bez.push((tx, ty));
						if bez.len() == 3 {
							if let Some(l) = last {
								flatten_cubic(out, affine, l, bez[0], bez[1], bez[2]);
							}
							last = Some(bez[2]);
							bez.clear();
						}
					}
					PdfPathSegmentType::Unknown => {}
				}
				if s.is_close() {
					if let (Some(l), Some(st)) = (last, start) {
						push(out, affine, l, st);
						last = Some(st);
					}
				}
			}
		}
		PdfPageObjectType::XObjectForm if depth < 8 => {
			let Some(form) = obj.as_x_object_form_object() else { return };
			let Ok(m) = form.matrix() else { return };
			let m = match parent {
				Some(p) => m.multiply(p),
				None => m,
			};
			for child in form.iter() {
				collect(&child, Some(m), affine, out, depth + 1);
				if out.len() >= MAX_SEGMENTS {
					return;
				}
			}
		}
		_ => {}
	}
}

fn apply(m: &PdfMatrix, x: f64, y: f64) -> (f64, f64) {
	let (a, b, c, d, e, f) = (m.a() as f64, m.b() as f64, m.c() as f64, m.d() as f64, m.e() as f64, m.f() as f64);
	(a * x + c * y + e, b * x + d * y + f)
}

fn push(out: &mut Vec<Segment>, affine: &PageAffine, a: (f64, f64), b: (f64, f64)) {
	let (pa, pb) = (affine.apply(a.0, a.1), affine.apply(b.0, b.1));
	if pa != pb {
		out.push(Segment { a: pa, b: pb });
	}
}

fn flatten_cubic(out: &mut Vec<Segment>, affine: &PageAffine, p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64)) {
	// Enough steps that chords stay within ~0.25 pt of the curve.
	let len = dist(p0, p1) + dist(p1, p2) + dist(p2, p3);
	let steps = ((len / 4.0).ceil() as usize).clamp(2, 64);
	let mut prev = p0;
	for i in 1..=steps {
		let t = i as f64 / steps as f64;
		let mt = 1.0 - t;
		let x = mt * mt * mt * p0.0 + 3.0 * mt * mt * t * p1.0 + 3.0 * mt * t * t * p2.0 + t * t * t * p3.0;
		let y = mt * mt * mt * p0.1 + 3.0 * mt * mt * t * p1.1 + 3.0 * mt * t * t * p2.1 + t * t * t * p3.1;
		push(out, affine, prev, (x, y));
		prev = (x, y);
	}
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
	(a.0 - b.0).hypot(a.1 - b.1)
}
