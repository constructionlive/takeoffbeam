//! The Select tool and what acts on a selection: box select, the right-click menu,
//! delete, copy and paste, select all, nudge.

use super::*;

impl Canvas {
	#[allow(clippy::too_many_arguments)]
	pub(super) fn select_tool(&mut self, ui: &mut egui::Ui, resp: &egui::Response, doc: &mut OpenDoc, pos: Pos2, pt: Point, mods: Modifiers, double: bool, primary_pressed: bool) {
		let in_canvas = doc.canvas_rect.contains(pos);
		let cmd = mods.command || mods.ctrl;
		if primary_pressed && in_canvas && self.drag.is_none() && self.editing_text.is_none() {
			if let Some(h) = self.handle_at(doc, pos) {
				doc.doc.lock().begin_gesture("Edit shape");
				self.drag = Some(h);
			}
		}
		if resp.drag_started() && self.drag.is_none() && in_canvas {
			let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
			let raw = doc.to_page(origin);
			match self.hit(doc, origin) {
				Some(id) => {
					if !doc.selection.contains(&id) {
						if !mods.shift && !cmd {
							doc.selection.clear();
						}
						doc.selection.insert(id);
					}
					doc.doc.lock().begin_gesture("Move");
					self.drag = Some(Drag::Move { last: raw, moved: false });
				}
				// Empty sheet: box-select (Shift/⌘ adds to the selection). Pan with
				// two fingers, Space-drag or the wheel button.
				None => {
					if !mods.shift && !cmd {
						doc.selection.clear();
					}
					self.drag = Some(Drag::Marquee { start: origin });
				}
			}
		}
		match &mut self.drag {
			Some(Drag::Move { last, moved }) if resp.dragged() => {
				let raw = doc.to_page(pos);
				let (dx, dy) = (raw.x - last.x, raw.y - last.y);
				if dx != 0.0 || dy != 0.0 {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Move", |d| d.translate_ids(&ids, dx, dy));
					*last = raw;
					*moved = true;
				}
				ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
			}
			Some(Drag::Vertex { id, idx, cutout }) if resp.dragged() || ui.input(|i| i.pointer.primary_down()) => {
				let (id, idx, cutout) = (id.clone(), *idx, *cutout);
				doc.doc.lock().edit("Edit shape", |d| {
					if let Some(m) = d.measurements.iter_mut().find(|m| m.id == id) {
						let target = match cutout {
							Some(ci) => m.cutouts.get_mut(ci).and_then(|c| c.get_mut(idx)),
							None => m.points.get_mut(idx),
						};
						if let Some(v) = target {
							*v = pt;
						}
						m.meta.modified_at = Some(now_ms());
					} else if let Some(a) = d.annotations.iter_mut().find(|a| a.id == id) {
						if let Some(v) = a.points.get_mut(idx) {
							*v = pt;
						}
						a.meta.modified_at = Some(now_ms());
					} else if let Some(s) = d.spaces.iter_mut().find(|s| s.id == id) {
						if let Some(v) = s.points.get_mut(idx) {
							*v = pt;
						}
						s.meta.modified_at = Some(now_ms());
					}
				});
			}
			Some(Drag::Resize { id, corner }) if resp.dragged() || ui.input(|i| i.pointer.primary_down()) => {
				let (id, corner) = (id.clone(), *corner);
				doc.doc.lock().edit("Resize", |d| {
					if let Some(a) = d.annotations.iter_mut().find(|a| a.id == id) {
						if let Some(v) = a.points.get_mut(corner) {
							*v = pt;
						}
					} else if let Some(m) = d.marks.iter_mut().find(|m| m.id == id) {
						let (x1, y1) = (m.x + m.w, m.y + m.h);
						if corner == 0 {
							m.x = pt.x.min(x1 - 0.001);
							m.y = pt.y.min(y1 - 0.001);
							m.w = x1 - m.x;
							m.h = y1 - m.y;
						} else {
							m.w = (pt.x - m.x).max(0.001);
							m.h = (pt.y - m.y).max(0.001);
						}
					} else if let Some(l) = d.legends.iter_mut().find(|l| l.id == id) {
						crate::legends::resize(l, corner, pt);
					}
				});
			}
			_ => {}
		}
		let released = ui.input(|i| i.pointer.primary_released());
		if released || resp.drag_stopped() {
			match self.drag.take() {
				Some(Drag::Marquee { start }) => {
					let r = Rect::from_two_pos(start, pos);
					if r.width() > 3.0 || r.height() > 3.0 {
						let ids = self.items_in(doc, r);
						doc.selection.extend(ids);
					}
				}
				Some(Drag::Move { .. }) | Some(Drag::Vertex { .. }) | Some(Drag::Resize { .. }) => {
					doc.doc.lock().end_gesture();
				}
				other => self.drag = other,
			}
		}
		if resp.clicked() && in_canvas {
			match self.hit(doc, pos) {
				Some(id) => {
					if mods.shift || cmd {
						if !doc.selection.remove(&id) {
							doc.selection.insert(id);
						}
					} else {
						doc.selection.clear();
						doc.selection.insert(id);
					}
				}
				None => {
					if !mods.shift && !cmd {
						doc.selection.clear();
					}
					// The map: a callout goes where it points; any other area shows in the Map tab.
					if self.map_links && !mods.shift && !cmd {
						match doc.map_area_at(pt) {
							Some(a) => {
								let is_callout = doc.map.as_ref().and_then(|m| m.node(&a)).is_some_and(|n| matches!(n, docmap::NodeRef::Part(_, x) if x.kind == docmap::PartKind::Callout));
								match doc.map_targets(&a).first().cloned() {
									Some(t) if is_callout => {
										doc.map_peek(&a);
										doc.map_go(&t);
									}
									_ => doc.map_peek(&a),
								}
							}
							None => doc.nav.focus = None,
						}
					}
				}
			}
		}
		if double && in_canvas {
			if let Some(id) = self.hit(doc, pos) {
				let is_text = doc.doc.lock().data.annotations.iter().any(|a| a.id == id && matches!(a.kind, AnnotationKind::Text | AnnotationKind::Callout | AnnotationKind::Stamp));
				if is_text {
					self.start_text_edit(doc, &id);
				}
			}
		}
		// Hover cursor feedback.
		if self.drag.is_none() && in_canvas {
			if self.handle_at(doc, pos).is_some() {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
			} else if self.hit(doc, pos).is_some() {
				ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
			}
		}
	}

	pub(super) fn items_in(&self, doc: &OpenDoc, r: Rect) -> Vec<String> {
		let d = doc.doc.lock();
		let page = doc.page;
		let inside = |q: &Point| r.contains(doc.to_screen(*q));
		let mut out = Vec::new();
		for m in d.data.marks.iter().filter(|m| m.page == page) {
			if inside(&Point { x: m.x + m.w / 2.0, y: m.y + m.h / 2.0 }) {
				out.push(m.id.clone());
			}
		}
		for m in d.data.measurements.iter().filter(|m| m.page == page) {
			if m.points.iter().all(inside) {
				out.push(m.id.clone());
			}
		}
		for a in d.data.annotations.iter().filter(|a| a.page == page) {
			if a.points.iter().all(inside) {
				out.push(a.id.clone());
			}
		}
		for s in d.data.spaces.iter().filter(|s| s.page == page && !s.hidden) {
			if s.points.iter().all(inside) {
				out.push(s.id.clone());
			}
		}
		for l in d.data.legends.iter().filter(|l| l.page == page) {
			if inside(&Point { x: l.x, y: l.y }) && inside(&Point { x: l.x + l.w, y: l.y + l.h }) {
				out.push(l.id.clone());
			}
		}
		out
	}

	pub(super) fn context_menu(&mut self, resp: &egui::Response, doc: &mut OpenDoc) {
		if resp.secondary_clicked() {
			if let Some(pos) = resp.interact_pointer_pos() {
				if let Some(id) = self.hit(doc, pos) {
					if !doc.selection.contains(&id) {
						doc.selection.clear();
						doc.selection.insert(id);
					}
				}
			}
		}
		resp.context_menu(|ui| {
			let n = doc.selection.len();
			if n > 0 {
				let any_pending = {
					let d = doc.doc.lock();
					d.data.marks.iter().any(|m| doc.selection.contains(&m.id) && m.status == MarkStatus::Pending)
				};
				if any_pending && ui.button(format!("{}  Confirm", crate::icons::CHECK)).clicked() {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Confirm", |d| {
						for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.status = MarkStatus::Confirmed;
						}
					});
					ui.close();
				}
				if ui.button(format!("{}  Copy", crate::icons::COPY)).clicked() {
					self.copy(doc);
					ui.close();
				}
				if ui.button(format!("{}  Duplicate", crate::icons::COPY)).clicked() {
					self.copy(doc);
					self.paste(doc, true);
					ui.close();
				}
				let cats: Vec<(String, String, CategoryKind)> = doc.doc.lock().data.categories.iter().map(|c| (c.id.clone(), c.name.clone(), c.kind())).collect();
				if !cats.is_empty() {
					ui.menu_button(format!("{}  Move to item", crate::icons::TAG), |ui| {
						for (id, name, kind) in &cats {
							if ui.button(name).clicked() {
								let ids = doc.selection.clone();
								let (id, kind) = (id.clone(), *kind);
								doc.doc.lock().edit("Change item", |d| {
									for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
										if kind == CategoryKind::Count {
											m.category_id = Some(id.clone());
										}
									}
									for m in d.measurements.iter_mut().filter(|m| ids.contains(&m.id)) {
										let fits = matches!((kind, m.kind), (CategoryKind::Length, MeasurementType::Length) | (CategoryKind::Area, MeasurementType::Area));
										if fits {
											m.category_id = Some(id.clone());
										}
									}
								});
								ui.close();
							}
						}
					});
				}
				let locked = {
					let d = doc.doc.lock();
					d.data.marks.iter().filter(|m| doc.selection.contains(&m.id)).any(|m| m.meta.locked)
						|| d.data.measurements.iter().filter(|m| doc.selection.contains(&m.id)).any(|m| m.meta.locked)
						|| d.data.annotations.iter().filter(|a| doc.selection.contains(&a.id)).any(|a| a.meta.locked)
				};
				if ui.button(if locked { format!("{}  Unlock", crate::icons::LOCK_OPEN) } else { format!("{}  Lock", crate::icons::LOCK) }).clicked() {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Lock", |d| {
						for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.meta.locked = !locked;
						}
						for m in d.measurements.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.meta.locked = !locked;
						}
						for a in d.annotations.iter_mut().filter(|a| ids.contains(&a.id)) {
							a.meta.locked = !locked;
						}
					});
					ui.close();
				}
				ui.separator();
				if ui.button(egui::RichText::new(format!("{}  Delete {}", crate::icons::TRASH_2, if n > 1 { format!("{n} items") } else { String::new() })).color(Color32::from_rgb(0xF8, 0x71, 0x71))).clicked() {
					self.delete_selection(doc);
					ui.close();
				}
			} else {
				if !self.clipboard.is_empty() && ui.button(format!("{}  Paste", crate::icons::CLIPBOARD_PASTE)).clicked() {
					self.paste(doc, false);
					ui.close();
				}
				if ui.button(format!("{}  Select all on page", crate::icons::MOUSE_POINTER_2)).clicked() {
					self.select_all(doc);
					ui.close();
				}
				if ui.button(format!("{}  Fit page", crate::icons::MAXIMIZE)).clicked() {
					doc.fit_page();
					ui.close();
				}
			}
		});
	}

	pub fn delete_selection(&mut self, doc: &mut OpenDoc) {
		if doc.selection.is_empty() {
			return;
		}
		let ids = doc.selection.clone();
		let n = doc.doc.lock().edit("Delete", |d| d.delete_ids(&ids));
		doc.selection.clear();
		if n < ids.len() {
			self.requests.push(CanvasRequest::Status("Locked items were kept.".into()));
		}
	}

	pub fn copy(&mut self, doc: &OpenDoc) {
		self.clipboard = doc.doc.lock().data.clip(&doc.selection);
	}

	pub fn paste(&mut self, doc: &mut OpenDoc, offset: bool) {
		if self.clipboard.is_empty() {
			return;
		}
		let (dx, dy) = if offset { (0.01, 0.01) } else { (0.0, 0.0) };
		let clip = self.clipboard.clone();
		let page = doc.page;
		let meta = doc.doc.lock().new_meta();
		let ids = doc.doc.lock().edit("Paste", |d| d.paste(&clip, page, dx, dy, &meta));
		doc.selection = ids.into_iter().collect();
	}

	pub fn select_all(&mut self, doc: &mut OpenDoc) {
		let d = doc.doc.lock();
		let p = doc.page;
		let ids: HashSet<String> = d.data.marks.iter().filter(|m| m.page == p).map(|m| m.id.clone())
			.chain(d.data.measurements.iter().filter(|m| m.page == p).map(|m| m.id.clone()))
			.chain(d.data.annotations.iter().filter(|a| a.page == p).map(|a| a.id.clone()))
			.collect();
		drop(d);
		doc.selection = ids;
	}

	pub fn nudge(&mut self, doc: &mut OpenDoc, dx_px: f32, dy_px: f32) {
		if doc.selection.is_empty() {
			return;
		}
		let (fx, fy) = doc.frac_scale();
		let ids = doc.selection.clone();
		doc.doc.lock().edit("Nudge", |d| d.translate_ids(&ids, (dx_px / fx) as f64, (dy_px / fy) as f64));
	}
}
