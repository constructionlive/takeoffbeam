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

pub fn scale_units_per_px(scale: &PageScale, page_w: f64, page_h: f64) -> Option<f64> {
	let ref_px = point_distance_px(scale.p1, scale.p2, page_w, page_h);
	(ref_px > 0.0 && scale.distance > 0.0).then(|| scale.distance / ref_px)
}

/// Real-world value + unit of a measurement, or None without a usable scale.
pub fn measurement_value(
	m: &Measurement,
	scale: Option<&PageScale>,
	page_w: f64,
	page_h: f64,
) -> Option<(f64, String)> {
	let scale = scale?;
	if page_w <= 0.0 || page_h <= 0.0 || m.points.len() < 2 {
		return None;
	}
	let upp = scale_units_per_px(scale, page_w, page_h)?;
	match m.kind {
		MeasurementType::Length => Some((polyline_length_px(&m.points, page_w, page_h) * upp, scale.unit.clone())),
		MeasurementType::Area if m.points.len() >= 3 => {
			Some((polygon_area_px(&m.points, page_w, page_h) * upp * upp, format!("{}²", scale.unit)))
		}
		MeasurementType::Area => None,
	}
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

#[cfg(test)]
mod tests {
	use super::*;

	fn p(x: f64, y: f64) -> Point {
		Point { x, y }
	}

	#[test]
	fn area_and_length_use_the_page_scale() {
		// 1000×500 px page; the reference spans 100 px = 10 ft → 0.1 ft/px.
		let scale = PageScale { page: 1, p1: p(0.0, 0.0), p2: p(0.1, 0.0), distance: 10.0, unit: "ft".into() };
		let square = Measurement {
			id: "ms_1".into(),
			page: 1,
			kind: MeasurementType::Area,
			category_id: None,
			points: vec![p(0.0, 0.0), p(0.1, 0.0), p(0.1, 0.2), p(0.0, 0.2)],
			source: None,
			label: None,
			note: None,
			color: None,
			value: None,
			unit: None,
			fields: None,
		};
		let (v, u) = measurement_value(&square, Some(&scale), 1000.0, 500.0).unwrap();
		assert!((v - 100.0).abs() < 1e-9, "{v}");
		assert_eq!(u, "ft²");
		let line = Measurement { kind: MeasurementType::Length, ..square };
		let (v, _) = measurement_value(&line, Some(&scale), 1000.0, 500.0).unwrap();
		assert!((v - 30.0).abs() < 1e-9, "{v}");
	}

	#[test]
	fn unit_keys_match_like_the_platform() {
		for u in ["sq ft", "SF", "ft²", "ft2", "Sq. Ft"] {
			assert_eq!(unit_key(u), "ft2", "{u}");
		}
		assert_eq!(unit_key("EA"), "ea");
	}
}
