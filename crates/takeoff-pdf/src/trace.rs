//! Following a drawn line in the picture of a page, for sheets with no
//! vector line work to follow: scans, flattened PDFs, alignments drawn over
//! aerial photos. The line's colour is read where it starts; from there the
//! tracker steps along pixels of that colour, jumping the gaps of dashed
//! lines and keeping to the middle of the line, much as `SnapIndex::track`
//! does with vector segments.

use image::RgbaImage;
use takeoff_core::Point;

use crate::{Drawing, FracRect, PdfError};

/// Distances in points of paper (scaled by the render's px per point).
const SNAP: f64 = 8.0;
const TOL: f64 = 3.0;
const GAP: f64 = 18.0;
/// Render resolution for tracing, and the most pixels a side.
const PX_PER_PT: f64 = 4.0;
const MAX_EDGE: u32 = 6000;

type P = (f64, f64);

fn sub(a: P, b: P) -> P {
	(a.0 - b.0, a.1 - b.1)
}

fn len(a: P) -> f64 {
	a.0.hypot(a.1)
}

fn unit(a: P) -> P {
	let l = len(a).max(1e-12);
	(a.0 / l, a.1 / l)
}

fn color(img: &RgbaImage, p: P) -> Option<[f64; 3]> {
	let (x, y) = (p.0.round() as i64, p.1.round() as i64);
	if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
		return None;
	}
	let c = img.get_pixel(x as u32, y as u32).0;
	Some([c[0] as f64, c[1] as f64, c[2] as f64])
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
	((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Colours across the line at `at` (perpendicular `n`), every half pixel from -r to r.
fn profile(img: &RgbaImage, at: P, n: P, r: f64) -> Vec<(f64, [f64; 3])> {
	let mut out = Vec::new();
	let mut t = -r;
	while t <= r {
		if let Some(c) = color(img, (at.0 + n.0 * t, at.1 + n.1 * t)) {
			out.push((t, c));
		}
		t += 0.5;
	}
	out
}

/// Where along a profile the line is: the middle of the run of matching
/// pixels nearest the centre.
fn line_in(profile: &[(f64, [f64; 3])], ink: [f64; 3], thr: f64) -> Option<f64> {
	let mut best: Option<f64> = None;
	let mut run: Option<(f64, f64)> = None;
	let close = |run: (f64, f64), best: &mut Option<f64>| {
		let mid = (run.0 + run.1) / 2.0;
		if best.is_none_or(|b| mid.abs() < b.abs()) {
			*best = Some(mid);
		}
	};
	for (t, c) in profile {
		if dist(*c, ink) <= thr {
			run = Some(run.map_or((*t, *t), |r| (r.0, *t)));
		} else if let Some(r) = run.take() {
			close(r, &mut best);
		}
	}
	if let Some(r) = run {
		close(r, &mut best);
	}
	best
}

/// Pixel centres wobble a little from step to step, which adds length:
/// average each point with its neighbours (ends kept), then drop the points
/// a straight run doesn't need.
fn steady(path: &[P], ppt: f64) -> Vec<P> {
	let n = path.len();
	if n < 5 {
		return path.to_vec();
	}
	let k = 3usize;
	let mut out = Vec::with_capacity(n);
	for i in 0..n {
		let r = k.min(i).min(n - 1 - i);
		let (mut x, mut y) = (0.0, 0.0);
		for p in &path[i - r..=i + r] {
			x += p.0;
			y += p.1;
		}
		let c = (2 * r + 1) as f64;
		out.push((x / c, y / c));
	}
	let pts: Vec<Point> = out.iter().map(|p| Point { x: p.0, y: p.1 }).collect();
	takeoff_core::geometry::simplify(&pts, 0.3 * ppt, 1.0, 1.0).into_iter().map(|p| (p.x, p.y)).collect()
}

fn step_of(ppt: f64) -> f64 {
	ppt.max(1.0)
}

/// Follow the line from near `a` to near `b` in `img` (image px). `ppt`:
/// image px per point of paper. The path traced, or None when no line of one
/// colour joins them.
pub fn track_image(img: &RgbaImage, a: P, b: P, ppt: f64) -> Option<Vec<P>> {
	let (snap, tol, gap) = (SNAP * ppt, TOL * ppt, GAP * ppt);
	let chord = sub(b, a);
	if len(chord) < ppt {
		return Some(vec![a, b]);
	}
	let mut dir = unit(chord);
	let normal = |d: P| (-d.1, d.0);
	// Where the line starts, and its colour: across the line at the start
	// (or a little on, if the start falls in a dash's gap), the run of
	// stand-out pixels nearest the point asked for.
	let n0 = normal(dir);
	let mut start: Option<(f64, f64, [f64; 3], f64)> = None;
	let mut s = 0.0;
	while s <= gap {
		let at = (a.0 + dir.0 * s, a.1 + dir.1 * s);
		let across = profile(img, at, n0, snap);
		if across.len() >= 4 {
			let median = {
				let mut ch: Vec<[f64; 3]> = across.iter().map(|x| x.1).collect();
				let mut m = [0.0; 3];
				for (k, v) in m.iter_mut().enumerate() {
					ch.sort_by(|x, y| x[k].total_cmp(&y[k]));
					*v = ch[ch.len() / 2][k];
				}
				m
			};
			// The stand-out pixel nearest the centre, then the middle of its run.
			if let Some((t, c)) = across.iter().filter(|(_, c)| dist(*c, median) >= 40.0).min_by(|x, y| x.0.abs().total_cmp(&y.0.abs())).map(|(t, c)| (*t, *c)) {
				let contrast = dist(c, median);
				let thr = (contrast * 0.45).max(28.0);
				let mid = line_in(&across.iter().map(|(u, k)| (*u - t, *k)).collect::<Vec<_>>(), c, thr).map_or(t, |m| t + m);
				// A drawn line is a narrow stroke; a wide patch is photo or fill.
				let width = across.iter().filter(|(u, k)| (*u - mid).abs() <= 4.0 * ppt && dist(*k, c) <= thr).count() as f64 * 0.5;
				if width <= 3.0 * ppt && start.is_none_or(|b| mid.abs() + 0.5 < b.1.abs()) {
					start = Some((s, mid, c, contrast));
				}
			}
		}
		if start.is_some_and(|b| b.1.abs() <= ppt) {
			break;
		}
		s += step_of(ppt);
	}
	let (s0, t0, mut ink, contrast) = start?;
	let thr = (contrast * 0.45).max(28.0);
	let begin = (a.0 + dir.0 * s0 + n0.0 * t0, a.1 + dir.1 * s0 + n0.1 * t0);
	let mut cur = begin;
	let step = step_of(ppt);
	// Start opposite the point asked for, even when the line was found a little on.
	let mut path = vec![(begin.0 - dir.0 * s0, begin.1 - dir.1 * s0)];
	if s0 > 0.0 {
		path.push(cur);
	}
	let mut travelled = 0.0;
	let total = len(chord);
	let finish = |mut path: Vec<P>, cur: P, dir: P| {
		let t = ((b.0 - cur.0) * dir.0 + (b.1 - cur.1) * dir.1).clamp(0.0, gap);
		path.push((cur.0 + dir.0 * t, cur.1 + dir.1 * t));
		steady(&path, ppt)
	};
	for _ in 0..400_000 {
		let to_b = sub(b, cur);
		let left = len(to_b);
		let ahead = to_b.0 * dir.0 + to_b.1 * dir.1;
		if left <= step * 1.5 || (ahead < 0.0 && left <= gap) {
			return Some(finish(path, cur, dir));
		}
		let mut found = None;
		let mut k = 1.0;
		while found.is_none() && k * step <= gap.max(step) {
			let pred = (cur.0 + dir.0 * step * k, cur.1 + dir.1 * step * k);
			let n = normal(dir);
			if let Some(t) = line_in(&profile(img, pred, n, tol), ink, thr) {
				found = Some((pred.0 + n.0 * t, pred.1 + n.1 * t));
			}
			k += 1.0;
		}
		let Some(q) = found else {
			if left <= gap {
				return Some(finish(path, cur, dir));
			}
			return None;
		};
		let mv = sub(q, cur);
		let l = len(mv);
		if l < 1e-6 {
			return None;
		}
		let u = unit(mv);
		dir = unit((dir.0 * 0.6 + u.0 * 0.4, dir.1 * 0.6 + u.1 * 0.4));
		if let Some(c) = color(img, q) {
			if dist(c, ink) <= thr {
				ink = [ink[0] * 0.95 + c[0] * 0.05, ink[1] * 0.95 + c[1] * 0.05, ink[2] * 0.95 + c[2] * 0.05];
			}
		}
		cur = q;
		path.push(cur);
		travelled += l;
		if travelled > total * 3.0 + 60.0 * ppt {
			return None;
		}
	}
	None
}

impl Drawing {
	/// Follow the line drawn from near `a` to near `b` (page fractions) in a
	/// render of that part of the page. None when no single line joins them.
	pub fn track_raster(&self, page: u32, a: Point, b: Point) -> Result<Option<Vec<Point>>, PdfError> {
		let (w, h) = self.page_size_pt(page)?;
		let (w, h) = (w as f64, h as f64);
		if !(w > 0.0 && h > 0.0) {
			return Ok(None); // a page with no size (a broken file)
		}
		let pad = (GAP * 2.0) / w.min(h);
		let (x0, y0) = ((a.x.min(b.x) - pad).max(0.0), (a.y.min(b.y) - pad).max(0.0));
		let (x1, y1) = ((a.x.max(b.x) + pad).min(1.0), (a.y.max(b.y) + pad).min(1.0));
		let region = FracRect { x: x0, y: y0, w: (x1 - x0).max(1e-4), h: (y1 - y0).max(1e-4) };
		let long_pt = (region.w * w).max(region.h * h);
		let ppt = PX_PER_PT.min(MAX_EDGE as f64 / long_pt.max(1.0));
		let r = self.render(page, (ppt * 72.0) as f32, region, Some(MAX_EDGE))?;
		let (pw, ph) = (r.page_px.0 as f64, r.page_px.1 as f64);
		let to_px = |p: Point| ((p.x - r.region.x) * pw, (p.y - r.region.y) * ph);
		let ppt = pw / w;
		let Some(path) = track_image(&r.image, to_px(a), to_px(b), ppt) else { return Ok(None) };
		let pts: Vec<Point> = path.into_iter().map(|(x, y)| Point { x: x / pw + r.region.x, y: y / ph + r.region.y }).collect();
		let pts = takeoff_core::geometry::simplify(&pts, 0.35, w, h);
		Ok(takeoff_core::geometry::plausible_trace(&pts, a, b, w, h).then_some(pts))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// An aerial-ish background (noisy greys and greens) with a dashed blue
	/// quarter circle, a second blue arc 12 px outside it, and a dark road across.
	fn picture() -> RgbaImage {
		let (w, h) = (900u32, 900u32);
		let mut img = RgbaImage::new(w, h);
		let mut seed = 7u64;
		let mut rnd = || {
			seed ^= seed << 13;
			seed ^= seed >> 7;
			seed ^= seed << 17;
			(seed % 1000) as f64 / 1000.0
		};
		for y in 0..h {
			for x in 0..w {
				let g = 150.0 + 60.0 * ((x as f64 / 37.0).sin() * (y as f64 / 53.0).cos()) + 30.0 * rnd();
				let green = if (x / 120 + y / 90) % 3 == 0 { 25.0 } else { 0.0 };
				img.put_pixel(x, y, image::Rgba([g as u8, (g + green).min(255.0) as u8, (g * 0.9) as u8, 255]));
			}
		}
		let (cx, cy) = (100.0, 100.0);
		for (r, dashed) in [(600.0, true), (612.0, false)] {
			let n = 6000;
			for k in 0..n {
				let a = k as f64 / n as f64 * std::f64::consts::FRAC_PI_2;
				// Dashes: 24 px drawn, 12 px blank along the arc.
				if dashed && ((a * r) as i64 % 36) >= 24 {
					continue;
				}
				for dr in -2..=2 {
					let (x, y) = (cx + (r + dr as f64 * 0.5) * a.cos(), cy + (r + dr as f64 * 0.5) * a.sin());
					img.put_pixel(x as u32, y as u32, image::Rgba([40, 60, 200, 255]));
				}
			}
		}
		for t in 0..900 {
			for d in -3..=3 {
				let (x, y) = (t as i64 + d, t as i64);
				if (0..900).contains(&x) {
					img.put_pixel(x as u32, y as u32, image::Rgba([45, 45, 50, 255]));
				}
			}
		}
		img
	}

	#[test]
	fn follows_a_dashed_line_in_a_picture() {
		let img = picture();
		let at = |deg: f64, r: f64| {
			let a = deg.to_radians();
			(100.0 + r * a.cos(), 100.0 + r * a.sin())
		};
		// ppt = 2: tolerances double in px.
		let path = track_image(&img, at(3.0, 602.0), at(87.0, 598.0), 2.0).expect("traced");
		let l: f64 = path.windows(2).map(|w| len(sub(w[1], w[0]))).sum();
		let arc = 600.0 * 84.0_f64.to_radians();
		assert!((l - arc).abs() < arc * 0.01, "length {l:.1} vs {arc:.1}");
		for p in &path {
			let r = len(sub(*p, (100.0, 100.0)));
			// (The ends sit opposite the points asked for, which were 2 px off it.)
			assert!((r - 600.0).abs() < 3.5, "off the line: r = {r:.1}");
		}
		// Nothing of one colour joins the line to bare ground.
		assert!(track_image(&img, at(3.0, 600.0), (850.0, 120.0), 2.0).is_none());
	}
}
