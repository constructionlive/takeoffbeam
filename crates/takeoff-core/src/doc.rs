//! The open takeoff: data + undo/redo + derived values. One `Document` is
//! shared by the viewer and the agent (behind a lock); every change goes
//! through `edit`, so both get undo, dirty tracking and recomputed quantities.

use std::collections::HashSet;

use crate::geometry::measurement_quantity;
use crate::model::*;

const UNDO_LIMIT: usize = 200;

/// Any item on a page, by id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemRef {
	Mark(String),
	Measurement(String),
	Annotation(String),
}

impl ItemRef {
	pub fn id(&self) -> &str {
		match self {
			ItemRef::Mark(id) | ItemRef::Measurement(id) | ItemRef::Annotation(id) => id,
		}
	}
}

struct Snapshot {
	label: String,
	data: TakeoffData,
}

pub struct Document {
	pub data: TakeoffData,
	/// Page sizes in PDF points, index 0 = page 1.
	pub page_sizes_pt: Vec<(f64, f64)>,
	/// Name stamped on new items (Bluebeam "Author").
	pub author: String,
	undo: Vec<Snapshot>,
	redo: Vec<Snapshot>,
	revision: u64,
	saved_revision: u64,
	/// Open gesture (drag/resize): one undo step for many mutations.
	gesture: bool,
}

impl Document {
	pub fn new(mut data: TakeoffData, page_sizes_pt: Vec<(f64, f64)>, author: impl Into<String>) -> Self {
		crate::normalize::normalize(&mut data);
		data.pages = page_infos(&page_sizes_pt);
		let mut doc = Self { data, page_sizes_pt, author: author.into(), undo: vec![], redo: vec![], revision: 0, saved_revision: 0, gesture: false };
		doc.recompute();
		doc
	}

	/// The PDF was read (again): new page sizes, same takeoff and undo history.
	pub fn set_page_sizes(&mut self, page_sizes_pt: Vec<(f64, f64)>) {
		self.data.pages = page_infos(&page_sizes_pt);
		self.page_sizes_pt = page_sizes_pt;
		self.recompute();
	}

	pub fn revision(&self) -> u64 {
		self.revision
	}
	pub fn is_dirty(&self) -> bool {
		self.revision != self.saved_revision
	}
	pub fn mark_saved(&mut self) {
		self.saved_revision = self.revision;
	}
	pub fn page_count(&self) -> u32 {
		self.page_sizes_pt.len() as u32
	}
	pub fn page_size_pt(&self, page: u32) -> (f64, f64) {
		self.page_sizes_pt.get(page.saturating_sub(1) as usize).copied().unwrap_or((612.0, 792.0))
	}

	/// One undoable change.
	pub fn edit<R>(&mut self, label: &str, f: impl FnOnce(&mut TakeoffData) -> R) -> R {
		if !self.gesture {
			self.push_undo(label);
		}
		let r = f(&mut self.data);
		self.after_change();
		r
	}

	/// Start a gesture: mutations until `end_gesture` are one undo step.
	pub fn begin_gesture(&mut self, label: &str) {
		if !self.gesture {
			self.push_undo(label);
			self.gesture = true;
		}
	}
	pub fn end_gesture(&mut self) {
		self.gesture = false;
	}
	pub fn in_gesture(&self) -> bool {
		self.gesture
	}

	fn push_undo(&mut self, label: &str) {
		self.undo.push(Snapshot { label: label.to_string(), data: self.data.clone() });
		if self.undo.len() > UNDO_LIMIT {
			self.undo.remove(0);
		}
		self.redo.clear();
	}

	fn after_change(&mut self) {
		self.recompute();
		self.revision += 1;
	}

	pub fn undo_label(&self) -> Option<&str> {
		self.undo.last().map(|s| s.label.as_str())
	}
	pub fn redo_label(&self) -> Option<&str> {
		self.redo.last().map(|s| s.label.as_str())
	}

	pub fn undo(&mut self) -> bool {
		self.gesture = false;
		let Some(snap) = self.undo.pop() else { return false };
		let current = std::mem::replace(&mut self.data, snap.data);
		self.redo.push(Snapshot { label: snap.label, data: current });
		self.after_change();
		true
	}

	pub fn redo(&mut self) -> bool {
		let Some(snap) = self.redo.pop() else { return false };
		let current = std::mem::replace(&mut self.data, snap.data);
		self.undo.push(Snapshot { label: snap.label, data: current });
		self.after_change();
		true
	}

	/// Recompute every measurement's stored value/unit from its points and scale.
	pub fn recompute(&mut self) {
		let sizes = &self.page_sizes_pt;
		let data = &mut self.data;
		let scales: Vec<_> = data
			.measurements
			.iter()
			.map(|m| {
				let at = m.points.first().copied().unwrap_or(Point { x: 0.5, y: 0.5 });
				data.scale_at(m.page, at).cloned()
			})
			.collect();
		for (m, scale) in data.measurements.iter_mut().zip(scales) {
			let (w, h) = sizes.get(m.page.saturating_sub(1) as usize).copied().unwrap_or((612.0, 792.0));
			match measurement_quantity(m, scale.as_ref(), w, h) {
				Some(q) => {
					m.value = Some(q.value);
					m.unit = Some(q.unit);
				}
				None => {
					m.value = None;
					m.unit = None;
				}
			}
		}
	}

	pub fn new_meta(&self) -> ItemMeta {
		let now = now_ms();
		ItemMeta { author: Some(self.author.clone()), created_at: Some(now), modified_at: Some(now), ..Default::default() }
	}

	/// Quantity details for one measurement (perimeter, volume, …).
	pub fn quantity(&self, m: &Measurement) -> Option<crate::geometry::Quantity> {
		let (w, h) = self.page_size_pt(m.page);
		let at = m.points.first().copied().unwrap_or(Point { x: 0.5, y: 0.5 });
		measurement_quantity(m, self.data.scale_at(m.page, at), w, h)
	}
}

/// Pages carry the aspect ratio for platform consumers (px at 200 DPI).
fn page_infos(sizes: &[(f64, f64)]) -> Vec<PageInfo> {
	sizes.iter().enumerate().map(|(i, (w, h))| PageInfo { page: i as u32 + 1, width_px: (w * 200.0 / 72.0).round(), height_px: (h * 200.0 / 72.0).round() }).collect()
}

impl TakeoffData {
	pub fn find(&self, id: &str) -> Option<ItemRef> {
		if self.marks.iter().any(|m| m.id == id) {
			Some(ItemRef::Mark(id.into()))
		} else if self.measurements.iter().any(|m| m.id == id) {
			Some(ItemRef::Measurement(id.into()))
		} else if self.annotations.iter().any(|a| a.id == id) {
			Some(ItemRef::Annotation(id.into()))
		} else {
			None
		}
	}

	pub fn item_page(&self, item: &ItemRef) -> Option<u32> {
		match item {
			ItemRef::Mark(id) => self.marks.iter().find(|m| &m.id == id).map(|m| m.page),
			ItemRef::Measurement(id) => self.measurements.iter().find(|m| &m.id == id).map(|m| m.page),
			ItemRef::Annotation(id) => self.annotations.iter().find(|a| &a.id == id).map(|a| a.page),
		}
	}

	/// Delete items by id (any kind, spaces and legends too). Locked items are kept. Returns how many went.
	pub fn delete_ids(&mut self, ids: &HashSet<String>) -> usize {
		let count = |d: &Self| d.marks.len() + d.measurements.len() + d.annotations.len() + d.spaces.len() + d.legends.len();
		let before = count(self);
		self.marks.retain(|m| !ids.contains(&m.id) || m.meta.locked);
		self.measurements.retain(|m| !ids.contains(&m.id) || m.meta.locked);
		self.annotations.retain(|a| !ids.contains(&a.id) || a.meta.locked);
		self.spaces.retain(|s| !ids.contains(&s.id) || s.meta.locked);
		self.legends.retain(|l| !ids.contains(&l.id) || l.meta.locked);
		before - count(self)
	}

	/// Move items by (dx, dy) page fractions, clamped to the page. Locked items stay.
	pub fn translate_ids(&mut self, ids: &HashSet<String>, dx: f64, dy: f64) {
		let now = now_ms();
		let mv = |p: &mut Point| {
			p.x = (p.x + dx).clamp(0.0, 1.0);
			p.y = (p.y + dy).clamp(0.0, 1.0);
		};
		for m in self.marks.iter_mut().filter(|m| ids.contains(&m.id) && !m.meta.locked) {
			m.x = (m.x + dx).clamp(0.0, (1.0 - m.w).max(0.0));
			m.y = (m.y + dy).clamp(0.0, (1.0 - m.h).max(0.0));
			m.meta.modified_at = Some(now);
		}
		for m in self.measurements.iter_mut().filter(|m| ids.contains(&m.id) && !m.meta.locked) {
			m.points.iter_mut().for_each(mv);
			m.cutouts.iter_mut().flatten().for_each(mv);
			m.meta.modified_at = Some(now);
		}
		for a in self.annotations.iter_mut().filter(|a| ids.contains(&a.id) && !a.meta.locked) {
			a.points.iter_mut().for_each(mv);
			a.meta.modified_at = Some(now);
		}
		for s in self.spaces.iter_mut().filter(|s| ids.contains(&s.id) && !s.meta.locked) {
			s.points.iter_mut().for_each(mv);
			s.meta.modified_at = Some(now);
		}
		for l in self.legends.iter_mut().filter(|l| ids.contains(&l.id) && !l.meta.locked) {
			l.x = (l.x + dx).clamp(0.0, 1.0 - l.w);
			l.y = (l.y + dy).clamp(0.0, 1.0 - l.h);
			l.meta.modified_at = Some(now);
		}
	}

	/// Copies of items, for the clipboard.
	pub fn clip(&self, ids: &HashSet<String>) -> Clipboard {
		Clipboard {
			marks: self.marks.iter().filter(|m| ids.contains(&m.id)).cloned().collect(),
			measurements: self.measurements.iter().filter(|m| ids.contains(&m.id)).cloned().collect(),
			annotations: self.annotations.iter().filter(|a| ids.contains(&a.id)).cloned().collect(),
			spaces: self.spaces.iter().filter(|s| ids.contains(&s.id)).cloned().collect(),
			legends: self.legends.iter().filter(|l| ids.contains(&l.id)).cloned().collect(),
		}
	}

	/// Paste a clipboard onto `page`, offset by (dx, dy). Returns the new ids.
	pub fn paste(&mut self, clip: &Clipboard, page: u32, dx: f64, dy: f64, meta: &ItemMeta) -> Vec<String> {
		let mut ids = Vec::new();
		let shift = |p: &Point| Point { x: (p.x + dx).clamp(0.0, 1.0), y: (p.y + dy).clamp(0.0, 1.0) };
		for m in &clip.marks {
			let id = new_id("m");
			ids.push(id.clone());
			self.marks.push(Mark {
				id,
				page,
				x: (m.x + dx).clamp(0.0, (1.0 - m.w).max(0.0)),
				y: (m.y + dy).clamp(0.0, (1.0 - m.h).max(0.0)),
				source: Source::User,
				status: MarkStatus::Confirmed,
				meta: meta.clone(),
				..m.clone()
			});
		}
		for m in &clip.measurements {
			let id = new_id("ms");
			ids.push(id.clone());
			self.measurements.push(Measurement {
				id,
				page,
				points: m.points.iter().map(shift).collect(),
				cutouts: m.cutouts.iter().map(|c| c.iter().map(shift).collect()).collect(),
				source: None,
				meta: meta.clone(),
				..m.clone()
			});
		}
		for a in &clip.annotations {
			let id = new_id("an");
			ids.push(id.clone());
			self.annotations.push(Annotation { id, page, points: a.points.iter().map(shift).collect(), meta: meta.clone(), ..a.clone() });
		}
		for s in &clip.spaces {
			let id = new_id("sp");
			ids.push(id.clone());
			self.spaces.push(Space { id, page, points: s.points.iter().map(shift).collect(), meta: meta.clone(), ..s.clone() });
		}
		for l in &clip.legends {
			let id = new_id("lg");
			ids.push(id.clone());
			self.legends.push(Legend { id, page, x: (l.x + dx).clamp(0.0, 1.0 - l.w), y: (l.y + dy).clamp(0.0, 1.0 - l.h), meta: meta.clone(), ..l.clone() });
		}
		ids
	}
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clipboard {
	pub marks: Vec<Mark>,
	pub measurements: Vec<Measurement>,
	pub annotations: Vec<Annotation>,
	#[serde(default)]
	pub spaces: Vec<Space>,
	#[serde(default)]
	pub legends: Vec<Legend>,
}

impl Clipboard {
	pub fn is_empty(&self) -> bool {
		self.marks.is_empty() && self.measurements.is_empty() && self.annotations.is_empty() && self.spaces.is_empty() && self.legends.is_empty()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn doc() -> Document {
		Document::new(TakeoffData::default(), vec![(1000.0, 500.0)], "tester")
	}

	#[test]
	fn undo_redo_and_recompute() {
		let mut d = doc();
		d.edit("scale", |t| t.page_scales.push(PageScale { page: 1, p1: Point { x: 0.0, y: 0.0 }, p2: Point { x: 0.1, y: 0.0 }, distance: 10.0, unit: "ft".into() }));
		d.edit("line", |t| t.measurements.push(Measurement::new("ms_1", 1, MeasurementType::Length, vec![Point { x: 0.0, y: 0.0 }, Point { x: 0.2, y: 0.0 }])));
		assert!((d.data.measurements[0].value.unwrap() - 20.0).abs() < 1e-9);
		assert!(d.is_dirty());
		assert!(d.undo());
		assert!(d.data.measurements.is_empty());
		assert!(d.undo());
		assert!(d.data.page_scales.is_empty());
		assert!(d.redo() && d.redo());
		assert_eq!(d.data.measurements.len(), 1);
		assert_eq!(d.undo_label(), Some("line"));
	}

	#[test]
	fn reloading_the_pdf_keeps_undo() {
		let mut d = doc();
		d.edit("Slip-sheet", |t| {
			t.page_scales.push(PageScale { page: 1, p1: Point { x: 0.0, y: 0.0 }, p2: Point { x: 0.1, y: 0.0 }, distance: 10.0, unit: "ft".into() });
			t.measurements.push(Measurement::new("ms_1", 1, MeasurementType::Length, vec![Point { x: 0.0, y: 0.0 }, Point { x: 0.2, y: 0.0 }]));
			t.pdf_state = 7;
		});
		d.set_page_sizes(vec![(2000.0, 500.0), (612.0, 792.0)]);
		assert_eq!((d.page_count(), d.data.pages.len()), (2, 2));
		assert!((d.data.measurements[0].value.unwrap() - 20.0).abs() < 1e-9, "fractions of the page: same length on bigger paper");
		assert_eq!(d.undo_label(), Some("Slip-sheet"));
		d.undo();
		assert_eq!((d.data.pdf_state, d.data.measurements.len()), (0, 0));
		d.redo();
		assert_eq!(d.data.pdf_state, 7);
	}

	#[test]
	fn gesture_is_one_undo_step() {
		let mut d = doc();
		d.edit("add", |t| t.annotations.push(Annotation { id: "a".into(), page: 1, kind: AnnotationKind::Rectangle, points: vec![Point { x: 0.1, y: 0.1 }, Point { x: 0.2, y: 0.2 }], text: None, style: Default::default(), subject: None, note: None, source: None, meta: Default::default() }));
		let ids: HashSet<String> = ["a".to_string()].into();
		d.begin_gesture("move");
		for _ in 0..10 {
			d.edit("move", |t| t.translate_ids(&ids, 0.01, 0.0));
		}
		d.end_gesture();
		assert!((d.data.annotations[0].points[0].x - 0.2).abs() < 1e-9);
		d.undo();
		assert!((d.data.annotations[0].points[0].x - 0.1).abs() < 1e-9);
	}

	#[test]
	fn viewport_scale_wins_inside_it() {
		let mut d = doc();
		d.edit("scales", |t| {
			t.page_scales.push(PageScale { page: 1, p1: Point { x: 0.0, y: 0.0 }, p2: Point { x: 0.1, y: 0.0 }, distance: 10.0, unit: "ft".into() });
			t.viewports.push(Viewport { id: "vp".into(), page: 1, name: "Detail".into(), x: 0.5, y: 0.0, w: 0.5, h: 1.0, scale: PageScale { page: 1, p1: Point { x: 0.0, y: 0.0 }, p2: Point { x: 0.1, y: 0.0 }, distance: 1.0, unit: "ft".into() } });
			t.measurements.push(Measurement::new("in_vp", 1, MeasurementType::Length, vec![Point { x: 0.6, y: 0.0 }, Point { x: 0.7, y: 0.0 }]));
			t.measurements.push(Measurement::new("plan", 1, MeasurementType::Length, vec![Point { x: 0.1, y: 0.0 }, Point { x: 0.2, y: 0.0 }]));
		});
		assert!((d.data.measurements[0].value.unwrap() - 1.0).abs() < 1e-9);
		assert!((d.data.measurements[1].value.unwrap() - 10.0).abs() < 1e-9);
	}
}
