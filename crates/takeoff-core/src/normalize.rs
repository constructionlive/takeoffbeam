//! Make a loaded (or agent-written, or hand-edited) takeoff safe to use:
//! finite coordinates inside the page, unique ids, no dangling category ids,
//! no shapes with too few points. Never fails; drops what cannot be fixed.

use std::collections::HashSet;

use crate::model::*;

fn clamp01(v: f64) -> f64 {
	if v.is_finite() {
		v.clamp(0.0, 1.0)
	} else {
		0.0
	}
}

fn fix_points(points: &mut Vec<Point>) {
	points.retain(|p| p.x.is_finite() && p.y.is_finite());
	for p in points.iter_mut() {
		p.x = clamp01(p.x);
		p.y = clamp01(p.y);
	}
}

pub fn normalize(d: &mut TakeoffData) {
	d.review_type = "takeoff".into();
	let mut seen_cat = HashSet::new();
	d.categories.retain(|c| !c.id.is_empty() && seen_cat.insert(c.id.clone()));
	for c in &mut d.categories {
		if let Some(o) = c.opacity {
			c.opacity = Some(if o.is_finite() { o.clamp(0.15, 1.0) } else { 1.0 });
		}
	}
	let cats: HashSet<String> = d.categories.iter().map(|c| c.id.clone()).collect();
	let orphan = |id: &mut Option<String>| {
		if id.as_ref().is_some_and(|c| !cats.contains(c)) {
			*id = None;
		}
	};

	let mut seen = HashSet::new();
	d.marks.retain(|m| {
		[m.x, m.y, m.w, m.h].iter().all(|v| v.is_finite()) && m.page >= 1 && !m.id.is_empty() && seen.insert(m.id.clone())
	});
	for m in &mut d.marks {
		m.w = m.w.abs().clamp(0.0005, 1.0);
		m.h = m.h.abs().clamp(0.0005, 1.0);
		m.x = m.x.clamp(0.0, 1.0 - m.w);
		m.y = m.y.clamp(0.0, 1.0 - m.h);
		orphan(&mut m.category_id);
	}
	for m in &mut d.measurements {
		fix_points(&mut m.points);
		m.cutouts.iter_mut().for_each(fix_points);
		m.cutouts.retain(|c| c.len() >= 3);
		orphan(&mut m.category_id);
	}
	d.measurements.retain(|m| {
		let min = match m.kind {
			MeasurementType::Length => 2,
			MeasurementType::Area | MeasurementType::Angle => 3,
		};
		m.page >= 1 && m.points.len() >= min && !m.id.is_empty() && seen.insert(m.id.clone())
	});
	for a in &mut d.annotations {
		fix_points(&mut a.points);
	}
	d.annotations.retain(|a| a.page >= 1 && !a.points.is_empty() && !a.id.is_empty() && seen.insert(a.id.clone()));
	for s in &mut d.spaces {
		fix_points(&mut s.points);
		if s.name.trim().is_empty() {
			s.name = "Space".into();
		}
	}
	d.spaces.retain(|s| s.page >= 1 && s.points.len() >= 3 && !s.id.is_empty() && seen.insert(s.id.clone()));
	d.legends.retain(|l| [l.x, l.y, l.w, l.h].iter().all(|v| v.is_finite()) && l.page >= 1 && !l.id.is_empty() && seen.insert(l.id.clone()));
	for l in &mut d.legends {
		l.w = l.w.abs().clamp(0.01, 1.0);
		l.h = l.h.abs().clamp(0.01, 1.0);
		l.x = l.x.clamp(0.0, 1.0 - l.w);
		l.y = l.y.clamp(0.0, 1.0 - l.h);
		l.category_ids.retain(|c| cats.contains(c));
	}

	d.page_scales.retain(|s| s.distance.is_finite() && s.distance > 0.0 && s.page >= 1);
	let mut seen_page = HashSet::new();
	// Last calibration per page wins (later entries are newer).
	d.page_scales.reverse();
	d.page_scales.retain(|s| seen_page.insert(s.page));
	d.page_scales.reverse();
	for s in &mut d.page_scales {
		s.p1 = Point { x: clamp01(s.p1.x), y: clamp01(s.p1.y) };
		s.p2 = Point { x: clamp01(s.p2.x), y: clamp01(s.p2.y) };
	}
	d.viewports.retain(|v| v.w > 0.0 && v.h > 0.0 && v.scale.distance > 0.0);
	let mut seen_layer = HashSet::new();
	d.hidden_layers.retain(|l| seen_layer.insert(l.clone()));
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn platform_json_loads_and_cleans() {
		let raw = r##"{
			"version": 1, "reviewType": "takeoff",
			"categories": [{"id": "c1", "name": "Outlets", "color": "#4CC2FF"}],
			"marks": [
				{"id": "m1", "categoryId": "c1", "page": 1, "x": 0.98, "y": 0.5, "w": 0.05, "h": 0.05, "source": "ai", "status": "pending"},
				{"id": "m1", "page": 1, "x": 0.1, "y": 0.1, "w": 0.01, "h": 0.01, "source": "user", "status": "confirmed"},
				{"id": "m2", "categoryId": "gone", "page": 2, "x": 0.1, "y": 0.1, "w": 0.01, "h": 0.01}
			],
			"measurements": [{"id": "ms1", "page": 1, "type": "length", "points": [{"x": 0.1, "y": 0.1}]}],
			"pageScales": [{"page": 1, "p1": {"x": 0, "y": 0}, "p2": {"x": 0.1, "y": 0}, "distance": 10, "unit": "ft"}]
		}"##;
		let mut d: TakeoffData = serde_json::from_str(raw).unwrap();
		normalize(&mut d);
		assert_eq!(d.marks.len(), 2);
		assert!((d.marks[0].x - 0.95).abs() < 1e-12);
		assert_eq!(d.marks[1].category_id, None);
		assert!(d.measurements.is_empty());
		assert_eq!(d.page_scales.len(), 1);
		// Round-trips without inventing fields the platform doesn't know.
		let out = serde_json::to_value(&d).unwrap();
		assert!(out.get("annotations").is_none());
		assert_eq!(out["marks"][0]["categoryId"], "c1");
	}
}
