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
			if (cx1 as i64 - cx0 as i64 + 1) * (cy1 as i64 - cy0 as i64 + 1) > 4096 {
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
		// The page is 64 cells across: a wider reach (a page with no size) finds nothing more.
		let r = (tol_frac / self.cell).ceil().min(80.0) as i32;
		let (cx, cy) = ((p.x / self.cell).floor() as i32, (p.y / self.cell).floor() as i32);
		let mut out: Vec<usize> = Vec::new();
		for dx in -r..=r {
			for dy in -r..=r {
				if let Some(v) = self.grid.get(&(cx.saturating_add(dx), cy.saturating_add(dy))) {
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

/// How the tracker steps along a drawn line (page px, i.e. points of paper).
const TRACK_SNAP: f64 = 8.0;
/// Sideways tolerance: the line must pass this close to where it was heading.
const TRACK_TOL: f64 = 2.5;
/// Longest gap jumped (the blanks of a dashed or centre line).
const TRACK_GAP: f64 = 18.0;
/// Lines turning more than this away from the heading are other lines (crossings).
const TRACK_TURN_DEG: f64 = 35.0;

impl SnapIndex {
	/// The nearest point on any line within `tol_px`, with that segment.
	fn on_line(&self, p: Point, tol_px: f64) -> Option<(Point, usize)> {
		let tol_frac = tol_px / self.page_w.min(self.page_h);
		self.near(p, tol_frac)
			.into_iter()
			.map(|i| (self.project(p, &self.segments[i]), i))
			.map(|(q, i)| (self.dist_px(p, q), q, i))
			.filter(|(d, _, _)| *d <= tol_px)
			.min_by(|a, b| a.0.total_cmp(&b.0))
			.map(|(_, q, i)| (q, i))
	}

	/// A segment's direction in px, unit length.
	fn dir_px(&self, i: usize) -> (f64, f64) {
		let s = self.segments[i];
		let (dx, dy) = ((s.b.x - s.a.x) * self.page_w, (s.b.y - s.a.y) * self.page_h);
		let l = dx.hypot(dy).max(1e-12);
		(dx / l, dy / l)
	}

	/// Follow the drawn line from near `from` to near `to`: step along it,
	/// jumping the gaps of dashed lines, passing lines that cross it, and
	/// keeping to it when another runs alongside. The path traced (dense,
	/// then simplified), or None if no single line leads from one to the other.
	pub fn track(&self, from: Point, to: Point) -> Option<Vec<Point>> {
		let (b, _) = self.on_line(to, TRACK_SNAP)?;
		// Where lines meet at the start (a crossing, a fork), try first the
		// nearest one heading towards the target.
		let tol_frac = TRACK_SNAP / self.page_w.min(self.page_h);
		let chord = ((b.x - from.x) * self.page_w, (b.y - from.y) * self.page_h);
		let cl = chord.0.hypot(chord.1).max(1e-9);
		let mut starts: Vec<(f64, f64, Point, usize)> = self
			.near(from, tol_frac)
			.into_iter()
			.filter_map(|i| {
				let q = self.project(from, &self.segments[i]);
				let d = self.dist_px(from, q);
				let dir = self.dir_px(i);
				let align = ((dir.0 * chord.0 + dir.1 * chord.1) / cl).abs();
				(d <= TRACK_SNAP).then_some((align, d, q, i))
			})
			.collect();
		// Nearest first, but a line heading across (a crossing) counts as far away;
		// lines side by side (rails) still go by distance.
		let score = |x: &(f64, f64, Point, usize)| x.1 + (1.0 - x.0) * 40.0;
		starts.sort_by(|x, y| score(x).total_cmp(&score(y)));
		starts.dedup_by(|x, y| x.3 == y.3);
		starts.into_iter().take(4).filter_map(|(_, _, a, sa)| self.track_from(a, sa, b, to)).find(|run| crate::geometry::plausible_trace(run, from, to, self.page_w, self.page_h))
	}

	fn track_from(&self, a: Point, sa: usize, b: Point, to: Point) -> Option<Vec<Point>> {
		let (w, h) = (self.page_w, self.page_h);
		let px = |p: Point| (p.x * w, p.y * h);
		let pt = |q: (f64, f64)| Point { x: q.0 / w, y: q.1 / h };
		let (target, start) = (px(b), px(a));
		let chord = (target.0 - start.0).hypot(target.1 - start.1);
		if chord < 1.0 {
			return Some(vec![a, b]);
		}
		let step = (chord / 300.0).clamp(1.0, 3.0);
		let cos_turn = TRACK_TURN_DEG.to_radians().cos();
		// Head along the starting segment, towards the target.
		let mut dir = self.dir_px(sa);
		if dir.0 * (target.0 - start.0) + dir.1 * (target.1 - start.1) < 0.0 {
			dir = (-dir.0, -dir.1);
		}
		// End on the line being followed, carried on to opposite where the
		// target was asked for (another line may be nearer to that point).
		let asked = px(to);
		let finish = |mut path: Vec<(f64, f64)>, cur: (f64, f64), dir: (f64, f64)| {
			let t = ((asked.0 - cur.0) * dir.0 + (asked.1 - cur.1) * dir.1).clamp(0.0, TRACK_GAP);
			path.push((cur.0 + dir.0 * t, cur.1 + dir.1 * t));
			let pts: Vec<Point> = path.into_iter().map(pt).collect();
			crate::geometry::simplify(&pts, 0.25, w, h)
		};
		let mut cur = start;
		let mut path = vec![start];
		let mut travelled = 0.0;
		let tol_frac = TRACK_TOL / w.min(h);
		for _ in 0..200_000 {
			let left = (target.0 - cur.0).hypot(target.1 - cur.1);
			let ahead = (target.0 - cur.0) * dir.0 + (target.1 - cur.1) * dir.1;
			// Arrived: close, or just past it.
			if left <= step * 1.5 || (ahead < 0.0 && left <= TRACK_GAP) {
				return Some(finish(path, cur, dir));
			}
			// The line where we're heading, a step on (further, across a gap).
			let mut found = None;
			let mut k = 1.0;
			while found.is_none() && k * step <= TRACK_GAP.max(step) {
				let pred = (cur.0 + dir.0 * step * k, cur.1 + dir.1 * step * k);
				let pp = pt(pred);
				found = self
					.near(pp, tol_frac)
					.into_iter()
					.filter_map(|i| {
						let d = self.dir_px(i);
						let c = d.0 * dir.0 + d.1 * dir.1;
						if c.abs() < cos_turn {
							return None;
						}
						let q = px(self.project(pp, &self.segments[i]));
						let off = (q.0 - pred.0).hypot(q.1 - pred.1);
						// It must take us forward (the end of the dash we're on doesn't).
						let gain = (q.0 - cur.0) * dir.0 + (q.1 - cur.1) * dir.1;
						(off <= TRACK_TOL && gain > step * 0.25).then_some((off, q, if c < 0.0 { (-d.0, -d.1) } else { d }))
					})
					.min_by(|x, y| x.0.total_cmp(&y.0));
				k += 1.0;
			}
			// The line ends (or turns away) within a gap of the target: that's arriving too.
			let Some((_, q, seg_dir)) = found else {
				if left <= TRACK_GAP {
					return Some(finish(path, cur, dir));
				}
				return None;
			};
			let mv = (q.0 - cur.0, q.1 - cur.1);
			let len = mv.0.hypot(mv.1);
			if len < 1e-6 {
				return None;
			}
			// Turn with the line (its own direction, steadied by the step taken).
			let blend = (seg_dir.0 * 0.7 + mv.0 / len * 0.3, seg_dir.1 * 0.7 + mv.1 / len * 0.3);
			let bl = blend.0.hypot(blend.1).max(1e-12);
			dir = (blend.0 / bl, blend.1 / bl);
			cur = q;
			path.push(cur);
			travelled += len;
			// Wandered off along some other line.
			if travelled > chord * 3.0 + 60.0 {
				return None;
			}
		}
		None
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
	((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some(Point { x: x1 + u * (x2 - x1), y: y1 + u * (y2 - y1) })
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

	/// A dashed quarter circle (radius 300 px), a line crossing it, and a
	/// second arc 4 px outside it, like a pair of rails.
	fn dashed_curve() -> SnapIndex {
		let (cx, cy, r) = (500.0, 500.0, 300.0);
		let mut segs = Vec::new();
		for (radius, dashed) in [(r, true), (r + 4.0, false)] {
			let n = 180;
			for k in 0..n {
				// Dashes: 6 drawn, 3 blank (in 0.5° steps ≈ 2.6 px each).
				if dashed && k % 9 >= 6 {
					continue;
				}
				let (a0, a1) = (k as f64 / n as f64 * std::f64::consts::FRAC_PI_2, (k + 1) as f64 / n as f64 * std::f64::consts::FRAC_PI_2);
				segs.push(Segment { a: p((cx + radius * a0.cos()) / 1000.0, (cy + radius * a0.sin()) / 1000.0), b: p((cx + radius * a1.cos()) / 1000.0, (cy + radius * a1.sin()) / 1000.0) });
			}
		}
		// A road crossing the curve at 45°, square to it.
		segs.push(Segment { a: p(0.5, 0.5), b: p(0.95, 0.95) });
		SnapIndex::new(segs, 1000.0, 1000.0)
	}

	#[test]
	fn track_follows_a_dashed_curve_across_a_crossing() {
		let g = dashed_curve();
		let (from, to) = (p(0.8, 0.5), p(0.5, 0.8));
		let run = g.track(from, to).expect("tracked");
		let len = crate::geometry::polyline_length_px(&run, 1000.0, 1000.0);
		let arc = 300.0 * std::f64::consts::FRAC_PI_2;
		assert!((len - arc).abs() < 1.5, "length {len:.2} vs arc {arc:.2}");
		// It kept to the dashed rail, not the solid one 4 px outside.
		for q in &run {
			let r = (q.x * 1000.0 - 500.0).hypot(q.y * 1000.0 - 500.0);
			assert!((r - 300.0).abs() < 1.5, "off the rail: r = {r:.2}");
		}
		assert!(run.len() < 200, "simplified: {} points", run.len());
		// Nothing leads from the curve to a point off every line.
		assert!(g.track(from, p(0.1, 0.1)).is_none());
	}

	#[test]
	fn far_off_lines_and_sizeless_pages_dont_overflow() {
		// A broken file: a line out at 1e30 and one across the whole i32 range, on a page with no size.
		let segs = vec![Segment { a: p(1e30, 1e30), b: p(1e30, -1e30) }, Segment { a: p(-1e30, 0.5), b: p(1e30, 0.5) }, Segment { a: p(0.2, 0.2), b: p(0.4, 0.2) }];
		for (w, h) in [(0.0, 0.0), (1000.0, 1000.0)] {
			let g = SnapIndex::new(segs.clone(), w, h);
			let _ = g.nearest(p(0.3, 0.2), 8.0);
			let _ = g.nearest(p(1e30, 1e30), 8.0);
			let _ = g.track(p(0.2, 0.2), p(0.4, 0.2));
			let _ = g.follow(2, 2.0);
		}
	}
}
