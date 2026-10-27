//! The space around a point, bounded by the lines drawn around it: a room,
//! a slab, a paved area. The page is rendered around the point, everything
//! that stands out from the paper there is a boundary, and the paper is
//! flooded from the point. Doorways and other openings are closed by
//! treating the boundary as thicker (half the opening on each side), filling,
//! then growing the fill back by the same amount; the narrowest closing that
//! holds is used, so a room with an open doorway comes out as the room, not
//! the whole floor. Works the same on vector sheets and scans.

use image::RgbaImage;
use takeoff_core::Point;

use crate::{Drawing, FracRect, PdfError};

/// Render resolution and the most pixels a side.
const PX_PER_PT: f64 = 4.0;
const MAX_EDGE: u32 = 4000;
/// First window around the point, in points of paper; grows when the fill reaches its edge.
const WINDOW_PT: f64 = 600.0;
/// Openings tried on a page with no scale, widest gap closed in points of paper.
pub const GAPS_PT: [f64; 8] = [0.0, 3.0, 6.0, 12.0, 18.0, 27.0, 36.0, 54.0];
/// On a scaled page: slips in the line work, then real openings (metres): a
/// door, a pair of doors, a wide opening.
const SLIPS_PT: [f64; 2] = [0.0, 3.0];
const OPENINGS_M: [f64; 3] = [1.2, 2.0, 3.0];

/// The openings to try, in points of paper, given how many points of paper
/// make a metre at the page's scale (None: no scale).
pub fn gaps(pt_per_m: Option<f64>) -> Vec<f64> {
	match pt_per_m.filter(|p| p.is_finite() && *p > 0.0) {
		Some(p) => SLIPS_PT.iter().copied().chain(OPENINGS_M.iter().map(|m| m * p).filter(|g| *g > SLIPS_PT[1])).collect(),
		None => GAPS_PT.to_vec(),
	}
}
/// A fill bigger than this share of the page has leaked out.
const MAX_SHARE: f64 = 0.6;
/// How far a pixel's colour may be from the paper's and still be paper.
const INK: f64 = 60.0;

/// A filled space.
#[derive(Debug, Clone)]
pub struct Fill {
	/// Outline, page fractions, clockwise on screen.
	pub points: Vec<Point>,
	/// Area in square points of paper.
	pub area_pt2: f64,
	/// Widest opening closed to get it, in points of paper (0 = none needed).
	pub gap_pt: f64,
}

/// Why nothing was filled.
#[derive(Debug, Clone, PartialEq)]
pub enum NoFill {
	/// The point is on a line, not in a space.
	OnLine,
	/// No closed boundary: the fill runs off the page or over most of it.
	Open,
}

type Mask = Vec<bool>;

struct Grid {
	w: usize,
	h: usize,
}

impl Grid {
	fn at(&self, x: usize, y: usize) -> usize {
		y * self.w + x
	}
}

/// Chamfer (3-4) distance to the nearest `true`, in px.
fn distance(g: &Grid, from: &Mask) -> Vec<f32> {
	let (w, h) = (g.w, g.h);
	let big = f32::MAX / 4.0;
	let mut d: Vec<f32> = from.iter().map(|&b| if b { 0.0 } else { big }).collect();
	for y in 0..h {
		for x in 0..w {
			let i = g.at(x, y);
			let mut v = d[i];
			if x > 0 {
				v = v.min(d[i - 1] + 1.0);
			}
			if y > 0 {
				v = v.min(d[i - w] + 1.0);
				if x > 0 {
					v = v.min(d[i - w - 1] + std::f32::consts::SQRT_2);
				}
				if x + 1 < w {
					v = v.min(d[i - w + 1] + std::f32::consts::SQRT_2);
				}
			}
			d[i] = v;
		}
	}
	for y in (0..h).rev() {
		for x in (0..w).rev() {
			let i = g.at(x, y);
			let mut v = d[i];
			if x + 1 < w {
				v = v.min(d[i + 1] + 1.0);
			}
			if y + 1 < h {
				v = v.min(d[i + w] + 1.0);
				if x + 1 < w {
					v = v.min(d[i + w + 1] + std::f32::consts::SQRT_2);
				}
				if x > 0 {
					v = v.min(d[i + w - 1] + std::f32::consts::SQRT_2);
				}
			}
			d[i] = v;
		}
	}
	d
}

/// Flood the open pixels 4-connected to `seed`.
fn flood(g: &Grid, open: impl Fn(usize) -> bool, seed: (usize, usize)) -> Mask {
	let mut m = vec![false; g.w * g.h];
	let s = g.at(seed.0, seed.1);
	if !open(s) {
		return m;
	}
	let mut stack = vec![s];
	m[s] = true;
	while let Some(i) = stack.pop() {
		let (x, y) = (i % g.w, i / g.w);
		let mut push = |j: usize| {
			if !m[j] && open(j) {
				m[j] = true;
				stack.push(j);
			}
		};
		if x > 0 {
			push(i - 1);
		}
		if x + 1 < g.w {
			push(i + 1);
		}
		if y > 0 {
			push(i - g.w);
		}
		if y + 1 < g.h {
			push(i + g.w);
		}
	}
	m
}

/// The outer boundary of a mask along pixel edges (vertices on pixel
/// corners): every loop of edges between in and out, keeping the one that
/// encloses the most (holes and specks inside are left in the fill).
fn outline(g: &Grid, m: &Mask) -> Vec<(f64, f64)> {
	use std::collections::HashMap;
	let inside = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < g.w && (y as usize) < g.h && m[g.at(x as usize, y as usize)];
	// Directed edges with the fill on the right (y down): corner → corner.
	let mut next: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
	for y in 0..g.h as i64 {
		for x in 0..g.w as i64 {
			if !inside(x, y) {
				continue;
			}
			if !inside(x, y - 1) {
				next.entry((x, y)).or_default().push((x + 1, y));
			}
			if !inside(x + 1, y) {
				next.entry((x + 1, y)).or_default().push((x + 1, y + 1));
			}
			if !inside(x, y + 1) {
				next.entry((x + 1, y + 1)).or_default().push((x, y + 1));
			}
			if !inside(x - 1, y) {
				next.entry((x, y + 1)).or_default().push((x, y));
			}
		}
	}
	let mut best: (f64, Vec<(f64, f64)>) = (0.0, vec![]);
	let starts: Vec<(i64, i64)> = next.keys().copied().collect();
	for s in starts {
		while next.get(&s).is_some_and(|v| !v.is_empty()) {
			let mut lp = vec![s];
			let mut cur = s;
			let mut dir = (0i64, 0i64);
			while let Some(outs) = next.get_mut(&cur).filter(|o| !o.is_empty()) {
				// Where two corners touch, turn right so loops stay apart.
				let k = if outs.len() > 1 {
					let right = (-dir.1, dir.0);
					outs.iter().position(|o| (o.0 - cur.0, o.1 - cur.1) == right).unwrap_or(0)
				} else {
					0
				};
				let n = outs.swap_remove(k);
				dir = (n.0 - cur.0, n.1 - cur.1);
				cur = n;
				if cur == s {
					break;
				}
				lp.push(cur);
			}
			let area = lp.iter().zip(lp.iter().cycle().skip(1)).map(|(a, b)| (a.0 * b.1 - b.0 * a.1) as f64).sum::<f64>() / 2.0;
			if area > best.0 {
				best = (area, lp.into_iter().map(|(x, y)| (x as f64, y as f64)).collect());
			}
		}
	}
	best.1
}

/// One fill of `img` from `seed` with gaps up to `gap` px closed. None when
/// the seed is shut in by the thickened boundary.
fn fill_once(g: &Grid, ink: &Mask, to_ink: &[f32], seed: (usize, usize), gap: f64) -> Option<Mask> {
	let r = (gap / 2.0) as f32;
	let core = flood(g, |i| !ink[i] && to_ink[i] > r, seed);
	if !core.iter().any(|&b| b) {
		return None;
	}
	if r <= 0.0 {
		return Some(core);
	}
	// Grow back over the band the thicker boundary took, never onto a line,
	// sharing it with the spaces next door: each band pixel goes to the space
	// nearest it, walking over paper. So a doorway splits down its middle,
	// while a corner (no other space reaches it past the walls) is filled out
	// square. Out to r√2: that is how far a corner is from the core's.
	let reach = r * std::f32::consts::SQRT_2 + 0.5;
	let (x0, y0, x1, y1) = bounds(g, &core);
	let pad = reach.ceil() as usize + 1;
	let (x0, y0, x1, y1) = (x0.saturating_sub(pad), y0.saturating_sub(pad), (x1 + pad).min(g.w - 1), (y1 + pad).min(g.h - 1));
	use std::cmp::Reverse;
	use std::collections::BinaryHeap;
	// Label: 0 none, 1 ours, 2 another space's.
	let mut label = vec![0u8; g.w * g.h];
	let mut best = vec![u32::MAX; g.w * g.h];
	let mut heap = BinaryHeap::new();
	let open = |i: usize| !ink[i] && to_ink[i] > r;
	for y in y0..=y1 {
		for x in x0..=x1 {
			let i = g.at(x, y);
			if open(i) {
				label[i] = if core[i] { 1 } else { 2 };
				best[i] = 0;
				// Only the ones at the band's edge have anywhere to go.
				let band = |j: usize| !ink[j] && !open(j);
				if (x > 0 && band(i - 1)) || (x + 1 < g.w && band(i + 1)) || (y > 0 && band(i - g.w)) || (y + 1 < g.h && band(i + g.w)) {
					heap.push(Reverse((0u32, i)));
				}
			}
		}
	}
	let max = (reach * 10.0) as u32;
	while let Some(Reverse((d, i))) = heap.pop() {
		if d > best[i] {
			continue;
		}
		let (x, y) = (i % g.w, i / g.w);
		for (dx, dy, w) in [(-1i64, 0i64, 10u32), (1, 0, 10), (0, -1, 10), (0, 1, 10), (-1, -1, 14), (1, -1, 14), (-1, 1, 14), (1, 1, 14)] {
			let (nx, ny) = (x as i64 + dx, y as i64 + dy);
			if nx < x0 as i64 || ny < y0 as i64 || nx > x1 as i64 || ny > y1 as i64 {
				continue;
			}
			let j = g.at(nx as usize, ny as usize);
			let nd = d + w;
			if ink[j] || open(j) || nd > max || nd >= best[j] {
				continue;
			}
			best[j] = nd;
			label[j] = label[i];
			heap.push(Reverse((nd, j)));
		}
	}
	Some((0..core.len()).map(|i| label[i] == 1).collect())
}

/// The box around the set pixels: (x0, y0, x1, y1).
fn bounds(g: &Grid, m: &Mask) -> (usize, usize, usize, usize) {
	let (mut x0, mut y0, mut x1, mut y1) = (g.w, g.h, 0, 0);
	for (i, _) in m.iter().enumerate().filter(|(_, &b)| b) {
		let (x, y) = (i % g.w, i / g.w);
		x0 = x0.min(x);
		y0 = y0.min(y);
		x1 = x1.max(x);
		y1 = y1.max(y);
	}
	(x0, y0, x1, y1)
}

/// Close the seams between pieces of a fill: the lines between two spaces
/// filled together, up to `c` px wide (dilate, then erode, by `c`).
fn close(g: &Grid, m: &Mask, c: f32) -> Mask {
	let grown: Mask = distance(g, m).iter().map(|d| *d <= c).collect();
	let outside: Mask = grown.iter().map(|b| !b).collect();
	// Off the window counts as outside, so a fill reaching the edge keeps it.
	let to_out = distance(g, &outside);
	(0..m.len()).map(|i| m[i] || (grown[i] && to_out[i] > c && {
		let (x, y) = ((i % g.w) as f32, (i / g.w) as f32);
		x.min(y).min(g.w as f32 - 1.0 - x).min(g.h as f32 - 1.0 - y) >= c
	})).collect()
}

/// What a fill of `img` from `seeds` gives with each gap in `gaps` (px):
/// the mask and whether it touches the image edge. Several points fill
/// together, as one space (their seams closed, up to `seam` px). A point on
/// a line moves to the nearest paper within `nudge` px.
fn fills(img: &RgbaImage, seeds: &[(f64, f64)], gaps: &[f64], nudge: f64, seam: f64) -> Result<(Vec<Option<(Mask, bool)>>, Mask), NoFill> {
	let g = Grid { w: img.width() as usize, h: img.height() as usize };
	// Nothing reaches past the window: a huge gap (a model's) would overflow the searches below.
	let most = (g.w + g.h) as f64;
	let nudge = nudge.min(most);
	let px = |x: usize, y: usize| {
		let c = img.get_pixel(x as u32, y as u32).0;
		[c[0] as f64, c[1] as f64, c[2] as f64]
	};
	let dist = |a: [f64; 3], b: [f64; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
	let clamp = |p: (f64, f64)| ((p.0.max(0.0) as usize).min(g.w - 1), (p.1.max(0.0) as usize).min(g.h - 1));
	let (sx, sy) = clamp(*seeds.first().ok_or(NoFill::OnLine)?);
	// The paper's colour where the first point is: the lightest of the area
	// nearby (a room may be tinted; the point may sit on its label).
	let r = nudge.max(2.0) as i64;
	let mut paper: Option<(f64, [f64; 3])> = None;
	for dy in -r..=r {
		for dx in -r..=r {
			let (x, y) = (sx as i64 + dx, sy as i64 + dy);
			if x >= 0 && y >= 0 && (x as usize) < g.w && (y as usize) < g.h {
				let c = px(x as usize, y as usize);
				if paper.is_none_or(|p| c[0] + c[1] + c[2] > p.0) {
					paper = Some((c[0] + c[1] + c[2], c));
				}
			}
		}
	}
	let paper = paper.ok_or(NoFill::OnLine)?.1;
	let ink: Mask = (0..g.w * g.h).map(|i| dist(px(i % g.w, i / g.w), paper) > INK).collect();
	let to_ink = distance(&g, &ink);
	// The nearest pixel to `p` where `ok`, within `reach`.
	let nearest = |p: (usize, usize), reach: i64, ok: &dyn Fn(usize) -> bool| -> Option<(usize, usize)> {
		let mut best: Option<(i64, (usize, usize))> = None;
		for dy in -reach..=reach {
			for dx in -reach..=reach {
				let (x, y) = (p.0 as i64 + dx, p.1 as i64 + dy);
				if x >= 0 && y >= 0 && (x as usize) < g.w && (y as usize) < g.h && ok(g.at(x as usize, y as usize)) {
					let d = dx * dx + dy * dy;
					if best.is_none_or(|b| d < b.0) {
						best = Some((d, (x as usize, y as usize)));
					}
				}
			}
		}
		best.map(|b| b.1)
	};
	// Each point on paper.
	let seeds: Vec<(usize, usize)> = seeds.iter().map(|s| {
		let s = clamp(*s);
		if ink[g.at(s.0, s.1)] { nearest(s, r, &|i| !ink[i]).ok_or(NoFill::OnLine) } else { Ok(s) }
	}).collect::<Result<_, _>>()?;
	let out = gaps
		.iter()
		.map(|&gap| {
			let gap = gap.min(most);
			let rr = (gap / 2.0) as f32;
			let mut all: Option<Mask> = None;
			for &seed in &seeds {
				if all.as_ref().is_some_and(|m| m[g.at(seed.0, seed.1)]) {
					continue;
				}
				// A thick boundary may cover the point: start from the open pixel nearest it.
				let s = if to_ink[g.at(seed.0, seed.1)] <= rr { nearest(seed, (gap + nudge) as i64, &|i| to_ink[i] > rr)? } else { seed };
				let m = fill_once(&g, &ink, &to_ink, s, gap)?;
				// The grown fill must still hold the point asked for.
				if !m[g.at(seed.0, seed.1)] {
					return None;
				}
				all = Some(match all {
					Some(a) => a.iter().zip(&m).map(|(x, y)| *x || *y).collect(),
					None => m,
				});
			}
			let mut m = all?;
			if seeds.len() > 1 {
				m = close(&g, &m, seam as f32);
			}
			let edge = (0..g.w).any(|x| m[g.at(x, 0)] || m[g.at(x, g.h - 1)]) || (0..g.h).any(|y| m[g.at(0, y)] || m[g.at(g.w - 1, y)]);
			Some((m, edge))
		})
		.collect();
	Ok((out, ink))
}

/// An opening closed by the fill is crossed by its outline through open
/// paper: draw it straight across instead, from where the outline leaves the
/// line work to where it meets it again (jamb to jamb).
fn straighten(g: &Grid, ink: &Mask, ring: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
	// A corner touches ink when a pixel within ~1.5 px of it is ink.
	let on = |p: &(f64, f64)| {
		let (x, y) = (p.0 as i64, p.1 as i64);
		(y - 2..=y + 1).any(|yy| (x - 2..=x + 1).any(|xx| xx >= 0 && yy >= 0 && (xx as usize) < g.w && (yy as usize) < g.h && ink[g.at(xx as usize, yy as usize)]))
	};
	let Some(start) = ring.iter().position(on) else { return ring };
	let n = ring.len();
	(0..n).map(|k| ring[(start + k) % n]).filter(on).collect()
}

/// Pick the closing that matters: areas per gap, narrowest first
/// (INFINITY: leaked off the page; None: the point was shut in). Closing a
/// doorway the fill leaked through shows as the biggest drop; with no big
/// drop the narrowest closing is the space.
fn choose(areas: &[Option<f64>]) -> Option<usize> {
	let ok: Vec<usize> = (0..areas.len()).filter(|&i| areas[i].is_some()).collect();
	let mut pick = *ok.first()?;
	let mut most = 0.3;
	for w in ok.windows(2) {
		let (a, b) = (areas[w[0]]?, areas[w[1]]?);
		let drop = if a.is_infinite() { if b.is_infinite() { 0.0 } else { 1.0 } } else { (a - b) / a };
		if drop > most {
			most = drop;
			pick = w[1];
		}
	}
	areas[pick]?.is_finite().then_some(pick)
}

impl Drawing {
	/// Fill the space around `at` (page fractions; several points fill
	/// together as one space, e.g. both halves of an opening a line crosses).
	/// `gaps`: the openings to try closing (points of paper, see [`gaps`]),
	/// keeping the closing that matters; one to close just that.
	pub fn fill_area(&self, page: u32, at: &[Point], gaps: &[f64]) -> Result<Result<Fill, NoFill>, PdfError> {
		let (w, h) = self.page_size_pt(page)?;
		let (w, h) = (w as f64, h as f64);
		if at.is_empty() {
			return Ok(Err(NoFill::OnLine));
		}
		// A page with no size (a broken file) has no space to fill.
		if !(w > 0.0 && h > 0.0) {
			return Ok(Err(NoFill::Open));
		}
		let gaps: Vec<f64> = if gaps.is_empty() { GAPS_PT.to_vec() } else { gaps.iter().map(|g| g.max(0.0)).collect() };
		let (ax0, ay0) = at.iter().fold((1.0f64, 1.0f64), |m, p| (m.0.min(p.x), m.1.min(p.y)));
		let (ax1, ay1) = at.iter().fold((0.0f64, 0.0f64), |m, p| (m.0.max(p.x), m.1.max(p.y)));
		let mut half = WINDOW_PT / 2.0;
		loop {
			let (hw, hh) = ((half / w).min(1.0), (half / h).min(1.0));
			let (x0, y0) = ((ax0 - hw).max(0.0), (ay0 - hh).max(0.0));
			let (x1, y1) = ((ax1 + hw).min(1.0), (ay1 + hh).min(1.0));
			let region = FracRect { x: x0, y: y0, w: (x1 - x0).max(1e-4), h: (y1 - y0).max(1e-4) };
			let whole = x0 <= 0.0 && y0 <= 0.0 && x1 >= 1.0 && y1 >= 1.0;
			let long_pt = (region.w * w).max(region.h * h);
			let ppt = PX_PER_PT.min(MAX_EDGE as f64 / long_pt.max(1.0));
			// Lines drawn crisp: a hairline stays solid however coarse the render.
			let r = self.render_lines(page, (ppt * 72.0) as f32, region, Some(MAX_EDGE), false)?;
			let (pw, ph) = (r.page_px.0 as f64, r.page_px.1 as f64);
			let ppt = pw / w;
			let seeds: Vec<(f64, f64)> = at.iter().map(|p| ((p.x - r.region.x) * pw, (p.y - r.region.y) * ph)).collect();
			let gaps_px: Vec<f64> = gaps.iter().map(|g| g * ppt).collect();
			let (results, ink) = match fills(&r.image, &seeds, &gaps_px, 3.0 * ppt, (2.0 * ppt).max(2.0)) {
				Ok(r) => r,
				Err(e) => return Ok(Err(e)),
			};
			// Touching the window's edge: the fill may go on past it. On the
			// page's edge it has leaked out.
			let max_px = MAX_SHARE * pw * ph;
			let areas: Vec<Option<f64>> = results
				.iter()
				.map(|f| f.as_ref().map(|(m, edge)| {
					let n = m.iter().filter(|&&b| b).count() as f64;
					if (*edge && whole) || n > max_px { f64::INFINITY } else { n }
				}))
				.collect();
			let pick = choose(&areas);
			let grow = !whole && match pick {
				Some(i) => results[i].as_ref().is_some_and(|f| f.1),
				// All leaked in this window: a bigger one may close them.
				None => results.iter().any(|f| f.as_ref().is_some_and(|f| f.1)),
			};
			if grow {
				half *= 2.5;
				continue;
			}
			let Some(i) = pick else { return Ok(Err(NoFill::Open)) };
			let (m, _) = results[i].as_ref().unwrap();
			let g = Grid { w: r.image.width() as usize, h: r.image.height() as usize };
			let ring = straighten(&g, &ink, outline(&g, m));
			let pts: Vec<Point> = ring.iter().map(|(x, y)| Point { x: x / pw + r.region.x, y: y / ph + r.region.y }).collect();
			// Pixel stairs into straight runs, under a pixel off.
			let mut pts = takeoff_core::geometry::simplify(&pts, 0.75 / ppt, w, h);
			// Simplifying keeps the ring's first point, though it may sit
			// mid-edge: drop the points at the seam while they are on the line between their neighbours.
			let flat = |pts: &[Point], i: usize| {
				let n = pts.len();
				takeoff_core::geometry::point_segment_distance_px(pts[i], pts[(i + n - 1) % n], pts[(i + 1) % n], w, h) < 0.75 / ppt
			};
			while pts.len() > 3 {
				if flat(&pts, 0) {
					pts.remove(0);
				} else if flat(&pts, pts.len() - 1) {
					pts.pop();
				} else {
					break;
				}
			}
			let area_pt2 = takeoff_core::geometry::polygon_area_px(&pts, w, h);
			return Ok(Ok(Fill { points: pts, area_pt2, gap_pt: gaps[i] }));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Two rooms side by side, 1 px walls: the left one 200 × 150 px with a
	/// 20 px doorway into the right one, which is 200 × 150 px too. A label
	/// sits in the left room. All inside a bordered sheet.
	fn plan() -> RgbaImage {
		let mut img = RgbaImage::from_pixel(600, 400, image::Rgba([255, 255, 255, 255]));
		let ink = image::Rgba([20, 20, 20, 255]);
		let mut line = |x0: u32, y0: u32, x1: u32, y1: u32| {
			for y in y0..=y1 {
				for x in x0..=x1 {
					img.put_pixel(x, y, ink);
				}
			}
		};
		// Sheet border.
		line(5, 5, 594, 5);
		line(5, 394, 594, 394);
		line(5, 5, 5, 394);
		line(594, 5, 594, 394);
		// Rooms: x 100..300 and 300..500, y 100..250.
		line(100, 100, 500, 100);
		line(100, 250, 500, 250);
		line(100, 100, 100, 250);
		line(500, 100, 500, 250);
		// The shared wall, with a doorway at y 165..185.
		line(300, 100, 300, 165);
		line(300, 185, 300, 250);
		// A label: small dark block.
		line(180, 170, 220, 178);
		img
	}

	fn area(pts: &[(f64, f64)]) -> f64 {
		pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum::<f64>().abs() / 2.0
	}

	#[test]
	fn closes_the_doorway_to_get_the_room() {
		let img = plan();
		let g = Grid { w: 600, h: 400 };
		let gaps = [0.0, 12.0, 24.0, 48.0];
		let (res, ink) = fills(&img, &[(150.0, 130.0)], &gaps, 6.0, 2.0).unwrap();
		let areas: Vec<Option<f64>> = res.iter().map(|f| f.as_ref().map(|(m, _)| m.iter().filter(|&&b| b).count() as f64)).collect();
		// Open: both rooms; 24 px closes the 20 px doorway.
		let both = areas[0].unwrap();
		assert!((both - 2.0 * 199.0 * 149.0).abs() < 400.0, "both rooms {both}");
		let i = choose(&areas).unwrap();
		assert_eq!(gaps[i], 24.0);
		let ring = straighten(&g, &ink, outline(&g, &res[i].as_ref().unwrap().0));
		let a = area(&ring);
		// The room inside its walls (199 × 149), corners square, the doorway
		// closed straight across.
		let room = 199.0 * 149.0;
		assert!((a - room).abs() < room * 0.005, "room area {a} vs {room}");
		assert!(ring.iter().all(|p| p.0 <= 301.0), "past the doorway: {ring:?}");
		// The label is inside the fill, not a hole in it.
		assert!(ring.iter().all(|p| !(p.0 > 180.0 && p.0 < 221.0 && p.1 > 170.0 && p.1 < 179.0)));
	}

	#[test]
	fn a_point_on_a_line_moves_into_the_space() {
		let img = plan();
		let (res, _) = fills(&img, &[(100.0, 200.0)], &[0.0], 6.0, 2.0).unwrap();
		let m = &res[0].as_ref().unwrap().0;
		assert!(m.iter().filter(|&&b| b).count() > 1000);
	}

	#[test]
	fn two_points_fill_both_rooms_as_one() {
		let img = plan();
		let g = Grid { w: 600, h: 400 };
		let (res, _) = fills(&img, &[(150.0, 130.0), (400.0, 130.0)], &[24.0], 6.0, 2.0).unwrap();
		let a = area(&outline(&g, &res[0].as_ref().unwrap().0));
		// Both rooms and the wall between them: 399 × 149.
		let both = 399.0 * 149.0;
		assert!((a - both).abs() < both * 0.01, "{a} vs {both}");
	}

	#[test]
	fn huge_gaps_dont_overflow() {
		let img = plan();
		let _ = fills(&img, &[(100.0, 200.0)], &[1e300], 1e300, 2.0);
	}
}
