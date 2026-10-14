//! One snap engine for the person in the viewer and for the agent: snap a
//! point to the drawing's own vector geometry (endpoint, midpoint,
//! intersection, on-line), and follow a run of connected lines.
//! Geometry is in page fractions; tolerances are in page px so they feel the
//! same at every zoom (callers pass the current px-per-fraction).

use std::collections::HashMap;

use crate::model::Point;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
	pub a: Point,
	pub b: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapKind {
	Endpoint,
	Midpoint,
	Intersection,
	OnLine,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snap {
	pub point: Point,
	pub kind: SnapKind,
	/// Index of the segment snapped to.
	pub segment: usize,
}

/// Page geometry with a uniform grid index for fast nearest queries.
pub struct SnapIndex {
	pub segments: Vec<Segment>,
	cell: f64,
	grid: HashMap<(i32, i32), Vec<usize>>,
	/// Page aspect: px per fraction in x and y (any consistent render size).
	pub page_w: f64,
	pub page_h: f64,
}

impl SnapIndex {
	/// `page_w/page_h`: page size in any px unit (points are fine).
	pub fn new(segments: Vec<Segment>, page_w: f64, page_h: f64) -> Self {
		// ~64 cells across; long segments are registered in every cell they cross.
		let cell = 1.0 / 64.0;
		let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
		for (i, s) in segments.iter().enumerate() {
			let (x0, x1) = (s.a.x.min(s.b.x), s.a.x.max(s.b.x));
			let (y0, y1) = (s.a.y.min(s.b.y), s.a.y.max(s.b.y));
			let (cx0, cx1) = ((x0 / cell).floor() as i32, (x1 / cell).floor() as i32);
			let (cy0, cy1) = ((y0 / cell).floor() as i32, (y1 / cell).floor() as i32);
			// Very long segments (borders): cap cells to keep the index small.
			if (cx1 - cx0 + 1) as i64 * (cy1 - cy0 + 1) as i64 > 4096 {
				continue;
			}
			for cx in cx0..=cx1 {
				for cy in cy0..=cy1 {
					grid.entry((cx, cy)).or_default().push(i);
				}
			}
		}
		Self { segments, cell, grid, page_w, page_h }
	}

	pub fn is_empty(&self) -> bool {
		self.segments.is_empty()
	}

	fn near(&self, p: Point, tol_frac: f64) -> Vec<usize> {
		let r = (tol_frac / self.cell).ceil() as i32;
		let (cx, cy) = ((p.x / self.cell).floor() as i32, (p.y / self.cell).floor() as i32);
		let mut out: Vec<usize> = Vec::new();
		for dx in -r..=r {
			for dy in -r..=r {
				if let Some(v) = self.grid.get(&(cx + dx, cy + dy)) {
					out.extend_from_slice(v);
				}
			}
		}
		out.sort_unstable();
		out.dedup();
		out
	}

	fn dist_px(&self, a: Point, b: Point) -> f64 {
		((a.x - b.x) * self.page_w).hypot((a.y - b.y) * self.page_h)
	}

	fn project(&self, p: Point, s: &Segment) -> Point {
		let (ax, ay) = (s.a.x * self.page_w, s.a.y * self.page_h);
		let (bx, by) = (s.b.x * self.page_w, s.b.y * self.page_h);
		let (px, py) = (p.x * self.page_w, p.y * self.page_h);
		let (dx, dy) = (bx - ax, by - ay);
		let len2 = dx * dx + dy * dy;
		let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
		Point { x: (ax + t * dx) / self.page_w, y: (ay + t * dy) / self.page_h }
	}

	/// Best snap within `tol_px` (page px): endpoints beat midpoints beat
	/// intersections beat "on the line", as in CAD/Bluebeam.
	pub fn nearest(&self, p: Point, tol_px: f64) -> Option<Snap> {
		let tol_frac = tol_px / self.page_w.min(self.page_h);
		let cands = self.near(p, tol_frac);
		let mut best: Option<(u8, f64, Snap)> = None;
		let mut consider = |rank: u8, pt: Point, kind: SnapKind, seg: usize, d: f64| {
			if d <= tol_px && best.as_ref().is_none_or(|(r, bd, _)| rank < *r || (rank == *r && d < *bd)) {
				best = Some((rank, d, Snap { point: pt, kind, segment: seg }));
			}
		};
		for &i in &cands {
			let s = &self.segments[i];
			for e in [s.a, s.b] {
				consider(0, e, SnapKind::Endpoint, i, self.dist_px(p, e));
			}
			let mid = Point { x: (s.a.x + s.b.x) / 2.0, y: (s.a.y + s.b.y) / 2.0 };
			consider(1, mid, SnapKind::Midpoint, i, self.dist_px(p, mid));
			let on = self.project(p, s);
			consider(3, on, SnapKind::OnLine, i, self.dist_px(p, on));
		}
		// Intersections among the few nearby segments.
		if cands.len() <= 200 {
			for (k, &i) in cands.iter().enumerate() {
				for &j in &cands[k + 1..] {
					if let Some(x) = intersect(&self.segments[i], &self.segments[j]) {
						consider(2, x, SnapKind::Intersection, i, self.dist_px(p, x));
					}
				}
			}
		}
		best.map(|(_, _, s)| s)
	}

	/// Walk connected segments from `start` both ways until a fork or a gap,
	/// returning the run as a polyline (for "follow line" measuring).
	pub fn follow(&self, start: usize, join_tol_px: f64) -> Vec<Point> {
		let tol_frac = join_tol_px / self.page_w.min(self.page_h);
		let mut used = vec![false; self.segments.len()];
		used[start] = true;
		let s = self.segments[start];
		let mut forward = vec![s.a, s.b];
		self.extend(&mut forward, &mut used, tol_frac, join_tol_px);
		let mut backward = vec![s.b, s.a];
		self.extend(&mut backward, &mut used, tol_frac, join_tol_px);
		backward.reverse();
		backward.pop();
		backward.pop();
		backward.extend(forward);
		backward
	}

	fn extend(&self, path: &mut Vec<Point>, used: &mut [bool], tol_frac: f64, tol_px: f64) {
		for _ in 0..10_000 {
			let end = *path.last().unwrap();
			let mut next: Vec<(usize, Point)> = Vec::new();
			for i in self.near(end, tol_frac) {
				if used[i] {
					continue;
				}
				let s = self.segments[i];
				if self.dist_px(s.a, end) <= tol_px {
					next.push((i, s.b));
				} else if self.dist_px(s.b, end) <= tol_px {
					next.push((i, s.a));
				}
			}
			// Stop at forks: the run is ambiguous there.
			if next.len() != 1 {
				return;
			}
			let (i, p) = next[0];
			used[i] = true;
			path.push(p);
		}
	}
}

fn intersect(s: &Segment, t: &Segment) -> Option<Point> {
	let (x1, y1, x2, y2) = (s.a.x, s.a.y, s.b.x, s.b.y);
	let (x3, y3, x4, y4) = (t.a.x, t.a.y, t.b.x, t.b.y);
	let den = (x1 - x2) * (y3 - y4) - (y1 - y2) * (x3 - x4);
	if den.abs() < 1e-15 {
		return None;
	}
	let u = ((x1 - x3) * (y3 - y4) - (y1 - y3) * (x3 - x4)) / den;
	let v = -((x1 - x2) * (y1 - y3) - (y1 - y2) * (x1 - x3)) / den;
	((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then(|| Point { x: x1 + u * (x2 - x1), y: y1 + u * (y2 - y1) })
}

/// Fit a rough polyline onto the drawing: each vertex snaps to the nearest
/// endpoint/intersection within `tol_px`, else stays. Returns how far each moved (px).
pub fn fit_points(index: &SnapIndex, points: &[Point], tol_px: f64) -> (Vec<Point>, Vec<f64>) {
	let mut out = Vec::with_capacity(points.len());
	let mut moved = Vec::with_capacity(points.len());
	for p in points {
		match index.nearest(*p, tol_px).filter(|s| s.kind != SnapKind::OnLine) {
			Some(s) => {
				moved.push(index.dist_px(*p, s.point));
				out.push(s.point);
			}
			None => {
				moved.push(0.0);
				out.push(*p);
			}
		}
	}
	(out, moved)
}

/// Shift-constrain: the point on the 0/45/90° rays from `from` nearest `to` (px-aware).
pub fn ortho(from: Point, to: Point, page_w: f64, page_h: f64) -> Point {
	let (dx, dy) = ((to.x - from.x) * page_w, (to.y - from.y) * page_h);
	let len = dx.hypot(dy);
	if len == 0.0 {
		return to;
	}
	let angle = dy.atan2(dx);
	let step = std::f64::consts::FRAC_PI_4;
	let snapped = (angle / step).round() * step;
	// Keep the projected length on the chosen ray.
	let proj = dx * snapped.cos() + dy * snapped.sin();
	Point { x: from.x + proj * snapped.cos() / page_w, y: from.y + proj * snapped.sin() / page_h }
}

#[cfg(test)]
mod tests {
	use super::*;

	fn p(x: f64, y: f64) -> Point {
		Point { x, y }
	}

	fn grid() -> SnapIndex {
		// A rectangle 0.2..0.6 × 0.2..0.4 plus a diagonal through it.
		let pts = [p(0.2, 0.2), p(0.6, 0.2), p(0.6, 0.4), p(0.2, 0.4)];
		let mut segs: Vec<Segment> = (0..4).map(|i| Segment { a: pts[i], b: pts[(i + 1) % 4] }).collect();
		segs.push(Segment { a: p(0.1, 0.1), b: p(0.7, 0.5) });
		SnapIndex::new(segs, 1000.0, 1000.0)
	}

	#[test]
	fn endpoint_beats_line() {
		let s = grid().nearest(p(0.203, 0.198), 8.0).unwrap();
		assert_eq!(s.kind, SnapKind::Endpoint);
		assert_eq!(s.point, p(0.2, 0.2));
		let s = grid().nearest(p(0.4, 0.405), 8.0).unwrap();
		assert_eq!(s.kind, SnapKind::Midpoint);
		let s = grid().nearest(p(0.45, 0.403), 8.0).unwrap();
		assert_eq!(s.kind, SnapKind::OnLine);
		assert!((s.point.y - 0.4).abs() < 1e-12);
	}

	#[test]
	fn follow_walks_the_loop() {
		let g = SnapIndex::new(grid().segments[..4].to_vec(), 1000.0, 1000.0);
		let run = g.follow(0, 1.0);
		assert_eq!(run.len(), 5, "{run:?}");
	}

	#[test]
	fn ortho_locks_to_45() {
		let q = ortho(p(0.0, 0.0), p(0.5, 0.02), 1000.0, 1000.0);
		assert!(q.y.abs() < 1e-12);
	}
}
