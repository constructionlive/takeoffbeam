//! Slip-sheeting: a newer revision of a sheet takes its page, and the takeoff
//! stays on it. Positions are page fractions, so a revision on the same paper
//! keeps every item exactly where it was. When the paper changes shape, the
//! old sheet is taken as drawn to fit the new one (scaled evenly, centred),
//! which is how a reissue on other paper normally looks.

use crate::model::*;

/// Where an old page's fractions land on a new page: `(sx, sy, ox, oy)`, new = old × s + o.
pub fn fit_transform(from_pt: (f64, f64), to_pt: (f64, f64)) -> (f64, f64, f64, f64) {
	let (w0, h0) = (from_pt.0.max(1e-6), from_pt.1.max(1e-6));
	let (w1, h1) = (to_pt.0.max(1e-6), to_pt.1.max(1e-6));
	let s = (w1 / w0).min(h1 / h0);
	let (sx, sy) = (w0 * s / w1, h0 * s / h1);
	(sx, sy, (1.0 - sx) / 2.0, (1.0 - sy) / 2.0)
}

/// Whether two page sizes have a different shape (more than 0.5%).
pub fn aspect_differs(a: (f64, f64), b: (f64, f64)) -> bool {
	let (ra, rb) = (a.0 / a.1.max(1e-6), b.0 / b.1.max(1e-6));
	(ra / rb - 1.0).abs() > 0.005
}

/// Move one page's items from an old page of `from_pt` onto a new one of
/// `to_pt` (see [`fit_transform`]). Scales follow, so lengths stay true.
pub fn carry_page(d: &mut TakeoffData, page: u32, from_pt: (f64, f64), to_pt: (f64, f64)) {
	let (sx, sy, ox, oy) = fit_transform(from_pt, to_pt);
	if (sx - 1.0).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9 {
		return;
	}
	let tp = |p: &mut Point| {
		p.x = p.x * sx + ox;
		p.y = p.y * sy + oy;
	};
	for m in d.marks.iter_mut().filter(|m| m.page == page) {
		(m.x, m.y, m.w, m.h) = (m.x * sx + ox, m.y * sy + oy, m.w * sx, m.h * sy);
	}
	for m in d.measurements.iter_mut().filter(|m| m.page == page) {
		m.points.iter_mut().for_each(tp);
		m.cutouts.iter_mut().flatten().for_each(tp);
	}
	for a in d.annotations.iter_mut().filter(|a| a.page == page) {
		a.points.iter_mut().for_each(tp);
	}
	for o in d.overlays.iter_mut().filter(|o| o.page == page) {
		(o.x, o.y, o.w, o.h) = (o.x * sx + ox, o.y * sy + oy, o.w * sx, o.h * sy);
	}
	for s in d.page_scales.iter_mut().filter(|s| s.page == page) {
		tp(&mut s.p1);
		tp(&mut s.p2);
	}
	for v in d.viewports.iter_mut().filter(|v| v.page == page) {
		(v.x, v.y, v.w, v.h) = (v.x * sx + ox, v.y * sy + oy, v.w * sx, v.h * sy);
		tp(&mut v.scale.p1);
		tp(&mut v.scale.p2);
	}
	for c in &mut d.categories {
		if let Some(r) = c.legend_region.as_mut().filter(|r| r.page == page) {
			(r.x, r.y, r.w, r.h) = (r.x * sx + ox, r.y * sy + oy, r.w * sx, r.h * sy);
		}
	}
}

/// Which parts of a sheet changed between two revisions: a coarse grid over
/// the page, true where the drawings differ.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeGrid {
	pub cols: usize,
	pub rows: usize,
	pub cells: Vec<bool>,
}

impl ChangeGrid {
	pub fn new(cols: usize, rows: usize) -> Self {
		Self { cols: cols.max(1), rows: rows.max(1), cells: vec![false; cols.max(1) * rows.max(1)] }
	}

	pub fn set(&mut self, col: usize, row: usize) {
		if col < self.cols && row < self.rows {
			self.cells[row * self.cols + col] = true;
		}
	}

	pub fn get(&self, col: usize, row: usize) -> bool {
		col < self.cols && row < self.rows && self.cells[row * self.cols + col]
	}

	/// Grow every changed cell by one in each direction (items drawn just
	/// beside a change are suspect too).
	pub fn dilate(&self) -> Self {
		let mut out = self.clone();
		for r in 0..self.rows {
			for c in 0..self.cols {
				if self.get(c, r) {
					for (dc, dr) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
						let (nc, nr) = (c as i64 + dc, r as i64 + dr);
						if nc >= 0 && nr >= 0 {
							out.set(nc as usize, nr as usize);
						}
					}
				}
			}
		}
		out
	}

	pub fn changed_at(&self, p: Point) -> bool {
		let c = (p.x.clamp(0.0, 0.999_999) * self.cols as f64) as usize;
		let r = (p.y.clamp(0.0, 0.999_999) * self.rows as f64) as usize;
		self.get(c, r)
	}

	/// Any changed cell under a box (fractions).
	pub fn changed_in(&self, x: f64, y: f64, w: f64, h: f64) -> bool {
		let c0 = (x.clamp(0.0, 1.0) * self.cols as f64).floor() as usize;
		let r0 = (y.clamp(0.0, 1.0) * self.rows as f64).floor() as usize;
		let c1 = (((x + w).clamp(0.0, 1.0)) * self.cols as f64).ceil() as usize;
		let r1 = (((y + h).clamp(0.0, 1.0)) * self.rows as f64).ceil() as usize;
		(r0..r1.max(r0 + 1)).any(|r| (c0..c1.max(c0 + 1)).any(|c| self.get(c, r)))
	}

	/// Any changed cell along a polyline (sampled every half cell).
	pub fn changed_along(&self, pts: &[Point], closed: bool) -> bool {
		if pts.len() == 1 {
			return self.changed_at(pts[0]);
		}
		let step = 0.5 / self.cols.max(self.rows) as f64;
		let n = pts.len();
		let segs = if closed && n > 2 { n } else { n - 1 };
		(0..segs).any(|i| {
			let (a, b) = (pts[i], pts[(i + 1) % n]);
			let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
			let k = (len / step).ceil().max(1.0) as usize;
			(0..=k).any(|j| {
				let t = j as f64 / k as f64;
				self.changed_at(Point { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t })
			})
		})
	}
}

/// The ids of a page's takeoff items and markups that touch changed parts of
/// the sheet: after a new revision, these are the ones to check.
pub fn items_on_changes(d: &TakeoffData, page: u32, grid: &ChangeGrid) -> Vec<String> {
	let mut out = Vec::new();
	for m in d.marks.iter().filter(|m| m.page == page) {
		if grid.changed_in(m.x, m.y, m.w, m.h) {
			out.push(m.id.clone());
		}
	}
	for m in d.measurements.iter().filter(|m| m.page == page) {
		let closed = m.closed || m.kind == MeasurementType::Area;
		if grid.changed_along(&m.points, closed) || m.cutouts.iter().any(|c| grid.changed_along(c, true)) {
			out.push(m.id.clone());
		}
	}
	for a in d.annotations.iter().filter(|a| a.page == page) {
		if grid.changed_along(&a.points, matches!(a.kind, AnnotationKind::Polygon | AnnotationKind::Cloud)) {
			out.push(a.id.clone());
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn mark(page: u32, x: f64, y: f64) -> Mark {
		Mark { id: new_id("m"), category_id: None, page, x, y, w: 0.02, h: 0.02, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() }
	}

	#[test]
	fn same_paper_keeps_every_position() {
		let mut d = TakeoffData::default();
		d.marks = vec![mark(1, 0.3, 0.4)];
		let before = d.clone();
		carry_page(&mut d, 1, (2592.0, 1728.0), (2592.0, 1728.0));
		assert_eq!(d, before);
		// Half-size paper of the same shape: fractions don't change either.
		carry_page(&mut d, 1, (2592.0, 1728.0), (1296.0, 864.0));
		assert_eq!(d, before);
	}

	#[test]
	fn other_paper_shape_fits_and_keeps_lengths() {
		// 36×24 → 42×24: the old sheet sits centred, pillarboxed.
		let (old, new) = ((2592.0, 1728.0), (3024.0, 1728.0));
		assert!(aspect_differs(old, new));
		let mut d = TakeoffData::default();
		d.marks = vec![mark(1, 0.0, 0.0), mark(2, 0.5, 0.5)];
		d.page_scales.push(PageScale { page: 1, p1: Point { x: 0.1, y: 0.5 }, p2: Point { x: 0.2, y: 0.5 }, distance: 10.0, unit: "ft".into() });
		d.measurements.push(Measurement::new("ms", 1, MeasurementType::Length, vec![Point { x: 0.1, y: 0.2 }, Point { x: 0.4, y: 0.2 }]));
		let len_pt = |d: &TakeoffData, size: (f64, f64)| (d.measurements[0].points[1].x - d.measurements[0].points[0].x) * size.0;
		let scale_pt = |d: &TakeoffData, size: (f64, f64)| (d.page_scales[0].p2.x - d.page_scales[0].p1.x) * size.0;
		let ratio_before = len_pt(&d, old) / scale_pt(&d, old);
		carry_page(&mut d, 1, old, new);
		let m = &d.marks[0];
		assert!((m.x - (1.0 - 2592.0 / 3024.0) / 2.0).abs() < 1e-9 && m.y.abs() < 1e-9, "{m:?}");
		assert!((m.w - 0.02 * 2592.0 / 3024.0).abs() < 1e-9 && (m.h - 0.02).abs() < 1e-9);
		assert_eq!(d.marks[1].x, 0.5, "other pages untouched");
		// Same real length: the scale moved with the drawing.
		assert!((len_pt(&d, new) / scale_pt(&d, new) - ratio_before).abs() < 1e-9);
		// Points stay where the same ink is: centre stays centre.
		let mut c = TakeoffData::default();
		c.marks = vec![Mark { w: 0.0, h: 0.0, ..mark(1, 0.5, 0.5) }];
		carry_page(&mut c, 1, (612.0, 792.0), (792.0, 612.0));
		assert!((c.marks[0].x - 0.5).abs() < 1e-9 && (c.marks[0].y - 0.5).abs() < 1e-9);
	}

	#[test]
	fn items_on_changed_cells() {
		let mut g = ChangeGrid::new(10, 10);
		g.set(7, 7);
		let mut d = TakeoffData::default();
		let near = mark(1, 0.71, 0.71);
		let far = mark(1, 0.1, 0.1);
		let (near_id, far_id) = (near.id.clone(), far.id.clone());
		d.marks = vec![near, far, mark(2, 0.71, 0.71)];
		// A line passing through the changed cell, and one far from it.
		d.measurements.push(Measurement::new("through", 1, MeasurementType::Length, vec![Point { x: 0.05, y: 0.75 }, Point { x: 0.95, y: 0.75 }]));
		d.measurements.push(Measurement::new("away", 1, MeasurementType::Length, vec![Point { x: 0.05, y: 0.15 }, Point { x: 0.95, y: 0.15 }]));
		let ids = items_on_changes(&d, 1, &g);
		assert!(ids.contains(&near_id) && ids.contains(&"through".to_string()));
		assert!(!ids.contains(&far_id) && !ids.contains(&"away".to_string()));
		assert_eq!(ids.len(), 2, "page 2 is another sheet");
		assert!(g.dilate().get(6, 8) && !g.dilate().get(5, 5));
	}
}
