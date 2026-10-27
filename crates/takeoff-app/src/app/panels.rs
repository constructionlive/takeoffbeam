//! The side panels and what they ask of the app: the Graph and Map views, opening
//! sheets from them, and following the agent around the drawing.

use super::*;

impl TakeoffApp {
	pub(super) fn panels(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		let active = self.active;
		if self.settings.left_panel {
			let mut action = None;
			let mut set_action = None;
			egui::Panel::left("left").resizable(true).default_size(236.0).size_range(170.0..=420.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
				ui.set_min_height(ui.available_height());
				ui.set_max_width(ui.available_width());
				self.left.set_tab = self.set.is_some();
				if let Some(d) = self.docs.get_mut(active) {
					let before = d.page;
					action = self.left.ui(ui, d, p);
					if d.view_mode == ViewMode::Graph && d.page != before {
						let page = d.page;
						d.map_go(&takeoff_core::docmap::sheet_id(page));
					}
				}
				if self.left.tab == 4 {
					let active_path = self.docs.get(active).map(|d| d.path.clone());
					if let Some(s) = &mut self.set {
						set_action = s.ui(ui, &self.docs, active_path.as_deref(), p);
					}
				}
			});
			if let Some(a) = action {
				self.page_action(a, &ctx);
			}
			if let Some(a) = set_action {
				self.set_action(a, &ctx);
			}
		}
		let mut map_action = None;
		if self.settings.right_panel {
			egui::Panel::right("right").resizable(true).default_size(340.0).size_range(260.0..=620.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |outer| {
				// Lay the contents out in a child that can't grow the panel: if anything
				// in it is too wide, egui would otherwise widen the panel past the window
				// and the canvas would paint over its left edge. Overflow just clips.
				let rect = outer.max_rect();
				let mut inner = outer.new_child(egui::UiBuilder::new().max_rect(rect).layout(*outer.layout()));
				inner.set_clip_rect(rect.intersect(outer.clip_rect()));
				outer.advance_cursor_after_rect(rect);
				let ui = &mut inner;
				let sel = self.docs.get(active).map(|d| d.selection.len()).unwrap_or(0);
				let props = if sel > 0 { format!("Properties ({sel})") } else { "Properties".into() };
				widgets::tabs(ui, p, &[(i::SIGMA, "Takeoff"), (i::LIST, &props), (i::SPARKLES, "AI"), (i::WAYPOINTS, "Map")], &mut self.right.tab);
				ui.separator();
				match self.right.tab {
					3 => {
						let chat = self.docs.get(active).map(|d| self.ai.map_chat(&self.settings, &d.path));
						if let (Some(d), Some(chat)) = (self.docs.get_mut(active), chat) {
							map_action = crate::map_panel::ui(ui, d, p, &chat);
						}
					}
					0 => {
						if let Some(d) = self.docs.get_mut(active) {
							self.right.takeoff(ui, d, &mut self.canvas, &mut self.settings, p);
						}
					}
					1 => {
						if let Some(d) = self.docs.get_mut(active) {
							self.right.properties(ui, d, &mut self.canvas, p);
						}
					}
					_ => {
						let doc = self.docs.get_mut(active);
						self.ai.ui(ui, doc, &mut self.canvas, &mut self.settings, p);
					}
				}
			});
		}
		if let Some(a) = map_action {
			self.map_action(a);
		}
		if self.settings.markups_list {
			let mut action = None;
			let r = egui::Panel::bottom("markups").resizable(true).show_separator_line(false).default_size(210.0).size_range(90.0..=600.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
				// Fill the panel even when the list is short, so its size sticks.
				ui.set_min_height(ui.available_height());
				if let Some(d) = self.docs.get_mut(active) {
					action = self.markups.ui(ui, d, p);
				}
			});
			let _ = r;
			match action {
				Some(ListAction::ExportCsv) => self.export_csv(false),
				Some(ListAction::ExportSummary) => self.export_csv(true),
				Some(ListAction::ExportXlsx) => self.export_xlsx(false),
				None => {}
			}
		}
		let graph = self.docs.get(active).is_some_and(|d| d.view_mode == ViewMode::Graph);
		if graph {
			let chat = self.docs.get(active).map(|d| self.ai.map_chat(&self.settings, &d.path));
			let mut action = None;
			egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas)).show(ui, |ui| {
				if let (Some(d), Some(chat)) = (self.docs.get_mut(active), &chat) {
					action = crate::graph_view::ui(ui, d, p, chat, self.settings.wheel_zooms);
				}
			});
			if let Some(a) = action {
				self.graph_action(a);
			}
			return;
		}
		egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas)).show(ui, |ui| {
			if let Some(d) = self.docs.get_mut(active) {
				let (snap, wheel) = (self.settings.snap, self.settings.wheel_zooms);
				self.canvas.user_moved = false;
				self.canvas.map_links = self.settings.map_links;
				self.canvas.ui(ui, d, p, snap, wheel);
				// Panning or zooming yourself while the AI works stops following it.
				if self.canvas.user_moved && self.settings.follow_agent && self.ai.busy() {
					self.settings.follow_agent = false;
					self.settings.save();
				}
				if self.settings.follow_agent && self.ai.busy() {
					let rect = ui.max_rect();
					let mut stop = false;
					egui::Area::new(egui::Id::new("follow-pill")).fixed_pos(egui::pos2(rect.center().x - 120.0, rect.top() + 48.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
						egui::Frame::popup(&ui.ctx().global_style()).inner_margin(egui::Margin::symmetric(12, 6)).corner_radius(14).show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.label(RichText::new(i::CROSSHAIR).font(theme::icon_font(13.0)).color(p.accent));
								ui.label(RichText::new("Following the AI").font(theme::semibold(12.5)));
								if ui.small_button("Stop").clicked() {
									stop = true;
								}
							});
						});
					});
					if stop {
						self.settings.follow_agent = false;
						self.settings.save();
					}
				}
				// Revision-compare bar.
				let mut close_compare = false;
				let mut select_flagged = None;
				if let Some(c) = &mut d.compare {
					let rect = ui.max_rect();
					egui::Area::new(egui::Id::new("compare-bar")).fixed_pos(egui::pos2(rect.left() + 12.0, rect.bottom() - 52.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
						egui::Frame::popup(&ui.ctx().global_style()).inner_margin(egui::Margin::symmetric(12, 8)).corner_radius(6).show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.label(RichText::new(i::GIT_COMPARE_ARROWS).font(theme::icon_font(14.0)).color(p.accent));
								let name = c.other.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
								let what = if c.revision { format!("Comparing with the previous revision ({name} p{})", c.other_page) } else { format!("Comparing with {name} p{}", c.other_page) };
								ui.label(RichText::new(what).font(theme::semibold(12.5)));
								ui.label(RichText::new("■ only here").color(Color32::from_rgb(230, 40, 40)).size(12.0));
								ui.label(RichText::new("■ only there").color(Color32::from_rgb(40, 90, 230)).size(12.0));
								if c.tex.is_some() {
									ui.label(RichText::new(format!("{:.1}% changed", c.changed * 100.0)).size(12.0).color(p.text2));
									if !c.flagged.is_empty() {
										let n = c.flagged.len();
										if ui.button(RichText::new(format!("{}  {n} item{} on changes", i::CIRCLE_ALERT, if n == 1 { "" } else { "s" })).color(Color32::from_rgb(0xD9, 0x8E, 0x04))).on_hover_text("Takeoff items and markups on parts of the sheet that changed: select them to check").clicked() {
											select_flagged = Some(c.flagged.clone());
										}
									}
								}
								ui.checkbox(&mut c.show_overlay, "Overlay");
								if ui.button(format!("{} Close", i::X)).clicked() {
									close_compare = true;
								}
							});
						});
					});
				}
				if close_compare {
					d.compare = None;
				}
				if let Some(ids) = select_flagged {
					d.selection = ids.into_iter().collect();
				}
				// Tool hint bubble at the top of the canvas.
				if self.canvas.tool != Tool::Select && self.canvas.tool != Tool::Pan {
					let rect = ui.max_rect();
					let text = format!("{}  {} — {}", self.canvas.tool.icon(), self.canvas.tool.label(), self.canvas.tool.hint());
					let painter = ui.painter();
					let g = painter.layout_no_wrap(text, theme::medium(12.0), p.text);
					let r = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 22.0), g.size() + Vec2::new(22.0, 10.0));
					painter.rect_filled(r.translate(Vec2::new(0.0, 2.0)), 6.0, Color32::from_black_alpha(40));
					painter.rect_filled(r, 6.0, p.panel);
					painter.rect_stroke(r, 6.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
					painter.galley(r.min + Vec2::new(11.0, 5.0), g, p.text);
				}
			}
		});
	}

	pub(super) fn graph_action(&mut self, a: GraphAction) {
		let active = self.active;
		match a {
			GraphAction::StartChat { instructions, fresh } => self.start_map_chat(instructions, fresh),
			GraphAction::Stop => self.ai.stop(),
			GraphAction::OpenChat => {
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
			GraphAction::Delete => self.dialog = Some(Dialog::DeleteMap),
			GraphAction::Focus(id) => {
				if let Some(d) = self.docs.get_mut(active) {
					d.map_go(&id);
				}
				self.show_map_tab();
			}
			GraphAction::Open(id) => self.open_on_sheet(&id),
		}
	}

	/// The Map tab, in view.
	pub(super) fn show_map_tab(&mut self) {
		self.settings.right_panel = true;
		self.right.tab = 3;
	}

	/// Page view, at a node of the map (onto the trail).
	pub(super) fn open_on_sheet(&mut self, id: &str) {
		if let Some(d) = self.docs.get_mut(self.active) {
			d.view_mode = ViewMode::Page;
			d.map_go(id);
		}
	}

	pub(super) fn map_action(&mut self, a: crate::map_panel::MapAction) {
		use crate::map_panel::MapAction;
		let active = self.active;
		match a {
			MapAction::Go(id) => {
				if let Some(d) = self.docs.get_mut(active) {
					d.map_go(&id);
				}
			}
			MapAction::Step(k) => {
				if let Some(d) = self.docs.get_mut(active) {
					d.map_step(k);
				}
			}
			MapAction::OpenOnSheet(id) => self.open_on_sheet(&id),
			MapAction::ShowInGraph(id) => {
				self.set_view_mode(ViewMode::Graph);
				if let Some(d) = self.docs.get_mut(active) {
					d.map_go(&id);
				}
			}
			MapAction::Ask { page, region, title } => {
				let Some(path) = self.docs.get(active).map(|d| d.path.clone()) else { return };
				let region = region.map(|r| (r.x, r.y, r.w, r.h)).unwrap_or((0.0, 0.0, 1.0, 1.0));
				self.ai.ask_about(takeoff_agent::ViewAttachment { drawing: path, page, region }, &format!("What is {title}, and what does it connect to?"));
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
			MapAction::StartMapping => self.set_view_mode(ViewMode::Graph),
			MapAction::OpenChat => {
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
		}
	}

	/// Map the set: a new conversation in the AI tab, on the chat's model.
	pub(super) fn start_map_chat(&mut self, instructions: String, fresh: bool) {
		let active = self.active;
		let Some(d) = self.docs.get_mut(active) else { return };
		if fresh {
			d.delete_map();
		}
		let resume = d.map.as_ref().is_some_and(|m| !m.sheets.is_empty());
		let mut text = String::from("Map this drawing set: build the document map.");
		if resume {
			text.push_str(" A map is already started: keep what is there and carry on with the sheets not mapped yet.");
		}
		if !instructions.is_empty() {
			text.push_str(&format!("\n\n{instructions}"));
		}
		let d = &self.docs[active];
		match self.ai.start_map_chat(text, d, &self.settings) {
			Ok(()) => {
				self.docs[active].map_status = None;
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
			Err(e) => self.docs[active].map_status = Some((e, true)),
		}
	}

	/// Take the canvas to where the AI is looking (switching tabs if needed).
	pub(super) fn follow_agent(&mut self) {
		let Some((path, page, region)) = self.ai.follow_request.take() else { return };
		if !self.settings.follow_agent {
			return;
		}
		if let Some(idx) = self.docs.iter().position(|d| d.path == path) {
			// A tab not shown yet doesn't know the canvas size; it's the same canvas.
			let canvas = self.docs.get(self.active).map(|d| d.canvas_rect).filter(|r| r.area() > 0.0);
			if let (Some(c), true) = (canvas, self.docs[idx].canvas_rect.area() <= 0.0) {
				self.docs[idx].canvas_rect = c;
			}
			self.active = idx;
			self.docs[idx].view_mode = ViewMode::Page;
			self.docs[idx].zoom_to(page, region);
		}
	}
}
