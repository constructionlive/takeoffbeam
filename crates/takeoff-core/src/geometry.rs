//! Measurement maths over page fractions. Callers pass the page size in px
//! (any consistent render size): the scale reference and the measured shape
//! are converted with the SAME size, so real-world values are DPI-invariant.

use crate::model::{Measurement, MeasurementType, PageScale, Point};

pub fn point_distance_px(a: Point, b: Point, page_w: f64, page_h: f64) -> f64 {
	((a.x - b.x) * page_w).hypot((a.y - b.y) * page_h)
}

pub fn polyline_length_px(points: &[Point], page_w: f64, page_h: f64) -> f64 {
	points.windows(2).map(|w| point_distance_px(w[0], w[1], page_w, page_h)).sum()
}

pub fn perimeter_px(points: &[Point], page_w: f64, page_h: f64) -> f64 {
	let open = polyline_length_px(points, page_w, page_h);
	match (points.first(), points.last()) {
		(Some(a), Some(b)) if points.len() > 2 => open + point_distance_px(*b, *a, page_w, page_h),
		_ => open,
	}
}

/// Shoelace formula in px (polygon implicitly closed).
pub fn polygon_area_px(points: &[Point], page_w: f64, page_h: f64) -> f64 {
	let n = points.len();
	let mut sum = 0.0;
	for i in 0..n {
		let a = points[i];
		let b = points[(i + 1) % n];
		sum += (a.x * page_w) * (b.y * page_h) - (b.x * page_w) * (a.y * page_h);
	}
	(sum / 2.0).abs()
}

/// Interior angle at `vertex` in degrees (0–180).
pub fn angle_deg(a: Point, vertex: Point, b: Point, page_w: f64, page_h: f64) -> f64 {
	let (ax, ay) = ((a.x - vertex.x) * page_w, (a.y - vertex.y) * page_h);
	let (bx, by) = ((b.x - vertex.x) * page_w, (b.y - vertex.y) * page_h);
	let dot = ax * bx + ay * by;
	let cross = ax * by - ay * bx;
	cross.atan2(dot).abs().to_degrees()
}

pub fn scale_units_per_px(scale: &PageScale, page_w: f64, page_h: f64) -> Option<f64> {
	let ref_px = point_distance_px(scale.p1, scale.p2, page_w, page_h);
	(ref_px > 0.0 && scale.distance > 0.0).then(|| scale.distance / ref_px)
}

/// What a measurement amounts to. `value`/`unit` is the headline quantity
/// (what the platform stores); the rest are the extra Bluebeam columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Quantity {
	pub value: f64,
	pub unit: String,
	/// Areas: the outline length.
	pub perimeter: Option<f64>,
	/// Areas with a depth: area × depth, in unit³.
	pub volume: Option<f64>,
	/// Length unit the extras are in.
	pub base_unit: String,
}

/// Real-world quantity of a measurement under `scale`, or None without a
/// usable scale (angles need no scale).
pub fn measurement_quantity(m: &Measurement, scale: Option<&PageScale>, page_w: f64, page_h: f64) -> Option<Quantity> {
	if page_w <= 0.0 || page_h <= 0.0 || m.points.len() < 2 {
		return None;
	}
	if m.kind == MeasurementType::Angle {
		let [a, v, b] = m.points[..] else { return None };
		return Some(Quantity {
			value: angle_deg(a, v, b, page_w, page_h),
			unit: "°".into(),
			perimeter: None,
			volume: None,
			base_unit: "°".into(),
		});
	}
	let scale = scale?;
	let upp = scale_units_per_px(scale, page_w, page_h)?;
	let unit = scale.unit.clone();
	match m.kind {
		MeasurementType::Length => {
			let px = if m.closed { perimeter_px(&m.points, page_w, page_h) } else { polyline_length_px(&m.points, page_w, page_h) };
			Some(Quantity { value: px * upp, unit: unit.clone(), perimeter: None, volume: None, base_unit: unit })
		}
		MeasurementType::Area => {
			if m.points.len() < 3 {
				return None;
			}
			let holes: f64 = m.cutouts.iter().filter(|c| c.len() >= 3).map(|c| polygon_area_px(c, page_w, page_h)).sum();
			let area = (polygon_area_px(&m.points, page_w, page_h) - holes).max(0.0) * upp * upp;
			let perimeter = perimeter_px(&m.points, page_w, page_h) * upp;
			Some(Quantity {
				value: area,
				unit: format!("{unit}²"),
				perimeter: Some(perimeter),
				volume: m.depth.filter(|d| *d > 0.0).map(|d| area * d),
				base_unit: unit,
			})
		}
		MeasurementType::Angle => unreachable!(),
	}
}

/// Platform-compatible shorthand: headline value + unit.
pub fn measurement_value(m: &Measurement, scale: Option<&PageScale>, page_w: f64, page_h: f64) -> Option<(f64, String)> {
	measurement_quantity(m, scale, page_w, page_h).map(|q| (q.value, q.unit))
}

pub fn format_measurement_value(value: f64, unit: &str) -> String {
	if value >= 100.0 {
		format!("{value:.1} {unit}")
	} else {
		format!("{value:.2} {unit}")
	}
}

/// Normalize a unit label for comparisons: "sq ft", "SF", "ft²", "ft2" are equal.
pub fn unit_key(unit: &str) -> String {
	let mut u: String = unit.to_lowercase().chars().filter(|c| *c != '.' && !c.is_whitespace()).collect();
	if let Some(rest) = u.strip_prefix("sq") {
		u = format!("{rest}2");
	}
	if let Some(rest) = u.strip_suffix('²') {
		u = format!("{rest}2");
	}
	match u.as_str() {
		"sf" => "ft2".into(),
		"sm" => "m2".into(),
		"each" => "ea".into(),
		_ => u,
	}
}

/// Distance from `p` to segment `a`–`b`, in px.
pub fn point_segment_distance_px(p: Point, a: Point, b: Point, page_w: f64, page_h: f64) -> f64 {
	let (px, py) = (p.x * page_w, p.y * page_h);
	let (ax, ay) = (a.x * page_w, a.y * page_h);
	let (bx, by) = (b.x * page_w, b.y * page_h);
	let (dx, dy) = (bx - ax, by - ay);
	let len2 = dx * dx + dy * dy;
	let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
	(px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// Even-odd point-in-polygon test (fractions).
pub fn point_in_polygon(p: Point, poly: &[Point]) -> bool {
	let mut inside = false;
	let n = poly.len();
	if n < 3 {
		return false;
	}
	let mut j = n - 1;
	for i in 0..n {
		let (a, b) = (poly[i], poly[j]);
		if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
			inside = !inside;
		}
		j = i;
	}
	inside
}

/// Axis-aligned bounds (x, y, w, h) of points, in fractions.
pub fn bounds(points: &[Point]) -> Option<(f64, f64, f64, f64)> {
	let first = points.first()?;
	let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
	for p in points {
		x0 = x0.min(p.x);
		y0 = y0.min(p.y);
		x1 = x1.max(p.x);
		y1 = y1.max(p.y);
	}
	Some((x0, y0, x1 - x0, y1 - y0))
}

/// Intersection over union of two (x, y, w, h) boxes.
pub fn iou(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
	let ix = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
	let iy = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
	if ix <= 0.0 || iy <= 0.0 {
		return 0.0;
	}
	let inter = ix * iy;
	inter / (a.2 * a.3 + b.2 * b.3 - inter)
}

/// Whether a traced run from `a` to `b` is believable: it stays near the
/// straight path between them and isn't much longer. A curve's bend between
/// two points placed on it is small; a run that wanders off along something
/// else (photo texture, another line) is not.
pub fn plausible_trace(run: &[Point], a: Point, b: Point, page_w: f64, page_h: f64) -> bool {
	let chord = point_distance_px(a, b, page_w, page_h);
	let len = polyline_length_px(run, page_w, page_h);
	// A quarter circle between its ends bows out 21% of the span; allow a bit more.
	let reach = (chord * 0.3).max(12.0);
	// It must also reach from one to the other: never shorter than the straight line.
	if len < chord * 0.97 - 2.0 || len > chord * 1.25 + 4.0 || run.iter().any(|p| point_segment_distance_px(*p, a, b, page_w, page_h) > reach) {
		return false;
	}
	// A drawn curve bends steadily; a run picking its way through texture
	// turns back and forth. Compare the turning along it (every ~6 px) with
	// how far its heading turns overall.
	let pts: Vec<(f64, f64)> = resample(run, 6.0, page_w, page_h);
	if pts.len() < 3 {
		return true;
	}
	let heading = |p: (f64, f64), q: (f64, f64)| (q.1 - p.1).atan2(q.0 - p.0);
	let wrap = |d: f64| (d + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
	let hs: Vec<f64> = pts.windows(2).map(|w| heading(w[0], w[1])).collect();
	let turns: Vec<f64> = hs.windows(2).map(|w| wrap(w[1] - w[0])).collect();
	let total: f64 = turns.iter().map(|t| t.abs()).sum();
	let net: f64 = turns.iter().sum::<f64>().abs();
	(total - net).to_degrees() <= 25.0 + len / 40.0
}

/// Points every `step_px` along a polyline (px coordinates).
fn resample(run: &[Point], step_px: f64, page_w: f64, page_h: f64) -> Vec<(f64, f64)> {
	let px: Vec<(f64, f64)> = run.iter().map(|p| (p.x * page_w, p.y * page_h)).collect();
	let mut out = vec![];
	let Some(&first) = px.first() else { return out };
	out.push(first);
	let mut carry = 0.0;
	for w in px.windows(2) {
		let (a, b) = (w[0], w[1]);
		let l = (b.0 - a.0).hypot(b.1 - a.1);
		let mut t = step_px - carry;
		while t <= l {
			out.push((a.0 + (b.0 - a.0) * t / l, a.1 + (b.1 - a.1) * t / l));
			t += step_px;
		}
		carry = (carry + l) % step_px;
	}
	out
}

/// Douglas–Peucker: drop vertices that sit within `tol_px` of the line
/// through their neighbours (keeps a dense traced curve light to store and edit).
pub fn simplify(points: &[Point], tol_px: f64, page_w: f64, page_h: f64) -> Vec<Point> {
	if points.len() <= 2 {
		return points.to_vec();
	}
	let mut keep = vec![false; points.len()];
	keep[0] = true;
	keep[points.len() - 1] = true;
	let mut stack = vec![(0usize, points.len() - 1)];
	while let Some((a, b)) = stack.pop() {
		let (mut far, mut at) = (0.0, a);
		for k in a + 1..b {
			let d = point_segment_distance_px(points[k], points[a], points[b], page_w, page_h);
			if d > far {
				far = d;
				at = k;
			}
		}
		if far > tol_px {
			keep[at] = true;
			stack.push((a, at));
			stack.push((at, b));
		}
	}
	points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// A smooth curve through `points` (centripetal Catmull–Rom, which never
/// loops or overshoots between close points), as a dense polyline: every
/// given point stays on it. `closed` joins the last point back to the first.
pub fn smooth_path(points: &[Point], closed: bool, page_w: f64, page_h: f64) -> Vec<Point> {
	let n = points.len();
	if n < 3 {
		return points.to_vec();
	}
	let px = |p: Point| (p.x * page_w, p.y * page_h);
	let get = |i: isize| -> (f64, f64) {
		if closed {
			px(points[i.rem_euclid(n as isize) as usize])
		} else if i < 0 {
			// Carry the bend on past the ends (the parabola through the last
			// three points), so a few points on an arc read as one steady curve.
			let (a, b, c) = (px(points[0]), px(points[1]), px(points[2]));
			(3.0 * a.0 - 3.0 * b.0 + c.0, 3.0 * a.1 - 3.0 * b.1 + c.1)
		} else if i as usize >= n {
			let (a, b, c) = (px(points[n - 1]), px(points[n - 2]), px(points[n - 3]));
			(3.0 * a.0 - 3.0 * b.0 + c.0, 3.0 * a.1 - 3.0 * b.1 + c.1)
		} else {
			px(points[i as usize])
		}
	};
	let spans = if closed { n } else { n - 1 };
	let mut out = vec![points[0]];
	for i in 0..spans as isize {
		let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
		let knot = |a: (f64, f64), b: (f64, f64)| ((b.0 - a.0).hypot(b.1 - a.1)).sqrt().max(1e-6);
		let (t1, t2, t3) = (knot(p0, p1), knot(p1, p2), knot(p2, p3));
		let (t1, t2, t3) = (t1, t1 + t2, t1 + t2 + t3);
		let chord = (p2.0 - p1.0).hypot(p2.1 - p1.1);
		let steps = ((chord / 2.0).ceil() as usize).clamp(4, 96);
		for k in 1..=steps {
			let t = t1 + (t2 - t1) * k as f64 / steps as f64;
			let lerp = |a: (f64, f64), b: (f64, f64), ta: f64, tb: f64| {
				let (wa, wb) = ((tb - t) / (tb - ta), (t - ta) / (tb - ta));
				(a.0 * wa + b.0 * wb, a.1 * wa + b.1 * wb)
			};
			let a1 = lerp(p0, p1, 0.0, t1);
			let a2 = lerp(p1, p2, t1, t2);
			let a3 = lerp(p2, p3, t2, t3);
			let b1 = lerp(a1, a2, 0.0, t2);
			let b2 = lerp(a2, a3, t1, t3);
			let c = lerp(b1, b2, t1, t2);
			out.push(Point { x: c.0 / page_w, y: c.1 / page_h });
		}
	}
	if closed {
		out.pop();
	}
	simplify(&out, 0.15, page_w, page_h)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn p(x: f64, y: f64) -> Point {
		Point { x, y }
	}

	fn scale() -> PageScale {
		// 1000×500 px page; the reference spans 100 px = 10 ft → 0.1 ft/px.
		PageScale { page: 1, p1: p(0.0, 0.0), p2: p(0.1, 0.0), distance: 10.0, unit: "ft".into() }
	}

	#[test]
	fn area_and_length_use_the_page_scale() {
		let square = Measurement::new("ms_1", 1, MeasurementType::Area, vec![p(0.0, 0.0), p(0.1, 0.0), p(0.1, 0.2), p(0.0, 0.2)]);
		let (v, u) = measurement_value(&square, Some(&scale()), 1000.0, 500.0).unwrap();
		assert!((v - 100.0).abs() < 1e-9, "{v}");
		assert_eq!(u, "ft²");
		let line = Measurement { kind: MeasurementType::Length, ..square.clone() };
		let (v, _) = measurement_value(&line, Some(&scale()), 1000.0, 500.0).unwrap();
		assert!((v - 30.0).abs() < 1e-9, "{v}");
		let perimeter = Measurement { closed: true, ..line };
		let (v, _) = measurement_value(&perimeter, Some(&scale()), 1000.0, 500.0).unwrap();
		assert!((v - 40.0).abs() < 1e-9, "{v}");
	}

	#[test]
	fn cutouts_and_depth() {
		let mut m = Measurement::new("ms_1", 1, MeasurementType::Area, vec![p(0.0, 0.0), p(0.1, 0.0), p(0.1, 0.2), p(0.0, 0.2)]);
		m.cutouts = vec![vec![p(0.0, 0.0), p(0.05, 0.0), p(0.05, 0.1), p(0.0, 0.1)]];
		m.depth = Some(0.5);
		let q = measurement_quantity(&m, Some(&scale()), 1000.0, 500.0).unwrap();
		assert!((q.value - 75.0).abs() < 1e-9, "{}", q.value);
		assert!((q.volume.unwrap() - 37.5).abs() < 1e-9);
		assert!((q.perimeter.unwrap() - 40.0).abs() < 1e-9);
	}

	#[test]
	fn right_angle() {
		let m = Measurement::new("a", 1, MeasurementType::Angle, vec![p(0.5, 0.0), p(0.0, 0.0), p(0.0, 0.5)]);
		let q = measurement_quantity(&m, None, 1000.0, 1000.0).unwrap();
		assert!((q.value - 90.0).abs() < 1e-9);
	}

	#[test]
	fn unit_keys_match_like_the_platform() {
		for u in ["sq ft", "SF", "ft²", "ft2", "Sq. Ft"] {
			assert_eq!(unit_key(u), "ft2", "{u}");
		}
		assert_eq!(unit_key("EA"), "ea");
	}

	#[test]
	fn smooth_paths_pass_through_their_points_and_follow_an_arc() {
		// Five points on a quarter circle of radius 100 (px = fractions × 1000).
		let (w, h) = (1000.0, 1000.0);
		let pts: Vec<Point> = (0..5).map(|k| {
			let a = k as f64 / 4.0 * std::f64::consts::FRAC_PI_2;
			Point { x: (200.0 + 100.0 * a.cos()) / w, y: (200.0 + 100.0 * a.sin()) / h }
		}).collect();
		let curve = smooth_path(&pts, false, w, h);
		for p in &pts {
			assert!(curve.iter().any(|q| point_distance_px(*p, *q, w, h) < 1e-6), "passes through {p:?}");
		}
		let arc = 100.0 * std::f64::consts::FRAC_PI_2;
		let straight = polyline_length_px(&pts, w, h);
		let smooth = polyline_length_px(&curve, w, h);
		assert!((smooth - arc).abs() < (straight - arc).abs() / 2.0, "smooth {smooth:.2} vs arc {arc:.2} (straight {straight:.2})");
		// Every vertex of the curve lies on the circle, within a fraction of a px.
		for q in &curve {
			let r = ((q.x * w - 200.0).hypot(q.y * h - 200.0) - 100.0).abs();
			// A straight chord between these points sags 1.9 px.
			assert!(r < 1.0, "{r}");
		}
		// Simplifying a straight run keeps its ends only.
		let line: Vec<Point> = (0..10).map(|k| Point { x: k as f64 / 10.0, y: 0.5 }).collect();
		assert_eq!(simplify(&line, 0.1, w, h).len(), 2);
	}
}
