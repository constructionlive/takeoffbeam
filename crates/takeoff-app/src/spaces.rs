//! Spaces: named regions (levels, units, rooms) that quantities belong to.
//! The canvas's Space and Fill Space tools draw them; this is how they look
//! on the sheet, how they're picked, the list in the Takeoff tab with the
//! totals per space, and their properties.

use std::collections::HashSet;

use eframe::egui::{self, Align2, Color32, Pos2, Rect, RichText, Shape, Stroke, Vec2};
use takeoff_core::list::{summary_by_space, summary_where, SummaryLine};
use takeoff_core::spaces::Spaces;
use takeoff_core::*;

use crate::canvas::Canvas;
use crate::docstate::OpenDoc;
use crate::geom;
use crate::icons as i;
use crate::theme::{self, hex, Palette};
use crate::tools::Tool;
use crate::widgets;

const HIT_PX: f32 = 6.0;

/// The Takeoff tab's spaces: which space the totals are for, and the list.
#[derive(Default)]
pub struct SpacesPanel {
	/// Item totals for this space and the spaces inside it only.
	pub filter: Option<String>,
	/// Show the totals per space.
	pub grouped: bool,
	renaming: Option<(String, String)>,
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
	Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Where a space's name sits on screen (its outline's top-left corner, inside).
fn label_rect(doc: &OpenDoc, s: &Space) -> Option<Rect> {
	let b = geometry::bounds(&s.points)?;
	let at = doc.to_screen(Point { x: b.0, y: b.1 }) + Vec2::new(4.0, 4.0);
	Some(Rect::from_min_size(at, Vec2::new(s.name.chars().count() as f32 * 6.6 + 12.0, 18.0)))
}

/// Spaces on the page, under everything else: a faint tint, a dashed
/// outline and the name.
pub fn paint(painter: &egui::Painter, doc: &OpenDoc, data: &TakeoffData, flash: &HashSet<String>, p: &Palette) {
	let mut shapes = Vec::new();
	let mut labels = Vec::new();
	for s in data.spaces.iter().filter(|s| s.page == doc.page && !s.hidden) {
		let color = hex(&s.color);
		let pts: Vec<Pos2> = s.points.iter().map(|q| doc.to_screen(*q)).collect();
		let selected = doc.selection.contains(&s.id) || flash.contains(&s.id);
		shapes.push(geom::fill_polygon(&pts, &[], with_alpha(color, if selected { 0.12 } else { 0.06 })));
		geom::dashed(&pts, true, Stroke::new(if selected { 2.5 } else { 1.5 }, with_alpha(color, 0.9)), 10.0, 5.0, &mut shapes);
		if selected {
			for v in &pts {
				shapes.push(Shape::circle_filled(*v, 4.0, color));
				shapes.push(Shape::circle_stroke(*v, 5.5, Stroke::new(1.5, p.selection)));
			}
		}
		if let Some(r) = label_rect(doc, s) {
			labels.push((r, s.name.clone(), color, selected));
		}
	}
	painter.extend(shapes);
	for (r, name, color, selected) in labels {
		painter.rect_filled(r, 3.0, with_alpha(color, if selected { 0.95 } else { 0.8 }));
		painter.text(r.left_center() + Vec2::new(6.0, 0.0), Align2::LEFT_CENTER, name, theme::semibold(11.0), Color32::WHITE);
	}
}

/// A space whose outline or name is under the screen point. Its inside
/// doesn't count, so items in it (and box-select) still work.
pub fn hit(doc: &OpenDoc, data: &TakeoffData, pos: Pos2) -> Option<String> {
	for s in data.spaces.iter().rev().filter(|s| s.page == doc.page && !s.hidden) {
		let pts: Vec<Pos2> = s.points.iter().map(|q| doc.to_screen(*q)).collect();
		if geom::dist_to_polyline(pos, &pts, true) <= HIT_PX || label_rect(doc, s).is_some_and(|r| r.contains(pos)) {
			return Some(s.id.clone());
		}
	}
	None
}

/// A new space from an outline, named "Space N".
pub fn add(doc: &OpenDoc, points: Vec<Point>) -> String {
	let mut d = doc.doc.lock();
	let n = d.data.spaces.len();
	let color = CATEGORY_COLORS[(n + 1) % CATEGORY_COLORS.len()].to_string();
	let s = Space { id: new_id("sp"), page: doc.page, name: spaces::next_name(&d.data), color, points, hidden: false, meta: d.new_meta() };
	let id = s.id.clone();
	d.edit("Space", |t| t.spaces.push(s));
	id
}

/// The item totals: everything, or only what is in `filter` and the spaces inside it.
pub fn summary_for(d: &Document, filter: Option<&str>) -> Vec<SummaryLine> {
	let Some(f) = filter else { return takeoff_core::list::summary(d) };
	let sp = Spaces::new(&d.data);
	let of = sp.attribute(&d.data);
	summary_where(d, |id, _| sp.is_within(of.get(id).map(String::as_str), f))
}

impl SpacesPanel {
	/// Above the item cards while they show one space's totals.
	pub fn filter_note(&mut self, ui: &mut egui::Ui, doc: &OpenDoc, p: &Palette) {
		let Some(f) = self.filter.clone() else { return };
		let path = Spaces::new(&doc.doc.lock().data).path_name(&f);
		ui.horizontal(|ui| {
			ui.label(RichText::new(format!("{}  Totals in {path}", i::FILTER)).size(12.0).color(p.accent));
			if ui.small_button("Show everything").clicked() {
				self.filter = None;
			}
		});
	}

	/// The Spaces section of the Takeoff tab.
	pub fn ui(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, canvas: &mut Canvas, p: &Palette) {
		let (tree, groups): (Vec<(Space, usize, String)>, _) = {
			let d = doc.doc.lock();
			let sp = Spaces::new(&d.data);
			let tree = sp.tree().into_iter().map(|(s, depth)| (s.clone(), depth, sp.path_name(&s.id))).collect();
			(tree, if self.grouped { summary_by_space(&d) } else { vec![] })
		};
		if self.filter.as_ref().is_some_and(|f| !tree.iter().any(|(s, _, _)| &s.id == f)) {
			self.filter = None;
		}
		ui.add_space(10.0);
		ui.horizontal(|ui| {
			widgets::section_label(ui, p, "Spaces");
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.menu_button(RichText::new(format!("{}  New", i::PLUS)).size(12.5), |ui| {
					if ui.button(format!("{}  Draw a space", Tool::Space.icon())).on_hover_text(Tool::Space.hint()).clicked() {
						canvas.set_tool(Tool::Space);
						ui.close();
					}
					if ui.button(format!("{}  Fill a space", Tool::SpaceFill.icon())).on_hover_text(Tool::SpaceFill.hint()).clicked() {
						canvas.set_tool(Tool::SpaceFill);
						ui.close();
					}
				});
			});
		});
		if tree.is_empty() {
			widgets::hint(ui, p, "Outline levels, units and rooms to see the quantities in each. Spaces can sit inside each other.");
			return;
		}
		let mut go_to = None;
		for (s, depth, path) in &tree {
			ui.horizontal(|ui| {
				ui.add_space(*depth as f32 * 14.0);
				let id = s.id.clone();
				ui.menu_button(RichText::new(i::SQUARE).font(theme::icon_font(14.0)).color(hex(&s.color)), |ui| {
					if let Some(c) = crate::right_panel::color_row(ui, &s.color) {
						doc.doc.lock().edit("Space color", |d| {
							if let Some(x) = d.spaces.iter_mut().find(|x| x.id == id) {
								x.color = c;
							}
						});
					}
				})
				.response
				.on_hover_text("Color");
				match &mut self.renaming {
					Some((rid, buf)) if *rid == s.id => {
						let r = ui.add(egui::TextEdit::singleline(buf).desired_width(140.0));
						r.request_focus();
						if r.lost_focus() {
							let (id, name) = self.renaming.take().unwrap();
							if !name.trim().is_empty() {
								doc.doc.lock().edit("Rename space", |d| {
									if let Some(x) = d.spaces.iter_mut().find(|x| x.id == id) {
										x.name = name.trim().to_string();
									}
								});
							}
						}
					}
					_ => {
						let here = s.page == doc.page;
						let text = RichText::new(&s.name).size(12.5).color(if s.hidden { p.muted } else { p.text });
						let l = ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click())).on_hover_text(format!("{path} · page {}\nClick to select · double-click to rename", s.page));
						if l.double_clicked() {
							self.renaming = Some((s.id.clone(), s.name.clone()));
						} else if l.clicked() {
							go_to = Some((s.page, s.id.clone(), here));
						}
					}
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if widgets::icon_button(ui, p, i::TRASH_2, "Delete space (its quantities stay)", false).clicked() {
						let id = s.id.clone();
						doc.doc.lock().edit("Delete space", |d| d.spaces.retain(|x| x.id != id));
						doc.selection.remove(&s.id);
					}
					let (icon, tip) = if s.hidden { (i::EYE_OFF, "Show on the sheet") } else { (i::EYE, "Hide on the sheet (it still counts)") };
					if widgets::icon_button(ui, p, icon, tip, false).clicked() {
						let id = s.id.clone();
						doc.doc.lock().edit(if s.hidden { "Show space" } else { "Hide space" }, |d| {
							if let Some(x) = d.spaces.iter_mut().find(|x| x.id == id) {
								x.hidden = !x.hidden;
							}
						});
					}
					if s.page != doc.page {
						ui.label(RichText::new(format!("p{}", s.page)).size(11.0).color(p.muted));
					}
				});
			});
		}
		if let Some((page, id, here)) = go_to {
			if !here {
				doc.go_to_page(page);
			}
			doc.selection.clear();
			doc.selection.insert(id);
		}
		ui.add_space(4.0);
		ui.horizontal(|ui| {
			ui.label(RichText::new("Totals for").size(12.0).color(p.text2));
			let current = self.filter.as_ref().and_then(|f| tree.iter().find(|(s, _, _)| &s.id == f)).map(|(_, _, path)| path.clone()).unwrap_or_else(|| "Everything".into());
			egui::ComboBox::from_id_salt("space-filter").selected_text(current).width(170.0).show_ui(ui, |ui| {
				ui.selectable_value(&mut self.filter, None, "Everything");
				for (s, depth, _) in &tree {
					ui.selectable_value(&mut self.filter, Some(s.id.clone()), format!("{}{}", "   ".repeat(*depth), s.name));
				}
			});
			ui.toggle_value(&mut self.grouped, "By space").on_hover_text("Totals per space: each quantity counts once, in the deepest space it is in");
		});
		if self.grouped {
			for g in &groups {
				ui.add_space(4.0);
				ui.horizontal(|ui| {
					ui.add_space(g.depth as f32 * 14.0);
					widgets::swatch(ui, hex(&g.color), 10.0);
					ui.label(RichText::new(&g.name).font(theme::semibold(12.0)));
				});
				for l in &g.lines {
					ui.horizontal(|ui| {
						ui.add_space(g.depth as f32 * 14.0 + 18.0);
						widgets::swatch(ui, hex(&l.color), 8.0);
						ui.label(RichText::new(&l.name).size(12.0).color(p.text2));
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(&l.total_text).font(theme::semibold(12.0))));
					});
				}
			}
		}
	}
}

/// The Properties tab for a selected space. False when `id` isn't one.
pub fn props(ui: &mut egui::Ui, doc: &mut OpenDoc, id: &str, p: &Palette) -> bool {
	let (s, path, lines) = {
		let d = doc.doc.lock();
		let Some(s) = d.data.spaces.iter().find(|s| s.id == id).cloned() else { return false };
		let sp = Spaces::new(&d.data);
		let of = sp.attribute(&d.data);
		let lines: Vec<_> = summary_where(&d, |item, _| sp.is_within(of.get(item).map(String::as_str), id)).into_iter().filter(|l| l.items > 0).collect();
		(s, sp.path_name(id), lines)
	};
	ui.horizontal(|ui| {
		ui.label(RichText::new(i::LASSO).font(theme::icon_font(18.0)).color(p.accent));
		ui.vertical(|ui| {
			ui.label(RichText::new("Space").font(theme::semibold(14.5)));
			ui.label(RichText::new(&path).size(11.5).color(p.text2));
		});
	});
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Name");
	let mut name = s.name.clone();
	if ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY)).changed() && !name.trim().is_empty() {
		doc.doc.lock().edit("Rename space", |d| {
			if let Some(x) = d.spaces.iter_mut().find(|x| x.id == id) {
				x.name = name.trim().to_string();
			}
		});
	}
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Color");
	if let Some(c) = crate::right_panel::color_row(ui, &s.color) {
		doc.doc.lock().edit("Space color", |d| {
			if let Some(x) = d.spaces.iter_mut().find(|x| x.id == id) {
				x.color = c;
			}
		});
	}
	let mut hidden = s.hidden;
	if ui.checkbox(&mut hidden, "Hidden on the sheet (still counts)").changed() {
		doc.doc.lock().edit(if hidden { "Hide space" } else { "Show space" }, |d| {
			if let Some(x) = d.spaces.iter_mut().find(|x| x.id == id) {
				x.hidden = hidden;
			}
		});
	}
	ui.add_space(8.0);
	widgets::section_label(ui, p, "In this space");
	if lines.is_empty() {
		widgets::hint(ui, p, "Nothing taken off inside it yet.");
	}
	for l in &lines {
		ui.horizontal(|ui| {
			widgets::swatch(ui, hex(&l.color), 10.0);
			ui.label(RichText::new(&l.name).size(12.5));
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(&l.total_text).font(theme::semibold(12.5))));
		});
	}
	ui.add_space(8.0);
	if ui.button(RichText::new(format!("{}  Delete space", i::TRASH_2)).color(p.danger)).on_hover_text("Its quantities stay").clicked() {
		doc.doc.lock().edit("Delete space", |d| d.spaces.retain(|x| x.id != id));
		doc.selection.clear();
	}
	true
}
