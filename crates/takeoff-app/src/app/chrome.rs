//! The window's chrome: top bar, tool buttons, menus, keyboard shortcuts, status bar
//! and the start screen.

use super::*;

impl TakeoffApp {
	pub(super) fn top_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		let inset = ctx.input(|i| fullscreen_top_inset(i.viewport()));
		egui::Panel::top("menu").frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(8, 4)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			if inset > 0.0 {
				ui.add_space(inset);
			}
			ui.horizontal(|ui| {
				// Brand mark.
				let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
				ui.painter().rect_filled(r, 4.0, p.accent_fill);
				ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, i::RULER, theme::icon_font(12.0), p.on_accent);
				ui.label(RichText::new("Takeoff Desk").font(theme::semibold(13.5)));
				ui.add_space(8.0);
				self.menus(ui, p);
				ui.add_space(8.0);
				// Document tabs.
				let mut close = None;
				for (idx, d) in self.docs.iter().enumerate() {
					let active = idx == self.active;
					let dirty = d.is_dirty();
					let frame = egui::Frame::new()
						.fill(if active { p.app_bg } else { Color32::TRANSPARENT })
						.stroke(if active { Stroke::new(1.0, p.border) } else { Stroke::NONE })
						.corner_radius(4)
						.inner_margin(egui::Margin::symmetric(8, 3));
					frame.show(ui, |ui| {
						ui.horizontal(|ui| {
							ui.spacing_mut().item_spacing.x = 6.0;
							ui.label(RichText::new(i::FILE_TEXT).font(theme::icon_font(12.0)).color(if active { p.accent } else { p.muted }));
							let name = if d.name.chars().count() > 34 { format!("{}…", d.name.chars().take(33).collect::<String>()) } else { d.name.clone() };
							let r = ui.add(egui::Label::new(RichText::new(name).size(12.5).color(if active { p.text } else { p.text2 })).sense(egui::Sense::click()));
							if r.clicked() {
								self.active = idx;
							}
							r.on_hover_text(d.path.display().to_string());
							if dirty {
								ui.label(RichText::new("●").size(9.0).color(p.accent));
							}
							if ui.add(egui::Button::new(RichText::new(i::X).font(theme::icon_font(11.0)).color(p.muted)).frame(false)).clicked() {
								close = Some(idx);
							}
						});
					});
				}
				if let Some(c) = close {
					self.active = c;
					self.close_active();
				}
				if ui.add(egui::Button::new(RichText::new(i::PLUS).font(theme::icon_font(14.0)).color(p.text2)).frame(false)).on_hover_text("Open drawing (⌘O)").clicked() {
					self.open_dialog(&ctx);
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if ui.add(egui::Button::new(RichText::new(format!("{}  Feedback", i::MESSAGE_SQUARE_TEXT)).size(12.5).color(p.text2)).frame(false)).on_hover_text("Email the Takeoff Desk team (rahul@construction.live)").clicked() {
						self.send_feedback();
					}
					if self.updater.available(&self.settings.skipped_update).is_some() && self.updater.dismissed {
						if ui.add(egui::Button::new(RichText::new(format!("{}  Update", i::DOWNLOAD)).size(12.5).color(p.accent)).frame(false)).on_hover_text("A new version is available").clicked() {
							self.updater.dismissed = false;
						}
					}
				});
			});
		});
		if self.docs.is_empty() {
			return;
		}
		egui::Panel::top("tools").frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(8, 4)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			ui.horizontal(|ui| {
				ui.spacing_mut().item_spacing.x = 2.0;
				let undo = self.doc().and_then(|d| d.doc.lock().undo_label().map(str::to_string));
				let redo = self.doc().and_then(|d| d.doc.lock().redo_label().map(str::to_string));
				if widgets::icon_button(ui, p, i::UNDO_2, &undo.clone().map(|l| format!("Undo {l} (⌘Z)")).unwrap_or("Nothing to undo".into()), false).clicked() {
					if let Some(d) = self.doc() {
						d.doc.lock().undo();
					}
				}
				if widgets::icon_button(ui, p, i::REDO_2, &redo.clone().map(|l| format!("Redo {l} (⇧⌘Z)")).unwrap_or("Nothing to redo".into()), false).clicked() {
					if let Some(d) = self.doc() {
						d.doc.lock().redo();
					}
				}
				separator(ui, p);
				for t in [Tool::Select, Tool::Pan] {
					self.tool_button(ui, p, t);
				}
				separator(ui, p);
				group_label(ui, p, "Measure");
				for t in Tool::MEASURE {
					self.tool_button(ui, p, t);
				}
				separator(ui, p);
				group_label(ui, p, "Markup");
				for t in Tool::MARKUP {
					self.tool_button(ui, p, t);
				}
				separator(ui, p);
				let snap = self.settings.snap;
				if widgets::icon_button(ui, p, i::MAGNET, if snap { "Snap to drawing lines: on (S) · hold Alt to bypass" } else { "Snap to drawing lines: off (S)" }, snap).clicked() {
					self.settings.snap = !snap;
					self.settings.save();
				}
				let badges = self.canvas.show_badges;
				if widgets::icon_button(ui, p, i::HASH, if badges { "Hide number badges" } else { "Show number badges" }, badges).clicked() {
					self.canvas.show_badges = !badges;
					self.settings.show_badges = !badges;
					self.settings.save();
				}
				if self.docs.get(self.active).is_some_and(|d| d.map.is_some()) {
					let links = self.settings.map_links;
					if widgets::icon_button(ui, p, i::EXTERNAL_LINK, if links { "Map links on the sheet: on · click a callout to go to what it names" } else { "Map links on the sheet: off" }, links).clicked() {
						self.settings.map_links = !links;
						self.settings.save();
					}
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					let ai_on = self.settings.right_panel && self.right.tab == 2;
					let mode = self.docs.get(self.active).map(|d| d.view_mode);
					let r = ui.add(egui::Button::new(RichText::new(format!("{}  AI", i::SPARKLES)).font(theme::semibold(12.5)).color(if ai_on { p.on_accent } else { p.accent })).fill(if ai_on { p.accent_fill } else { Color32::TRANSPARENT }).stroke(Stroke::new(1.0, p.accent)).corner_radius(4));
					if r.on_hover_text("Ask the drawing agent (⌘J)").clicked() {
						self.toggle_ai();
					}
					if self.ai.busy() {
						ui.spinner();
					}
					ui.add_space(10.0);
					if let Some(m) = mode {
						self.view_switch(ui, p, m);
					}
				});
			});
		});
	}

	/// Page | Graph: the sheet itself, or the map of the whole set.
	pub(super) fn view_switch(&mut self, ui: &mut egui::Ui, p: &Palette, mode: ViewMode) {
		let building = self.docs.get(self.active).is_some_and(|d| self.ai.map_chat(&self.settings, &d.path).running_here);
		let frame = egui::Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(5).inner_margin(egui::Margin::same(2));
		frame.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			// Right-to-left: Graph is added first so it sits on the right.
			for (m, icon, label, tip) in [(ViewMode::Graph, i::WAYPOINTS, "Graph", "The map of the whole set: sheets, details, notes and how they relate (⌘G)"), (ViewMode::Page, i::FILE_TEXT, "Page", "The sheet (⌘G)")] {
				let on = mode == m;
				let text = if m == ViewMode::Graph && building { format!("{icon}  {label} …") } else { format!("{icon}  {label}") };
				let r = ui.add(egui::Button::new(RichText::new(text).size(12.5).color(if on { p.text } else { p.text2 })).fill(if on { p.hover } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).corner_radius(4));
				if r.on_hover_text(tip).clicked() {
					self.set_view_mode(m);
				}
			}
		});
	}

	pub(super) fn set_view_mode(&mut self, m: ViewMode) {
		if let Some(d) = self.docs.get_mut(self.active) {
			if d.view_mode != m {
				// The graph opens on what the Map tab shows (the sheet on screen, or the area picked on it).
				let focus = d.map_focus();
				d.view_mode = m;
				if let (ViewMode::Graph, Some(f)) = (m, focus) {
					d.graph.select_node(&f);
				}
			}
		}
	}

	/// Single page, continuous or spreads, for every open drawing (and remembered).
	pub(super) fn set_page_layout(&mut self, l: crate::layout::PageLayout) {
		self.settings.page_layout = l;
		self.settings.save();
		for d in &mut self.docs {
			d.set_layout(l);
		}
	}

	pub(super) fn toggle_ai(&mut self) {
		if self.settings.right_panel && self.right.tab == 2 {
			self.right.tab = 0;
		} else {
			self.settings.right_panel = true;
			self.right.tab = 2;
		}
	}

	pub(super) fn tool_button(&mut self, ui: &mut egui::Ui, p: &Palette, t: Tool) {
		let tip = match t.shortcut() {
			Some(k) => format!("{} ({k})\n{}", t.label(), t.hint()),
			None => t.label().to_string(),
		};
		if widgets::icon_button(ui, p, t.icon(), &tip, self.canvas.tool == t).clicked() {
			self.set_view_mode(ViewMode::Page);
			self.canvas.set_tool(t);
			if matches!(t, Tool::Annot(_)) {
				self.settings.right_panel = true;
				if self.docs.get(self.active).is_some_and(|d| d.selection.is_empty()) {
					self.right.tab = 1;
				}
			}
		}
	}

	pub(super) fn menus(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		let has_doc = !self.docs.is_empty();
		ui.menu_button("File", |ui| {
			ui.set_min_width(240.0);
			if ui.button(format!("{}  Open…    ⌘O", i::FOLDER_OPEN)).clicked() {
				ui.close();
				self.open_dialog(&ctx);
			}
			ui.menu_button(format!("{}  Open recent", i::HISTORY), |ui| {
				if self.settings.recent.is_empty() {
					ui.label(RichText::new("Nothing yet").color(p.muted));
				}
				for r in self.settings.recent.clone() {
					let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
					if ui.button(name).on_hover_text(r.display().to_string()).clicked() {
						ui.close();
						self.open(r, &ctx);
					}
				}
			});
			self.set_menu(ui, &ctx);
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Save takeoff    ⌘S", i::SAVE))).clicked() {
				if let Some(d) = self.doc() {
					d.save_now();
				}
				ui.close();
			}
			ui.menu_button(format!("{}  Export", i::FILE_OUTPUT), |ui| {
				ui.set_min_width(260.0);
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Marked-up PDF (flattened)…", i::FILE_TEXT))).clicked() {
					ui.close();
					self.export_pdf();
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Page image with markups (PNG)…", i::IMAGE))).clicked() {
					ui.close();
					self.export_png();
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Markups list (CSV)…", i::FILE_SPREADSHEET))).clicked() {
					ui.close();
					self.export_csv(false);
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Quantity summary (CSV)…", i::SIGMA))).clicked() {
					ui.close();
					self.export_csv(true);
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Markups list + summary (Excel)…", i::FILE_SPREADSHEET))).clicked() {
					ui.close();
					self.export_xlsx(false);
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Quantity summary (Excel)…", i::SIGMA))).clicked() {
					ui.close();
					self.export_xlsx(true);
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Takeoff file (JSON)…", i::DOWNLOAD))).clicked() {
					ui.close();
					self.export_json();
				}
			});
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Close    ⌘W", i::X))).clicked() {
				ui.close();
				self.close_active();
			}
		});
		ui.menu_button("Edit", |ui| {
			ui.set_min_width(220.0);
			let enabled = has_doc;
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Undo    ⌘Z", i::UNDO_2))).clicked() {
				if let Some(d) = self.doc() {
					d.doc.lock().undo();
				}
				ui.close();
			}
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Redo    ⇧⌘Z", i::REDO_2))).clicked() {
				if let Some(d) = self.doc() {
					d.doc.lock().redo();
				}
				ui.close();
			}
			ui.separator();
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Select all on page    ⌘A", i::MOUSE_POINTER_2))).clicked() {
				if let Some(d) = self.docs.get_mut(self.active) {
					self.canvas.select_all(d);
				}
				ui.close();
			}
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Confirm all AI marks", i::CHECK))).clicked() {
				if let Some(d) = self.doc() {
					d.doc.lock().edit("Confirm all", |t| t.marks.iter_mut().for_each(|m| m.status = MarkStatus::Confirmed));
				}
				ui.close();
			}
			ui.separator();
			if ui.button(format!("{}  Preferences…", i::SETTINGS)).clicked() {
				self.dialog = Some(Dialog::Settings);
				ui.close();
			}
		});
		ui.menu_button("Document", |ui| {
			ui.set_min_width(240.0);
			let page = self.docs.get(self.active).map(|d| d.page).unwrap_or(1);
			let picked = |s: &Self| -> Vec<u32> {
				let mut v: Vec<u32> = s.left.picked.iter().copied().collect();
				v.sort();
				if v.is_empty() { vec![page] } else { v }
			};
			let mut act = None;
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Rotate pages clockwise", i::ROTATE_CW))).clicked() {
				act = Some(PageAction::Rotate { pages: picked(self), quarter_turns: 1 });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Insert blank page", i::FILE_PLUS))).clicked() {
				act = Some(PageAction::InsertBlank { at: page + 1 });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Insert pages from PDF…", i::FILE_INPUT))).clicked() {
				act = Some(PageAction::InsertFile { at: page + 1 });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Extract pages…", i::FILE_OUTPUT))).clicked() {
				act = Some(PageAction::Extract { pages: picked(self) });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Delete pages", i::FILE_MINUS))).clicked() {
				act = Some(PageAction::Delete { pages: picked(self) });
			}
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Split document…", i::SPLIT))).clicked() {
				act = Some(PageAction::Split);
			}
			if ui.button(format!("{}  Merge PDFs…", i::COMBINE)).clicked() {
				act = Some(PageAction::Merge);
			}
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Compare revisions…", i::GIT_COMPARE_ARROWS))).on_hover_text("Overlay another revision: red = only here, blue = only there").clicked() {
				ui.close();
				if let Some(other) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_title("Choose the other revision").pick_file() {
					if let Some(d) = self.docs.get_mut(self.active) {
						let pg = d.page;
						d.start_compare(other, pg, &ctx);
					}
				}
			}
			let revised = self.docs.get(self.active).is_some_and(|d| d.doc.lock().data.previous_revision(d.page).is_some());
			if ui.add_enabled(revised, egui::Button::new(format!("{}  Compare with the previous revision", i::HISTORY))).on_hover_text("This sheet was slip-sheeted: overlay the revision it replaced, and see which items sit on what changed").clicked() {
				act = Some(PageAction::CompareRevision { page });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Slip-sheet new revision…", i::REFRESH_CW))).on_hover_text("Replace sheets with a newer revision, matched by sheet number; the takeoff moves onto them").clicked() {
				act = Some(PageAction::SlipSheet);
			}
			if let Some(a) = act {
				ui.close();
				self.page_action(a, &ctx);
			}
		});
		ui.menu_button("View", |ui| {
			ui.set_min_width(220.0);
			let mode = self.docs.get(self.active).map(|d| d.view_mode);
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Page view", i::FILE_TEXT)).selected(mode == Some(ViewMode::Page))).clicked() {
				self.set_view_mode(ViewMode::Page);
				ui.close();
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Graph view    ⌘G", i::WAYPOINTS)).selected(mode == Some(ViewMode::Graph))).clicked() {
				self.set_view_mode(ViewMode::Graph);
				ui.close();
			}
			ui.separator();
			ui.checkbox(&mut self.settings.left_panel, "Pages panel");
			ui.checkbox(&mut self.settings.right_panel, "Side panel");
			ui.checkbox(&mut self.settings.markups_list, "Markups list");
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Fit page    ⌘0", i::MAXIMIZE))).clicked() {
				if let Some(d) = self.doc() {
					d.fit_page();
				}
				ui.close();
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Fit width", i::ARROW_LEFT_RIGHT))).clicked() {
				if let Some(d) = self.doc() {
					d.fit_width();
				}
				ui.close();
			}
			ui.separator();
			ui.label(RichText::new("Page layout").color(p.muted).size(11.5));
			let now = self.settings.page_layout;
			for (l, icon, label, key) in LAYOUTS {
				if ui.add(egui::Button::new(format!("{icon}  {label}    {key}")).selected(now.is(l))).clicked() {
					self.set_page_layout(now.with(l.continuous, l.spreads));
					ui.close();
				}
			}
			let mut cover = now.cover;
			if ui.add_enabled(now.spreads, egui::Checkbox::new(&mut cover, "Cover page on its own")).changed() {
				self.set_page_layout(crate::layout::PageLayout { cover, ..now });
			}
			ui.separator();
			ui.label(RichText::new("Theme").color(p.muted).size(11.5));
			let mut t = self.settings.theme;
			ui.horizontal(|ui| {
				ui.selectable_value(&mut t, ThemeChoice::System, format!("{} System", i::MONITOR));
				ui.selectable_value(&mut t, ThemeChoice::Light, format!("{} Light", i::SUN));
				ui.selectable_value(&mut t, ThemeChoice::Dark, format!("{} Dark", i::MOON));
			});
			if t != self.settings.theme {
				self.settings.theme = t;
				self.settings.save();
				self.theme_applied = None;
			}
		});
		ui.menu_button("Help", |ui| {
			if ui.button(format!("{}  Send feedback…", i::MESSAGE_SQUARE_TEXT)).clicked() {
				ui.close();
				self.send_feedback();
			}
			if ui.button(format!("{}  Copy diagnostics", i::COPY)).on_hover_text("Version, OS, model and the last crash, for a bug report").clicked() {
				ui.close();
				ctx.copy_text(crate::crash::diagnostics(&self.settings.model));
				self.toast("Copied diagnostics", false);
			}
			if ui.button(format!("{}  Show logs", i::FOLDER_OPEN)).on_hover_text("The folder with crash reports").clicked() {
				ui.close();
				self.show_logs();
			}
			if ui.button(format!("{}  Check for updates…", i::REFRESH_CW)).clicked() {
				ui.close();
				self.updater.manual = true;
				self.updater.check(&ctx);
			}
			ui.separator();
			if ui.button(format!("{}  Keyboard shortcuts    ?", i::KEYBOARD)).clicked() {
				self.dialog = Some(Dialog::Shortcuts);
				ui.close();
			}
			if ui.button(format!("{}  About Takeoff Desk", i::INFO)).clicked() {
				self.dialog = Some(Dialog::About);
				ui.close();
			}
		});
	}

	pub(super) fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let active = self.active;
		let mut pick = None;
		egui::Panel::bottom("status").exact_size(30.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 3)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			let Some(d) = self.docs.get_mut(active) else { return };
			ui.horizontal_centered(|ui| {
				ui.spacing_mut().item_spacing.x = 4.0;
				let n = d.page_count();
				if widgets::icon_button(ui, p, i::CHEVRON_LEFT, "Previous page (PgUp)", false).clicked() && d.page > 1 {
					let pg = d.page - 1;
					d.go_to_page(pg);
				}
				if self.page_input.is_empty() || !ui.memory(|m| m.has_focus(egui::Id::new("page-input"))) {
					self.page_input = d.page.to_string();
				}
				let r = ui.add(egui::TextEdit::singleline(&mut self.page_input).id(egui::Id::new("page-input")).desired_width(36.0).horizontal_align(egui::Align::Center));
				if r.lost_focus() {
					if let Ok(v) = self.page_input.trim().parse::<u32>() {
						d.go_to_page(v);
					}
					self.page_input.clear();
				}
				ui.label(RichText::new(format!("/ {n}")).color(p.text2));
				if widgets::icon_button(ui, p, i::CHEVRON_RIGHT, "Next page (PgDn)", false).clicked() && d.page < n {
					let pg = d.page + 1;
					d.go_to_page(pg);
				}
				let label = d.page_label(d.page);
				if !label.starts_with("Page ") {
					ui.label(RichText::new(label).font(theme::semibold(12.0)));
				}
				ui.add_space(10.0);
				ui.separator();
				// Zoom.
				if widgets::icon_button(ui, p, i::ZOOM_OUT, "Zoom out (⌘−)", false).clicked() {
					let c = d.canvas_rect.center();
					d.zoom_at(c, 1.0 / 1.25);
				}
				let pct = (d.zoom * 100.0 * 72.0 / 72.0).round();
				egui::ComboBox::from_id_salt("zoom").selected_text(format!("{pct}%")).width(76.0).show_ui(ui, |ui| {
					if ui.selectable_label(false, "Fit page").clicked() {
						d.fit_page();
					}
					if ui.selectable_label(false, "Fit width").clicked() {
						d.fit_width();
					}
					for z in [25.0, 50.0, 100.0, 200.0, 400.0, 800.0] {
						if ui.selectable_label(false, format!("{z}%")).clicked() {
							let c = d.canvas_rect.center();
							d.zoom_at(c, z / 100.0 / d.zoom);
						}
					}
				});
				if widgets::icon_button(ui, p, i::ZOOM_IN, "Zoom in (⌘=)", false).clicked() {
					let c = d.canvas_rect.center();
					d.zoom_at(c, 1.25);
				}
				if widgets::icon_button(ui, p, i::MAXIMIZE, "Fit page (⌘0)", false).clicked() {
					d.fit_page();
				}
				ui.separator();
				let now = self.settings.page_layout;
				for (l, icon, label, key) in LAYOUTS {
					if widgets::icon_button(ui, p, icon, &format!("{label} ({key})"), now.is(l)).clicked() {
						pick = Some(now.with(l.continuous, l.spreads));
					}
				}
				ui.separator();
				// Scale + cursor position (on the page under the pointer).
				let pg = if self.canvas.cursor_page.is_some() { self.canvas.cursor_on } else { d.page };
				let (w_pt, h_pt) = d.page_size(pg);
				let (scale, display) = {
					let doc = d.doc.lock();
					let at = self.canvas.cursor_page.unwrap_or(Point { x: 0.5, y: 0.5 });
					(doc.data.scale_at(pg, at).cloned(), doc.data.display.clone())
				};
				match &scale {
					Some(s) => {
						ui.label(RichText::new(format!("{} {}", i::SCALING, describe_scale(s, w_pt))).size(12.0).color(p.text2));
					}
					None => {
						if ui.add(egui::Button::new(RichText::new(format!("{} No scale — set one", i::CIRCLE_ALERT)).size(12.0).color(p.warning)).frame(false)).clicked() {
							self.right.tab = 0;
							self.settings.right_panel = true;
						}
					}
				}
				if let (Some(c), Some(s)) = (self.canvas.cursor_page, &scale) {
					if let Some(upp) = takeoff_core::geometry::scale_units_per_px(s, w_pt, h_pt) {
						let (x, y) = (c.x * w_pt * upp, c.y * h_pt * upp);
						ui.label(RichText::new(format!("x {}  y {}", units::format_length(x, &s.unit, &display), units::format_length(y, &s.unit, &display))).size(11.5).color(p.muted).monospace());
					}
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					match &d.save_error {
						Some(e) => {
							ui.label(RichText::new(format!("{} Not saved", i::CIRCLE_ALERT)).color(p.danger)).on_hover_text(e);
						}
						None if d.is_dirty() => {
							ui.label(RichText::new("Saving…").size(12.0).color(p.muted));
						}
						None => {
							ui.label(RichText::new(format!("{} Saved", i::CHECK)).size(12.0).color(p.muted)).on_hover_text(format!("Takeoff saved next to the PDF:\n{}", takeoff_core::sidecar::sidecar_path(&d.path).display()));
						}
					}
					ui.separator();
					let (marks, meas, ann) = {
						let doc = d.doc.lock();
						(doc.data.marks.iter().filter(|m| m.page == d.page).count(), doc.data.measurements.iter().filter(|m| m.page == d.page).count(), doc.data.annotations.iter().filter(|a| a.page == d.page).count())
					};
					ui.label(RichText::new(format!("{marks} counts · {meas} measurements · {ann} markups on this page")).size(12.0).color(p.muted));
					if !d.selection.is_empty() {
						ui.separator();
						ui.label(RichText::new(format!("{} selected", d.selection.len())).size(12.0).color(p.selection));
					}
				});
			});
		});
		if let Some(l) = pick {
			self.set_page_layout(l);
		}
	}

	pub(super) fn start_screen(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		ui.vertical_centered(|ui| {
			ui.add_space((ui.available_height() * 0.16).max(30.0));
			let (r, _) = ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
			ui.painter().rect_filled(r, 10.0, p.accent_fill);
			ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, i::RULER, theme::icon_font(30.0), p.on_accent);
			ui.add_space(14.0);
			ui.label(RichText::new("Takeoff Desk").font(theme::bold(28.0)).color(p.text));
			ui.label(RichText::new("Quantity takeoff with an AI that can see your drawings.").size(14.0).color(p.text2));
			ui.add_space(22.0);
			ui.horizontal(|ui| {
				let w = 420.0;
				ui.add_space((ui.available_width() - w) / 2.0);
				ui.vertical(|ui| {
					ui.set_width(w);
					if ui.add_sized([w, 40.0], egui::Button::new(RichText::new(format!("{}   Open a drawing…", i::FOLDER_OPEN)).font(theme::semibold(14.0)).color(p.on_accent)).fill(p.accent_fill).corner_radius(6)).clicked() {
						self.open_dialog(&ctx);
					}
					ui.add_space(6.0);
					ui.label(RichText::new("or drop PDFs anywhere in this window").size(12.0).color(p.muted));
					ui.add_space(18.0);
					if !self.settings.recent.is_empty() {
						widgets::section_label(ui, p, "Recent");
						for r in self.settings.recent.clone().into_iter().take(8) {
							let exists = r.exists();
							let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
							let dir = r.parent().map(|d| d.display().to_string()).unwrap_or_default();
							let resp = egui::Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
								ui.set_width(w - 22.0);
								ui.horizontal(|ui| {
									ui.label(RichText::new(i::FILE_TEXT).font(theme::icon_font(16.0)).color(if exists { p.accent } else { p.muted }));
									ui.vertical(|ui| {
										ui.label(RichText::new(&name).font(theme::semibold(13.0)).color(if exists { p.text } else { p.muted }));
										ui.add(egui::Label::new(RichText::new(&dir).size(11.0).color(p.muted)).truncate());
									});
								});
							});
							let resp = resp.response.interact(egui::Sense::click());
							if resp.hovered() {
								ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
							}
							if resp.clicked() && exists {
								self.open(r, &ctx);
							}
							ui.add_space(4.0);
						}
					}
					ui.add_space(18.0);
					widgets::section_label(ui, p, "What you can do");
					for (icon, title, body) in [
						(i::HASH, "Count, measure, mark up", "Counts, lengths, areas with cutouts, volumes, angles, clouds, callouts — snapped to the drawing's own lines."),
						(i::SPARKLES, "Ask the drawing", "A local or cloud AI that sets scales, counts symbols, measures runs and reads title blocks — you review what it adds."),
						(i::FILES, "Work the PDF", "Split, merge, extract, rotate, insert and reorder pages; export a flattened, marked-up PDF and CSV quantities."),
					] {
						ui.horizontal(|ui| {
							ui.label(RichText::new(icon).font(theme::icon_font(16.0)).color(p.accent));
							ui.vertical(|ui| {
								ui.label(RichText::new(title).font(theme::semibold(13.0)));
								ui.label(RichText::new(body).size(12.0).color(p.text2));
							});
						});
						ui.add_space(6.0);
					}
				});
			});
		});
	}

	pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
		let cmd = |k: Key| KeyboardShortcut::new(Modifiers::COMMAND, k);
		let cmd_shift = |k: Key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::O))) {
			self.open_dialog(ctx);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::J))) {
			self.toggle_ai();
			ctx.memory_mut(|m| m.request_focus(egui::Id::new("ai-input")));
		}
		if self.docs.is_empty() {
			return;
		}
		let typing = ctx.egui_wants_keyboard_input();
		let active = self.active;
		let Some(d) = self.docs.get_mut(active) else { return };
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::S))) {
			d.save_now();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::W))) {
			self.close_active();
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::G))) {
			let next = if d.view_mode == ViewMode::Graph { ViewMode::Page } else { ViewMode::Graph };
			self.set_view_mode(next);
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::F))) {
			if d.view_mode == ViewMode::Graph {
				ctx.memory_mut(|m| m.request_focus(egui::Id::new("graph-search")));
				return;
			}
			self.settings.left_panel = true;
			self.left.focus_search();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Num0))) {
			d.fit_page();
		}
		// ⌘4–⌘7: single page, continuous, two pages, two pages continuous.
		if let Some(k) = [Key::Num4, Key::Num5, Key::Num6, Key::Num7].into_iter().position(|k| ctx.input_mut(|i| i.consume_shortcut(&cmd(k)))) {
			let l = LAYOUTS[k].0;
			self.set_page_layout(self.settings.page_layout.with(l.continuous, l.spreads));
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Equals)) || i.consume_shortcut(&cmd(Key::Plus))) {
			let c = d.canvas_rect.center();
			d.zoom_at(c, 1.25);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Minus))) {
			let c = d.canvas_rect.center();
			d.zoom_at(c, 0.8);
		}
		if typing {
			return;
		}
		// Back / Forward along the map trail (⌥← ⌥→, ⌘[ ⌘]).
		let (back, fwd) = ctx.input_mut(|i| {
			let alt = |k| egui::KeyboardShortcut::new(Modifiers::ALT, k);
			(i.consume_shortcut(&alt(Key::ArrowLeft)) || i.consume_shortcut(&cmd(Key::OpenBracket)), i.consume_shortcut(&alt(Key::ArrowRight)) || i.consume_shortcut(&cmd(Key::CloseBracket)))
		});
		if back {
			d.map_back();
		}
		if fwd {
			d.map_forward();
		}
		// The graph has no tools, selection or pages to step through.
		if d.view_mode == ViewMode::Graph {
			if ctx.input(|i| i.key_pressed(Key::Escape)) {
				d.graph.deselect();
			}
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::Z)) || i.consume_shortcut(&cmd(Key::Y))) {
			d.doc.lock().redo();
		} else if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Z))) {
			d.doc.lock().undo();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::A))) {
			self.canvas.select_all(d);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::C))) {
			self.canvas.copy(d);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::X))) {
			self.canvas.copy(d);
			self.canvas.delete_selection(d);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::V))) {
			self.canvas.paste(d, false);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::D))) {
			self.canvas.copy(d);
			self.canvas.paste(d, true);
		}
		let (esc, del, pgup, pgdn, arrows, help, shift) = ctx.input(|i| {
			(
				i.key_pressed(Key::Escape),
				i.key_pressed(Key::Delete) || (i.key_pressed(Key::Backspace) && self.canvas.points.is_empty()),
				i.key_pressed(Key::PageUp),
				i.key_pressed(Key::PageDown),
				[i.key_pressed(Key::ArrowLeft), i.key_pressed(Key::ArrowRight), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)],
				i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?")),
				i.modifiers.shift,
			)
		});
		if esc {
			self.canvas.escape(d);
		}
		if del && !d.selection.is_empty() {
			self.canvas.delete_selection(d);
		}
		if pgup && d.page > 1 {
			let p = d.page - 1;
			d.go_to_page(p);
		}
		if pgdn && d.page < d.page_count() {
			let p = d.page + 1;
			d.go_to_page(p);
		}
		let step = if shift { 10.0 } else { 1.0 };
		if !d.selection.is_empty() {
			let [l, r, u, dn] = arrows;
			let (dx, dy) = ((r as i32 - l as i32) as f32 * step, (dn as i32 - u as i32) as f32 * step);
			if dx != 0.0 || dy != 0.0 {
				self.canvas.nudge(d, dx, dy);
			}
		} else if arrows[0] && d.page > 1 {
			let p = d.page - 1;
			d.go_to_page(p);
		} else if arrows[1] && d.page < d.page_count() {
			let p = d.page + 1;
			d.go_to_page(p);
		}
		if help {
			self.dialog = Some(Dialog::Shortcuts);
		}
		// Single-letter tool keys.
		let mods = ctx.input(|i| i.modifiers);
		if !mods.command && !mods.ctrl && !mods.alt {
			let typed: Vec<String> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None }).collect());
			for t in typed {
				let c = t.chars().next().unwrap_or(' ').to_ascii_uppercase();
				if c == 'S' {
					self.settings.snap = !self.settings.snap;
					self.toast(if self.settings.snap { "Snap on" } else { "Snap off" }, false);
					continue;
				}
				if let Some(tool) = Tool::all().find(|t| t.shortcut() == Some(c)) {
					self.canvas.set_tool(tool);
				}
			}
		}
	}
}
