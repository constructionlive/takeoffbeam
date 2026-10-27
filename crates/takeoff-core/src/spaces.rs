//! Spaces: named regions of a page that quantities are attributed to.
//!
//! - A count belongs to the space around its centre, an angle to the space
//!   around its vertex, a markup to the space around the centre of its bounds.
//! - An area belongs to the space around an interior point: its centroid, or
//!   the middle of the widest span across it at that height when the
//!   centroid falls outside (an L-shaped room).
//! - A length belongs to the deepest space holding more than half of it,
//!   else to the space holding the most of it. It is never split, so totals
//!   are the same with or without spaces.
//! - Edges count as inside. Spaces nest by geometry: the smallest space
//!   around a point is the deepest one and wins (ties: the later one), and
//!   the spaces around it give its path (Level 2 › Unit 204 › Kitchen).

use std::collections::HashMap;

use crate::model::*;

const EPS: f64 = 1e-9;

/// Inside or on the edge of the polygon `poly` (implicitly closed).
pub fn contains(poly: &[Point], p: Point) -> bool {
	let n = poly.len();
	if n < 3 {
		return false;
	}
	let mut inside = false;
	for i in 0..n {
		let (a, b) = (poly[i], poly[(i + 1) % n]);
		if on_segment(p, a, b) {
			return true;
		}
		if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
			inside = !inside;
		}
	}
	inside
}

fn on_segment(p: Point, a: Point, b: Point) -> bool {
	let (dx, dy) = (b.x - a.x, b.y - a.y);
	let len = dx.hypot(dy);
	if len < EPS {
		return (p.x - a.x).hypot(p.y - a.y) <= EPS;
	}
	let dot = (p.x - a.x) * dx + (p.y - a.y) * dy;
	((p.x - a.x) * dy - (p.y - a.y) * dx).abs() <= EPS * len && dot >= -EPS * len && dot <= len * len + EPS * len
}

/// Signed area in page fractions (shoelace).
fn signed_area(poly: &[Point]) -> f64 {
	(0..poly.len()).map(|i| {
		let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
		a.x * b.y - b.x * a.y
	}).sum::<f64>() / 2.0
}

/// Area-weighted centroid (vertex mean for a degenerate polygon).
pub fn centroid(poly: &[Point]) -> Option<Point> {
	if poly.is_empty() {
		return None;
	}
	let a = signed_area(poly);
	if a.abs() < 1e-15 {
		let n = poly.len() as f64;
		return Some(Point { x: poly.iter().map(|p| p.x).sum::<f64>() / n, y: poly.iter().map(|p| p.y).sum::<f64>() / n });
	}
	let (mut cx, mut cy) = (0.0, 0.0);
	for i in 0..poly.len() {
		let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
		let k = p.x * q.y - q.x * p.y;
		cx += (p.x + q.x) * k;
		cy += (p.y + q.y) * k;
	}
	Some(Point { x: cx / (6.0 * a), y: cy / (6.0 * a) })
}

/// A point inside the polygon: its centroid, or the middle of the widest
/// span across it at the centroid's height.
pub fn interior_point(poly: &[Point]) -> Option<Point> {
	let c = centroid(poly)?;
	if poly.len() < 3 || contains(poly, c) {
		return Some(c);
	}
	let n = poly.len();
	let mut xs: Vec<f64> = (0..n)
		.filter_map(|i| {
			let (a, b) = (poly[i], poly[(i + 1) % n]);
			((a.y > c.y) != (b.y > c.y)).then(|| a.x + (c.y - a.y) / (b.y - a.y) * (b.x - a.x))
		})
		.collect();
	xs.sort_by(|a, b| a.total_cmp(b));
	xs.chunks_exact(2).max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0]))).map(|s| Point { x: (s[0] + s[1]) / 2.0, y: c.y }).or(Some(poly[0]))
}

/// Where segment a–b crosses segment c–d, as t along a–b (parallel: none).
fn crossing(a: Point, b: Point, c: Point, d: Point) -> Option<f64> {
	let (rx, ry, sx, sy) = (b.x - a.x, b.y - a.y, d.x - c.x, d.y - c.y);
	let den = rx * sy - ry * sx;
	if den.abs() < 1e-15 {
		return None;
	}
	let (qx, qy) = (c.x - a.x, c.y - a.y);
	let t = (qx * sy - qy * sx) / den;
	let u = (qx * ry - qy * rx) / den;
	((0.0..=1.0).contains(&t) && (-EPS..=1.0 + EPS).contains(&u)).then_some(t)
}

struct Entry<'a> {
	space: &'a Space,
	/// Paper area (page px²), for "smallest is deepest".
	area: f64,
	bbox: (f64, f64, f64, f64),
	index: usize,
}

/// The document's spaces, ready to attribute items.
pub struct Spaces<'a> {
	list: Vec<Entry<'a>>,
	sizes: HashMap<u32, (f64, f64)>,
}

impl<'a> Spaces<'a> {
	pub fn new(d: &'a TakeoffData) -> Self {
		let sizes: HashMap<u32, (f64, f64)> = d.pages.iter().map(|p| (p.page, (p.width_px.max(1.0), p.height_px.max(1.0)))).collect();
		let list = d
			.spaces
			.iter()
			.enumerate()
			.filter(|(_, s)| s.points.len() >= 3)
			.map(|(index, space)| {
				let (w, h) = sizes.get(&space.page).copied().unwrap_or((1.0, 1.0));
				let bbox = crate::geometry::bounds(&space.points).unwrap_or_default();
				Entry { space, area: signed_area(&space.points).abs() * w * h, bbox, index }
			})
			.collect();
		Self { list, sizes }
	}

	pub fn is_empty(&self) -> bool {
		self.list.is_empty()
	}

	pub fn get(&self, id: &str) -> Option<&'a Space> {
		self.list.iter().find(|e| e.space.id == id).map(|e| e.space)
	}

	fn entry(&self, id: &str) -> Option<&Entry<'a>> {
		self.list.iter().find(|e| e.space.id == id)
	}

	/// Outermost first: bigger area, then earlier in the list (equal areas within rounding).
	fn deeper(a: &Entry, b: &Entry) -> std::cmp::Ordering {
		if (a.area - b.area).abs() <= 1e-9 * a.area.max(b.area) {
			a.index.cmp(&b.index)
		} else {
			b.area.total_cmp(&a.area)
		}
	}

	fn around_entries(&self, page: u32, p: Point) -> Vec<&Entry<'a>> {
		let mut v: Vec<&Entry> = self
			.list
			.iter()
			.filter(|e| e.space.page == page && p.x >= e.bbox.0 - EPS && p.x <= e.bbox.0 + e.bbox.2 + EPS && p.y >= e.bbox.1 - EPS && p.y <= e.bbox.1 + e.bbox.3 + EPS && contains(&e.space.points, p))
			.collect();
		v.sort_by(|a, b| Self::deeper(a, b));
		v
	}

	/// The spaces around `p` on `page`, outermost first.
	pub fn around(&self, page: u32, p: Point) -> Vec<&'a Space> {
		self.around_entries(page, p).into_iter().map(|e| e.space).collect()
	}

	/// The deepest space around `p` on `page`.
	pub fn deepest(&self, page: u32, p: Point) -> Option<&'a Space> {
		self.around_entries(page, p).last().map(|e| e.space)
	}

	/// A space and the spaces it sits in, outermost first, ending with it.
	pub fn path(&self, id: &str) -> Vec<&'a Space> {
		let Some(me) = self.entry(id) else { return vec![] };
		let Some(at) = interior_point(&me.space.points) else { return vec![me.space] };
		let mut v: Vec<&Entry> = self.around_entries(me.space.page, at).into_iter().filter(|e| Self::deeper(e, me) == std::cmp::Ordering::Less).collect();
		v.push(me);
		v.into_iter().map(|e| e.space).collect()
	}

	/// "Level 2 › Unit 204 › Kitchen".
	pub fn path_name(&self, id: &str) -> String {
		self.path(id).iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" › ")
	}

	/// How deep a space sits (0 = in no other space).
	pub fn depth(&self, id: &str) -> usize {
		self.path(id).len().saturating_sub(1)
	}

	/// Whether `space` is `within` or inside it (the item's space, the filter's space).
	pub fn is_within(&self, space: Option<&str>, within: &str) -> bool {
		space.is_some_and(|s| s == within || self.path(s).iter().any(|x| x.id == within))
	}

	pub fn of_mark(&self, m: &Mark) -> Option<&'a Space> {
		self.deepest(m.page, Point { x: m.x + m.w / 2.0, y: m.y + m.h / 2.0 })
	}

	pub fn of_measurement(&self, m: &Measurement) -> Option<&'a Space> {
		match m.kind {
			MeasurementType::Area => interior_point(&m.points).and_then(|p| self.deepest(m.page, p)),
			MeasurementType::Angle => m.points.get(1).or(m.points.first()).and_then(|p| self.deepest(m.page, *p)),
			MeasurementType::Length => self.of_run(m.page, &m.points, m.closed),
		}
	}

	pub fn of_annotation(&self, a: &Annotation) -> Option<&'a Space> {
		crate::geometry::bounds(&a.points).and_then(|b| self.deepest(a.page, Point { x: b.0 + b.2 / 2.0, y: b.1 + b.3 / 2.0 }))
	}

	/// A polyline's space: the deepest holding more than half of its length,
	/// else the one holding the most.
	pub fn of_run(&self, page: u32, pts: &[Point], closed: bool) -> Option<&'a Space> {
		let first = *pts.first()?;
		let rb = crate::geometry::bounds(pts)?;
		let cands: Vec<&Entry> = self.list.iter().filter(|e| e.space.page == page && rb.0 <= e.bbox.0 + e.bbox.2 + EPS && e.bbox.0 <= rb.0 + rb.2 + EPS && rb.1 <= e.bbox.1 + e.bbox.3 + EPS && e.bbox.1 <= rb.1 + rb.3 + EPS).collect();
		if cands.is_empty() {
			return None;
		}
		let (w, h) = self.sizes.get(&page).copied().unwrap_or((1.0, 1.0));
		let mut segs: Vec<(Point, Point)> = pts.windows(2).map(|s| (s[0], s[1])).collect();
		if closed && pts.len() > 2 {
			segs.push((pts[pts.len() - 1], first));
		}
		let mut held = vec![0.0; cands.len()];
		let mut total = 0.0;
		for (a, b) in segs {
			let len = ((b.x - a.x) * w).hypot((b.y - a.y) * h);
			if len <= 0.0 {
				continue;
			}
			// Cut the segment where it crosses a space's edge; each piece is wholly in or out.
			let mut ts = vec![0.0, 1.0];
			for e in &cands {
				let poly = &e.space.points;
				for i in 0..poly.len() {
					ts.extend(crossing(a, b, poly[i], poly[(i + 1) % poly.len()]));
				}
			}
			ts.sort_by(|x, y| x.total_cmp(y));
			for t in ts.windows(2) {
				if t[1] - t[0] < 1e-12 {
					continue;
				}
				let tm = (t[0] + t[1]) / 2.0;
				let mid = Point { x: a.x + (b.x - a.x) * tm, y: a.y + (b.y - a.y) * tm };
				let piece = (t[1] - t[0]) * len;
				total += piece;
				for (k, e) in cands.iter().enumerate() {
					if contains(&e.space.points, mid) {
						held[k] += piece;
					}
				}
			}
		}
		if total <= 0.0 {
			return self.deepest(page, first);
		}
		let majority = cands.iter().zip(&held).filter(|(_, l)| **l > total * 0.5 + 1e-9 * total).max_by(|a, b| Self::deeper(a.0, b.0)).map(|(e, _)| e.space);
		majority.or_else(|| cands.iter().zip(&held).filter(|(_, l)| **l > 0.0).max_by(|a, b| a.1.total_cmp(b.1).then(Self::deeper(a.0, b.0))).map(|(e, _)| e.space))
	}

	/// The space of any item by id (marks, measurements, markups).
	pub fn of_item(&self, d: &TakeoffData, id: &str) -> Option<&'a Space> {
		if let Some(m) = d.marks.iter().find(|m| m.id == id) {
			return self.of_mark(m);
		}
		if let Some(m) = d.measurements.iter().find(|m| m.id == id) {
			return self.of_measurement(m);
		}
		d.annotations.iter().find(|a| a.id == id).and_then(|a| self.of_annotation(a))
	}

	/// Every item's space: item id → space id (items in no space are left out).
	pub fn attribute(&self, d: &TakeoffData) -> HashMap<String, String> {
		let mut out = HashMap::new();
		if self.is_empty() {
			return out;
		}
		for m in &d.marks {
			if let Some(s) = self.of_mark(m) {
				out.insert(m.id.clone(), s.id.clone());
			}
		}
		for m in &d.measurements {
			if let Some(s) = self.of_measurement(m) {
				out.insert(m.id.clone(), s.id.clone());
			}
		}
		for a in &d.annotations {
			if let Some(s) = self.of_annotation(a) {
				out.insert(a.id.clone(), s.id.clone());
			}
		}
		out
	}

	/// Spaces in display order: by page, then as a tree (each followed by
	/// the spaces inside it), with their depth.
	pub fn tree(&self) -> Vec<(&'a Space, usize)> {
		let mut keyed: Vec<(u32, Vec<String>, &'a Space, usize)> = self
			.list
			.iter()
			.map(|e| {
				let path = self.path(&e.space.id);
				(e.space.page, path.iter().map(|s| format!("{}\u{0}{}", s.name.to_lowercase(), s.id)).collect(), e.space, path.len() - 1)
			})
			.collect();
		keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
		keyed.into_iter().map(|(_, _, s, d)| (s, d)).collect()
	}
}

/// A fresh space name: "Space 3".
pub fn next_name(d: &TakeoffData) -> String {
	let mut n = d.spaces.len() + 1;
	while d.spaces.iter().any(|s| s.name == format!("Space {n}")) {
		n += 1;
	}
	format!("Space {n}")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn p(x: f64, y: f64) -> Point {
		Point { x, y }
	}

	fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
		vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]
	}

	fn space(id: &str, name: &str, points: Vec<Point>) -> Space {
		Space { id: id.into(), page: 1, name: name.into(), color: "#4CC2FF".into(), points, hidden: false, meta: Default::default() }
	}

	fn mark(id: &str, cx: f64, cy: f64) -> Mark {
		Mark { id: id.into(), category_id: None, page: 1, x: cx - 0.005, y: cy - 0.005, w: 0.01, h: 0.01, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() }
	}

	/// Level (whole left half) › Unit (top-left quarter) › Kitchen and Bath side by side.
	fn building() -> TakeoffData {
		let mut d = TakeoffData::default();
		d.pages.push(PageInfo { page: 1, width_px: 1000.0, height_px: 1000.0 });
		d.spaces.push(space("kit", "Kitchen", rect(0.1, 0.1, 0.25, 0.4)));
		d.spaces.push(space("lvl", "Level 2", rect(0.0, 0.0, 0.5, 1.0)));
		d.spaces.push(space("bath", "Bath", rect(0.25, 0.1, 0.4, 0.4)));
		d.spaces.push(space("unit", "Unit 204", rect(0.05, 0.05, 0.45, 0.45)));
		d
	}

	#[test]
	fn point_in_polygon_counts_edges_and_vertices_as_inside() {
		let sq = rect(0.2, 0.2, 0.6, 0.6);
		assert!(contains(&sq, p(0.4, 0.4)));
		assert!(contains(&sq, p(0.2, 0.4)), "left edge");
		assert!(contains(&sq, p(0.6, 0.6)), "corner");
		assert!(contains(&sq, p(0.4, 0.2)), "top edge");
		assert!(!contains(&sq, p(0.61, 0.4)));
		assert!(!contains(&sq, p(0.1, 0.1)));
		assert!(!contains(&sq[..2], p(0.3, 0.2)), "two points enclose nothing");
		// Concave: an L whose notch is outside.
		let l = vec![p(0.0, 0.0), p(0.6, 0.0), p(0.6, 0.2), p(0.2, 0.2), p(0.2, 0.6), p(0.0, 0.6)];
		assert!(contains(&l, p(0.1, 0.5)));
		assert!(!contains(&l, p(0.4, 0.4)), "in the notch");
		// Ray through a vertex doesn't double count.
		let diamond = vec![p(0.5, 0.1), p(0.9, 0.5), p(0.5, 0.9), p(0.1, 0.5)];
		assert!(contains(&diamond, p(0.3, 0.5)));
		assert!(!contains(&diamond, p(0.05, 0.5)));
	}

	#[test]
	fn interior_point_of_an_l_shape_is_inside_it() {
		let l = vec![p(0.0, 0.0), p(0.6, 0.0), p(0.6, 0.1), p(0.1, 0.1), p(0.1, 0.6), p(0.0, 0.6)];
		let c = centroid(&l).unwrap();
		assert!(!contains(&l, c), "the centroid of a thin L is outside it");
		assert!(contains(&l, interior_point(&l).unwrap()));
		assert_eq!(interior_point(&rect(0.0, 0.0, 0.4, 0.2)).unwrap(), p(0.2, 0.1));
	}

	#[test]
	fn nesting_deepest_wins_with_the_full_path() {
		let d = building();
		let s = Spaces::new(&d);
		assert_eq!(s.deepest(1, p(0.2, 0.2)).unwrap().id, "kit");
		assert_eq!(s.deepest(1, p(0.42, 0.42)).unwrap().id, "unit", "in the unit, outside both rooms");
		assert_eq!(s.deepest(1, p(0.3, 0.8)).unwrap().id, "lvl");
		assert!(s.deepest(1, p(0.8, 0.5)).is_none());
		assert!(s.deepest(2, p(0.2, 0.2)).is_none(), "other page");
		let around: Vec<&str> = s.around(1, p(0.2, 0.2)).iter().map(|x| x.id.as_str()).collect();
		assert_eq!(around, ["lvl", "unit", "kit"]);
		assert_eq!(s.path_name("kit"), "Level 2 › Unit 204 › Kitchen");
		assert_eq!(s.path_name("lvl"), "Level 2");
		assert_eq!(s.depth("bath"), 2);
		assert!(s.is_within(Some("kit"), "unit"));
		assert!(s.is_within(Some("kit"), "kit"));
		assert!(!s.is_within(Some("kit"), "bath"));
		assert!(!s.is_within(None, "lvl"));
		let tree: Vec<(&str, usize)> = s.tree().iter().map(|(x, d)| (x.id.as_str(), *d)).collect();
		assert_eq!(tree, [("lvl", 0), ("unit", 1), ("bath", 2), ("kit", 2)]);
	}

	#[test]
	fn shared_edges_go_to_the_smaller_then_the_later_space() {
		let d = building();
		let s = Spaces::new(&d);
		// On the wall between Kitchen and Bath (same size): Bath is later in the list.
		assert_eq!(s.deepest(1, p(0.25, 0.2)).unwrap().id, "bath");
		// On the unit's own edge: still in the unit.
		assert_eq!(s.deepest(1, p(0.05, 0.42)).unwrap().id, "unit");
		assert_eq!(s.of_mark(&mark("m", 0.2, 0.3)).unwrap().id, "kit");
	}

	#[test]
	fn lengths_go_to_the_space_holding_most_of_them() {
		let d = building();
		let s = Spaces::new(&d);
		let run = |pts: Vec<Point>| s.of_run(1, &pts, false).map(|x| x.id.clone());
		// 0.12 in the kitchen, 0.03 in the bath: the kitchen.
		assert_eq!(run(vec![p(0.13, 0.3), p(0.28, 0.3)]).as_deref(), Some("kit"));
		// Half and half across the wall: neither holds more than half; the unit holds it all.
		assert_eq!(run(vec![p(0.15, 0.3), p(0.35, 0.3)]).as_deref(), Some("unit"));
		// Mostly out of the building: the level holds 0.1 of 0.5 (plurality).
		assert_eq!(run(vec![p(0.4, 0.8), p(0.9, 0.8)]).as_deref(), Some("lvl"));
		assert_eq!(run(vec![p(0.6, 0.8), p(0.9, 0.8)]), None);
		// Along the shared wall: both hold it, the later of the equal rooms wins.
		assert_eq!(run(vec![p(0.25, 0.15), p(0.25, 0.35)]).as_deref(), Some("bath"));
		// A closed perimeter around the kitchen lies on its edges.
		assert_eq!(s.of_run(1, &rect(0.1, 0.1, 0.25, 0.4), true).unwrap().id, "kit");
	}

	#[test]
	fn areas_angles_and_markups() {
		let d = building();
		let s = Spaces::new(&d);
		let mut area = Measurement::new("a", 1, MeasurementType::Area, rect(0.26, 0.12, 0.39, 0.38));
		assert_eq!(s.of_measurement(&area).unwrap().id, "bath");
		area.page = 2;
		assert!(s.of_measurement(&area).is_none());
		let angle = Measurement::new("g", 1, MeasurementType::Angle, vec![p(0.9, 0.9), p(0.2, 0.2), p(0.9, 0.1)]);
		assert_eq!(s.of_measurement(&angle).unwrap().id, "kit");
		let mut d2 = d.clone();
		d2.marks.push(mark("m1", 0.3, 0.2));
		d2.marks.push(mark("m2", 0.9, 0.9));
		let s2 = Spaces::new(&d2);
		let map = s2.attribute(&d2);
		assert_eq!(map.get("m1").map(String::as_str), Some("bath"));
		assert!(!map.contains_key("m2"));
		assert_eq!(s2.of_item(&d2, "m1").unwrap().name, "Bath");
	}

	#[test]
	fn names_are_fresh() {
		let mut d = TakeoffData::default();
		assert_eq!(next_name(&d), "Space 1");
		d.spaces.push(space("a", "Space 2", rect(0.0, 0.0, 0.1, 0.1)));
		assert_eq!(next_name(&d), "Space 3");
	}
}
