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
}
