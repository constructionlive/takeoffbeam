//! Left panel: Pages (thumbnails + page tools), Bookmarks, Layers, Search.

use std::collections::HashSet;

use eframe::egui::{self, Color32, RichText, ScrollArea, Sense, Stroke, StrokeKind, Vec2};

use crate::docstate::OpenDoc;
use crate::icons as i;
use crate::theme::{self, Palette};
use crate::widgets;

#[derive(Default)]
pub struct LeftPanel {
	pub tab: usize,
	pub search_text: String,
	pub match_case: bool,
	pub whole_word: bool,
	layer_filter: String,
	/// Pages ticked for page tools.
	pub picked: HashSet<u32>,
}

/// Page actions the app runs (they rewrite the PDF and remap the takeoff).
#[derive(Debug, Clone)]
pub enum PageAction {
	Rotate { pages: Vec<u32>, quarter_turns: u32 },
	Delete { pages: Vec<u32> },
	Extract { pages: Vec<u32> },
	InsertBlank { at: u32 },
	InsertFile { at: u32 },
	Move { from: u32, to: u32 },
	Split,
	Merge,
}

impl LeftPanel {
	pub fn focus_search(&mut self) {
		self.tab = 3;
	}

	pub fn ui(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) -> Option<PageAction> {
		let mut action = None;
		widgets::tabs(ui, p, &[(i::IMAGES, "Pages"), (i::BOOKMARK, ""), (i::LAYERS, ""), (i::SEARCH, "")], &mut self.tab);
		ui.separator();
		match self.tab {
			0 => action = self.pages(ui, doc, p),
			1 => self.bookmarks(ui, doc, p),
			2 => self.layers(ui, doc, p),
			_ => self.search(ui, doc, p),
		}
		action
	}

	fn pages(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) -> Option<PageAction> {
		let mut action = None;
		let n = doc.page_count();
		let picked: Vec<u32> = {
			let mut v: Vec<u32> = self.picked.iter().copied().filter(|x| *x <= n).collect();
			v.sort();
			if v.is_empty() {
				vec![doc.page]
			} else {
				v
			}
		};
		ui.horizontal_wrapped(|ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			if widgets::icon_button(ui, p, i::ROTATE_CW, "Rotate clockwise", false).clicked() {
				action = Some(PageAction::Rotate { pages: picked.clone(), quarter_turns: 1 });
			}
			if widgets::icon_button(ui, p, i::ROTATE_CCW, "Rotate counter-clockwise", false).clicked() {
				action = Some(PageAction::Rotate { pages: picked.clone(), quarter_turns: 3 });
			}
			if widgets::icon_button(ui, p, i::FILE_PLUS, "Insert blank page after", false).clicked() {
				action = Some(PageAction::InsertBlank { at: picked.last().copied().unwrap_or(doc.page) + 1 });
			}
			if widgets::icon_button(ui, p, i::FILE_INPUT, "Insert pages from a PDF after", false).clicked() {
				action = Some(PageAction::InsertFile { at: picked.last().copied().unwrap_or(doc.page) + 1 });
			}
			if widgets::icon_button(ui, p, i::FILE_OUTPUT, "Extract selected pages to a new PDF", false).clicked() {
				action = Some(PageAction::Extract { pages: picked.clone() });
			}
			if widgets::icon_button(ui, p, i::SPLIT, "Split document…", false).clicked() {
				action = Some(PageAction::Split);
			}
			if widgets::icon_button(ui, p, i::COMBINE, "Merge PDFs…", false).clicked() {
				action = Some(PageAction::Merge);
			}
			if widgets::icon_button(ui, p, i::TRASH_2, "Delete selected pages", false).clicked() && n > picked.len() as u32 {
				action = Some(PageAction::Delete { pages: picked.clone() });
			}
		});
		let label = if self.picked.is_empty() { format!("{n} page{} · click a thumbnail's corner to pick several", if n == 1 { "" } else { "s" }) } else { format!("{} of {n} picked · Esc to clear", self.picked.len()) };
		widgets::hint(ui, p, &label);
		if !self.picked.is_empty() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
			self.picked.clear();
		}
		ui.add_space(4.0);
		// Thumbnails never widen the panel (that would feed back every frame).
		let width = ui.available_width();
		let thumb_w = (width - 36.0).clamp(80.0, 220.0);
		let counts = {
			let d = doc.doc.lock();
			let mut c = vec![0usize; n as usize + 1];
			for m in &d.data.marks {
				if let Some(x) = c.get_mut(m.page as usize) {
					*x += 1;
				}
			}
			for m in &d.data.measurements {
				if let Some(x) = c.get_mut(m.page as usize) {
					*x += 1;
				}
			}
			for a in &d.data.annotations {
				if let Some(x) = c.get_mut(a.page as usize) {
					*x += 1;
				}
			}
			c
		};
		let row_h = {
			let (w, h) = doc.page_size(1);
			thumb_w * (h / w) as f32 + 30.0
		};
		ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, n as usize, |ui, range| {
			ui.set_max_width(width);
			for idx in range {
				let page = idx as u32 + 1;
				let (w, h) = doc.page_size(page);
				let th = thumb_w * (h / w) as f32;
				ui.vertical_centered(|ui| {
					let (rect, resp) = ui.allocate_exact_size(Vec2::new(thumb_w, th), Sense::click_and_drag());
					let current = page == doc.page;
					let painter = ui.painter();
					painter.rect_filled(rect.translate(Vec2::new(0.0, 2.0)), 2.0, Color32::from_black_alpha(if p.dark { 70 } else { 20 }));
					painter.rect_filled(rect, 2.0, Color32::WHITE);
					if ui.is_rect_visible(rect) {
						if let Some(t) = doc.thumb(page) {
							painter.image(t.id(), rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)), Color32::WHITE);
						}
					}
					let stroke = if current { Stroke::new(2.5, p.accent) } else if resp.hovered() { Stroke::new(1.0, p.text2) } else { Stroke::new(1.0, p.border) };
					painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Outside);
					// Pick box (top-left) for multi-page actions.
					let pick = egui::Rect::from_min_size(rect.left_top() + Vec2::splat(6.0), Vec2::splat(16.0));
					let picked = self.picked.contains(&page);
					if picked || resp.hovered() {
						painter.rect_filled(pick, 3.0, if picked { p.accent_fill } else { Color32::from_white_alpha(220) });
						painter.rect_stroke(pick, 3.0, Stroke::new(1.0, if picked { p.accent_fill } else { Color32::from_gray(150) }), StrokeKind::Inside);
						if picked {
							painter.text(pick.center(), egui::Align2::CENTER_CENTER, i::CHECK, theme::icon_font(11.0), p.on_accent);
						}
					}
					if counts[page as usize] > 0 {
						let txt = counts[page as usize].to_string();
						let g = painter.layout_no_wrap(txt, theme::semibold(10.0), Color32::WHITE);
						let br = egui::Rect::from_min_size(rect.right_top() + Vec2::new(-g.size().x - 14.0, 6.0), g.size() + Vec2::new(8.0, 2.0));
						painter.rect_filled(br, br.height() / 2.0, p.accent_fill);
						painter.galley(br.min + Vec2::new(4.0, 1.0), g, p.on_accent);
					}
					if resp.clicked() {
						if resp.interact_pointer_pos().is_some_and(|pos| pick.expand(4.0).contains(pos)) || ui.input(|i| i.modifiers.command || i.modifiers.shift) {
							if !self.picked.remove(&page) {
								self.picked.insert(page);
							}
						} else {
							doc.go_to_page(page);
						}
					}
					// Drag a thumbnail onto another to move the page.
					if resp.drag_stopped() {
						if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
							let rel = ((pos.y - rect.top()) / row_h).round() as i64;
							let to = (page as i64 + rel).clamp(1, n as i64) as u32;
							if to != page {
								action = Some(PageAction::Move { from: page, to });
							}
						}
					}
					resp.context_menu(|ui| {
						if ui.button(format!("{}  Rotate clockwise", i::ROTATE_CW)).clicked() {
							action = Some(PageAction::Rotate { pages: vec![page], quarter_turns: 1 });
							ui.close();
						}
						if ui.button(format!("{}  Extract page", i::FILE_OUTPUT)).clicked() {
							action = Some(PageAction::Extract { pages: vec![page] });
							ui.close();
						}
						if ui.button(format!("{}  Insert blank page after", i::FILE_PLUS)).clicked() {
							action = Some(PageAction::InsertBlank { at: page + 1 });
							ui.close();
						}
						if n > 1 && ui.button(format!("{}  Delete page", i::TRASH_2)).clicked() {
							action = Some(PageAction::Delete { pages: vec![page] });
							ui.close();
						}
					});
					let label = doc.page_label(page);
					let text = if label.starts_with("Page ") { format!("{page}") } else { format!("{page} · {label}") };
					ui.label(RichText::new(text).size(11.5).color(if current { p.text } else { p.text2 }).font(if current { theme::semibold(11.5) } else { theme::medium(11.5) }));
					ui.add_space(6.0);
				});
			}
		});
		action
	}

	fn bookmarks(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) {
		if doc.outline.is_empty() {
			widgets::empty_state(ui, p, i::BOOKMARK, "No bookmarks", "This PDF has no outline.");
			return;
		}
		let items = doc.outline.clone();
		ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
			for b in &items {
				ui.horizontal(|ui| {
					ui.add_space(b.depth as f32 * 14.0);
					let current = b.page == Some(doc.page);
					let text = RichText::new(&b.title).color(if current { p.accent } else { p.text }).size(12.5);
					let resp = ui.add(egui::Label::new(text).truncate().sense(Sense::click()));
					if let Some(pg) = b.page {
						if resp.clicked() {
							doc.go_to_page(pg);
						}
						resp.on_hover_text(format!("Page {pg}"));
					}
				});
			}
		});
	}

	fn layers(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) {
		if doc.layers.is_empty() {
			widgets::empty_state(ui, p, i::LAYERS, "No layers", "This PDF has no optional-content layers.");
			return;
		}
		let mut hidden = doc.hidden_layers();
		let mut changed = false;
		ui.add(egui::TextEdit::singleline(&mut self.layer_filter).hint_text(format!("{} Filter layers", i::FILTER)).desired_width(f32::INFINITY));
		ui.horizontal(|ui| {
			if ui.small_button(format!("{} Show all", i::EYE)).clicked() {
				hidden.clear();
				changed = true;
			}
			if ui.small_button(format!("{} Hide all", i::EYE_OFF)).clicked() {
				hidden = doc.layers.iter().cloned().collect();
				changed = true;
			}
		});
		widgets::hint(ui, p, &format!("{} layers · {} hidden · the AI sees the same layers", doc.layers.len(), hidden.len()));
		let filter = self.layer_filter.to_lowercase();
		let descriptions: std::collections::HashMap<String, String> =
			doc.doc.lock().data.layers.iter().filter_map(|l| l.description.clone().map(|d| (l.name.clone(), d))).collect();
		ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
			for name in doc.layers.clone().iter().filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter)) {
				let visible = !hidden.contains(name);
				ui.horizontal(|ui| {
					let icon = if visible { i::EYE } else { i::EYE_OFF };
					if widgets::icon_button(ui, p, icon, if visible { "Hide layer" } else { "Show layer" }, false).clicked() {
						if visible {
							hidden.insert(name.clone());
						} else {
							hidden.remove(name);
						}
						changed = true;
					}
					let resp = ui.add(egui::Label::new(RichText::new(name).color(if visible { p.text } else { p.muted }).size(12.5)).truncate());
					if let Some(d) = descriptions.get(name) {
						resp.on_hover_text(d);
					}
				});
			}
		});
		if changed {
			doc.set_hidden_layers(hidden);
		}
	}

	fn search(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) {
		let resp = ui.add(egui::TextEdit::singleline(&mut self.search_text).hint_text(format!("{}  Search drawing text", i::SEARCH)).desired_width(f32::INFINITY));
		if self.tab == 3 && doc.search.is_none() && self.search_text.is_empty() {
			resp.request_focus();
		}
		ui.horizontal(|ui| {
			let a = ui.checkbox(&mut self.match_case, "Match case").changed();
			let b = ui.checkbox(&mut self.whole_word, "Whole words").changed();
			if (a || b) && !self.search_text.is_empty() {
				doc.start_search(self.search_text.clone(), self.match_case, self.whole_word);
			}
		});
		if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || (resp.changed() && self.search_text.len() >= 3) {
			doc.start_search(self.search_text.clone(), self.match_case, self.whole_word);
		}
		let Some(s) = &doc.search else {
			widgets::hint(ui, p, "Finds text across every sheet: notes, tags, room names, title blocks.");
			return;
		};
		let status = if s.done { format!("{} results", s.hits.len()) } else { format!("{} results so far…", s.hits.len()) };
		widgets::hint(ui, p, &status);
		let hits = s.hits.clone();
		let mut go = None;
		ScrollArea::vertical().auto_shrink(false).show_rows(ui, 44.0, hits.len(), |ui, range| {
			for idx in range {
				let h = &hits[idx];
				let active = doc.search.as_ref().and_then(|s| s.active) == Some(idx);
				let frame = egui::Frame::new().fill(if active { p.hover } else { Color32::TRANSPARENT }).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 4));
				let r = frame.show(ui, |ui| {
					ui.set_width(ui.available_width());
					ui.label(RichText::new(doc.page_label(h.page)).font(theme::semibold(11.0)).color(p.text2));
					ui.add(egui::Label::new(RichText::new(&h.context).size(12.0).color(p.text)).truncate());
				});
				if r.response.interact(Sense::click()).clicked() {
					go = Some(idx);
				}
			}
		});
		if let Some(idx) = go {
			let h = hits[idx].clone();
			if let Some(s) = &mut doc.search {
				s.active = Some(idx);
			}
			doc.zoom_to(h.page, (h.rect.0 - 0.05, h.rect.1 - 0.05, h.rect.2 + 0.1, h.rect.3 + 0.1));
		}
	}
}
