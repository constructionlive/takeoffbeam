//! Dialogs, toasts and banners: the modal dialogs, crash-report notice and update banner.

use super::*;

impl TakeoffApp {
	pub(super) fn dialogs(&mut self, ctx: &egui::Context, p: &Palette) {
		let Some(mut dialog) = self.dialog.take() else { return };
		let mut keep = true;
		let title = match &dialog {
			Dialog::Calibrate { .. } => "Calibrate scale",
			Dialog::Viewport { .. } => "New viewport",
			Dialog::Depth { .. } => "Depth for volume",
			Dialog::Split { .. } => "Split document",
			Dialog::Merge { .. } => "Merge PDFs",
			Dialog::Settings => "Preferences",
			Dialog::Shortcuts => "Keyboard shortcuts",
			Dialog::About => "About Takeoff Desk",
			Dialog::ConfirmPdfEdit { .. } => "Change the PDF file?",
			Dialog::DeleteMap => "Delete the document map?",
		};
		let modal = egui::Modal::new(egui::Id::new("dialog")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(18).corner_radius(8));
		let resp = modal.show(ctx, |ui| {
			ui.set_width(match dialog {
				Dialog::Shortcuts => 560.0,
				Dialog::Merge { .. } => 520.0,
				_ => 420.0,
			});
			ui.label(RichText::new(title).font(theme::semibold(16.0)));
			ui.add_space(10.0);
			match &mut dialog {
				Dialog::Calibrate { page, p1, p2, text, unit, error, all_pages } => {
					ui.label(RichText::new("How long is the line you drew, in real life?").color(p.text2));
					ui.add_space(4.0);
					let r = ui.add(egui::TextEdit::singleline(text).hint_text("e.g. 22'-6\"  or  12.5  or  3.2 m").desired_width(220.0));
					r.request_focus();
					ui.horizontal(|ui| {
						ui.label("Unit if not typed");
						for u in units_list() {
							ui.selectable_value(unit, u.to_string(), u);
						}
					});
					ui.checkbox(all_pages, "Use for all pages of the same size");
					if let Some(e) = error {
						ui.label(RichText::new(e.as_str()).color(p.danger));
					}
					ui.add_space(8.0);
					let enter = ui.input(|i| i.key_pressed(Key::Enter));
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Set scale").clicked() || enter {
							match parse_length(text, unit) {
								Some((v, u)) if v > 0.0 => {
									let (page, p1, p2, all) = (*page, *p1, *p2, *all_pages);
									if let Some(d) = self.docs.get_mut(self.active) {
										let sizes = d.page_sizes.clone();
										let here = d.page_size(page);
										d.doc.lock().edit("Calibrate", |t| {
											let pages: Vec<u32> = if all { sizes.iter().enumerate().filter(|(_, s)| **s == here).map(|(i, _)| i as u32 + 1).collect() } else { vec![page] };
											for pg in pages {
												t.page_scales.retain(|s| s.page != pg);
												t.page_scales.push(PageScale { page: pg, p1, p2, distance: v, unit: u.clone() });
											}
										});
									}
									self.canvas.set_tool(Tool::Select);
									keep = false;
								}
								_ => *error = Some("Type a length like 22'-6\", 12.5 or 3.2 m".into()),
							}
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Viewport { page, rect, name, preset } => {
					ui.label(RichText::new("Measurements inside this box use its own scale.").color(p.text2));
					ui.add(egui::TextEdit::singleline(name).desired_width(240.0));
					let presets: Vec<&ScalePreset> = ScalePreset::all().collect();
					egui::ComboBox::from_id_salt("vp-preset").selected_text(presets[*preset].label).width(240.0).show_ui(ui, |ui| {
						for (k, pr) in presets.iter().enumerate() {
							ui.selectable_value(preset, k, pr.label);
						}
					});
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Create viewport").clicked() {
							if let Some(d) = self.docs.get_mut(self.active) {
								let (w, _) = d.page_size(*page);
								let scale = presets[*preset].to_page_scale(*page, w);
								let vp = Viewport { id: new_id("vp"), page: *page, name: name.clone(), x: rect.0, y: rect.1, w: rect.2, h: rect.3, scale };
								d.doc.lock().edit("Viewport", |t| t.viewports.push(vp));
							}
							keep = false;
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Depth { id, value } => {
					ui.label(RichText::new("Depth or thickness, in the page's scale unit (e.g. 0.33 for a 4\" slab in feet).").color(p.text2));
					let r = ui.add(egui::TextEdit::singleline(value).desired_width(160.0));
					r.request_focus();
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Set depth").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
							let unit = self.docs.get(self.active).and_then(|d| d.doc.lock().data.scale_for(d.page).map(|s| s.unit.clone())).unwrap_or_else(|| "ft".into());
							if let Some((v, _)) = parse_length(value, &unit) {
								if let Some(d) = self.docs.get_mut(self.active) {
									let id = id.clone();
									d.doc.lock().edit("Depth", |t| {
										if let Some(m) = t.measurements.iter_mut().find(|m| m.id == id) {
											m.depth = Some(v);
										}
									});
								}
								keep = false;
							}
						}
						if widgets::secondary_button(ui, p, "Skip").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Split { mode, every, ranges, error } => {
					ui.horizontal(|ui| {
						ui.selectable_value(mode, 0, "Every N pages");
						ui.selectable_value(mode, 1, "Page ranges");
						ui.selectable_value(mode, 2, "By bookmarks");
					});
					ui.add_space(6.0);
					match *mode {
						0 => {
							ui.horizontal(|ui| {
								ui.label("Pages per file");
								ui.add(egui::DragValue::new(every).range(1..=10_000));
							});
						}
						1 => {
							ui.add(egui::TextEdit::singleline(ranges).hint_text("e.g. 1-12, 13-40, 41-").desired_width(260.0));
						}
						_ => {
							widgets::hint(ui, p, "One file per top-level bookmark, named after it.");
						}
					}
					if let Some(e) = error {
						ui.label(RichText::new(e.as_str()).color(p.danger));
					}
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Choose folder and split").clicked() {
							match self.run_split(*mode, *every, ranges) {
								Ok(n) => {
									self.toast(format!("Wrote {n} files"), false);
									keep = false;
								}
								Err(e) if e.is_empty() => {}
								Err(e) => *error = Some(e),
							}
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Merge { files, error } => {
					widgets::hint(ui, p, "Files are combined in this order. Takeoffs saved next to each file are merged too.");
					let mut remove = None;
					let mut swap = None;
					for (k, f) in files.iter().enumerate() {
						ui.horizontal(|ui| {
							ui.label(RichText::new(format!("{}.", k + 1)).color(p.muted));
							ui.add(egui::Label::new(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).truncate());
							ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
								if ui.small_button(i::X).clicked() {
									remove = Some(k);
								}
								if k + 1 < files.len() && ui.small_button(i::ARROW_DOWN).clicked() {
									swap = Some((k, k + 1));
								}
								if k > 0 && ui.small_button(i::ARROW_UP).clicked() {
									swap = Some((k, k - 1));
								}
							});
						});
					}
					if let Some(k) = remove {
						files.remove(k);
					}
					if let Some((a, b)) = swap {
						files.swap(a, b);
					}
					if ui.button(format!("{}  Add PDFs…", i::PLUS)).clicked() {
						if let Some(more) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_files() {
							files.extend(more);
						}
					}
					if let Some(e) = error {
						ui.label(RichText::new(e.as_str()).color(p.danger));
					}
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if ui.add_enabled(files.len() >= 2, egui::Button::new(RichText::new("Merge and open").color(p.on_accent).font(theme::semibold(13.0))).fill(p.accent_fill)).clicked() {
							match self.run_merge(files) {
								Ok(Some(path)) => {
									self.open(path, ctx);
									keep = false;
								}
								Ok(None) => {}
								Err(e) => *error = Some(e),
							}
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::ConfirmPdfEdit { action } => {
					ui.label(RichText::new("Page tools rewrite the PDF file itself. Your takeoff follows the pages (deleted pages drop their markups, rotated pages turn theirs).").color(p.text2));
					ui.label(RichText::new("A backup of the original is kept next to it the first time.").color(p.text2));
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Continue").clicked() {
							self.pdf_edits_confirmed = true;
							let a = action.clone();
							keep = false;
							self.dialog = None;
							self.page_action(a, ctx);
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::DeleteMap => {
					ui.label(RichText::new("The map of sheets, details and links is removed from this drawing. The AI stops using it, and mapping the set again starts from nothing.").color(p.text2));
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if ui.add(egui::Button::new(RichText::new("Delete map").color(Color32::WHITE)).fill(p.danger)).clicked() {
							if let Some(d) = self.docs.get_mut(self.active) {
								d.delete_map();
							}
							keep = false;
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Settings => {
					egui::Grid::new("prefs").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
						ui.label("Your name");
						if ui.text_edit_singleline(&mut self.settings.author).changed() {
							for d in &self.docs {
								d.doc.lock().author = self.settings.author.clone();
							}
						}
						ui.end_row();
						ui.label("Mouse wheel");
						ui.horizontal(|ui| {
							ui.selectable_value(&mut self.settings.wheel_zooms, true, "Zooms").on_hover_text("A trackpad always pans with two fingers; pinch to zoom");
							ui.selectable_value(&mut self.settings.wheel_zooms, false, "Scrolls (pinch / ⌘ to zoom)");
						});
						ui.end_row();
						ui.label("Snap to lines");
						ui.checkbox(&mut self.settings.snap, "On by default (S toggles, Alt bypasses)");
						ui.end_row();
						ui.label("llama-server");
						ui.add(egui::TextEdit::singleline(&mut self.settings.llama_server).hint_text("auto-detect").desired_width(240.0));
						ui.end_row();
					});
					ui.add_space(10.0);
					if widgets::primary_button(ui, p, "Done").clicked() {
						if !self.settings.llama_server.trim().is_empty() {
							std::env::set_var("TAKEOFF_LLAMA_SERVER", self.settings.llama_server.trim());
						}
						self.settings.save();
						keep = false;
					}
				}
				Dialog::Shortcuts => {
					egui::Grid::new("keys").num_columns(4).spacing([18.0, 6.0]).striped(true).show(ui, |ui| {
						let mut col = 0;
						for t in Tool::all() {
							if let Some(k) = t.shortcut() {
								ui.label(RichText::new(k.to_string()).monospace().font(theme::semibold(12.0)).color(p.accent));
								ui.label(t.label());
								col += 1;
								if col % 2 == 0 {
									ui.end_row();
								}
							}
						}
						ui.end_row();
						for (k, v) in [
							("Space", "Pan while held"),
							("S", "Toggle snapping"),
							("Shift", "Lock to 0/45/90°"),
							("Alt", "Bypass snapping"),
							("Enter", "Finish shape"),
							("Backspace", "Remove last point"),
							("Esc", "Cancel / Select tool"),
							("Del", "Delete selection"),
							("⌘Z / ⇧⌘Z", "Undo / Redo"),
							("⌘C ⌘V ⌘D", "Copy, paste, duplicate"),
							("⌘F", "Search text"),
							("⌘J", "Ask the AI"),
							("⌘G", "Page / Graph view"),
							("⌥← / ⌥→", "Back / forward along the map"),
							("PgUp / PgDn", "Previous / next page"),
							("⌘0", "Fit page"),
							("⌘4 / ⌘5", "Single page / continuous"),
							("⌘6 / ⌘7", "Two pages / two continuous"),
						] {
							ui.label(RichText::new(k).monospace().font(theme::semibold(12.0)).color(p.accent));
							ui.label(v);
							col += 1;
							if col % 2 == 0 {
								ui.end_row();
							}
						}
					});
					ui.add_space(10.0);
					if widgets::primary_button(ui, p, "Close").clicked() {
						keep = false;
					}
				}
				Dialog::About => {
					ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
					ui.label(RichText::new("Open-source takeoff with a native AI agent, by construction.live.").color(p.text2));
					ui.label(RichText::new("Apache License 2.0. Includes pdfium (BSD), Inter (OFL), Lucide icons (ISC).").size(12.0).color(p.muted));
					ui.hyperlink_to("construction.live", "https://construction.live");
					ui.add_space(10.0);
					if widgets::primary_button(ui, p, "Close").clicked() {
						keep = false;
					}
				}
			}
		});
		if resp.should_close() {
			keep = false;
		}
		if keep && self.dialog.is_none() {
			self.dialog = Some(dialog);
		}
	}

	pub(super) fn toasts(&mut self, ctx: &egui::Context, p: &Palette) {
		let Some((msg, at, error)) = &self.toast else { return };
		let age = at.elapsed().as_secs_f32();
		if age > 4.0 {
			self.toast = None;
			return;
		}
		ctx.request_repaint_after(std::time::Duration::from_millis(100));
		egui::Area::new(egui::Id::new("toast")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -48.0]).order(egui::Order::Tooltip).show(ctx, |ui| {
			egui::Frame::popup(&ctx.global_style()).fill(if *error { p.danger } else { p.panel }).corner_radius(6).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
				ui.label(RichText::new(msg).color(if *error { Color32::WHITE } else { p.text }).size(13.0));
			});
		});
	}

	/// Bottom-right, non-blocking: send last session's crash report by email, or let it go.
	pub(super) fn crash_notice(&mut self, ctx: &egui::Context, p: &Palette) {
		let Some((report, dir)) = self.crash.clone() else { return };
		let (mut send, mut show, mut dismiss) = (false, false, false);
		egui::Area::new(egui::Id::new("crash-notice")).anchor(egui::Align2::RIGHT_BOTTOM, [-14.0, -40.0]).order(egui::Order::Foreground).show(ctx, |ui| {
			egui::Frame::popup(&ctx.global_style()).fill(p.panel).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
				ui.set_max_width(320.0);
				ui.horizontal(|ui| {
					ui.label(RichText::new(i::CIRCLE_ALERT).font(theme::icon_font(15.0)).color(p.danger));
					ui.label(RichText::new("Takeoff Desk closed unexpectedly").font(theme::semibold(13.0)).color(p.text));
				});
				let first = report.lines().find(|l| l.starts_with("Panic: ")).unwrap_or("").trim_start_matches("Panic: ");
				ui.label(RichText::new(format!("A crash report was saved. Send it to the team so we can fix it? Nothing is sent unless you click Send.{}", if first.is_empty() { String::new() } else { format!("\n\n{}", first.chars().take(140).collect::<String>()) })).size(12.0).color(p.text2));
				ui.add_space(6.0);
				ui.horizontal(|ui| {
					send = ui.button(format!("{}  Send report…", i::SEND)).on_hover_text("Opens an email to the team with the report in it").clicked();
					show = ui.button("Show").on_hover_text("Open the logs folder").clicked();
					dismiss = ui.button("Dismiss").clicked();
				});
			});
		});
		if send {
			let url = updates::crash_mailto(&self.settings.model, &report, &dir.join("crash.txt"));
			if open::that(&url).is_err() {
				self.toast("No email app found — write to rahul@construction.live", true);
				return;
			}
		}
		if show {
			self.show_logs();
		}
		if send || dismiss {
			crate::crash::archive(&dir);
			self.crash = None;
		}
	}

	pub(super) fn update_banner(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let Some(rel) = self.updater.available(&self.settings.skipped_update) else { return };
		if self.updater.dismissed && !rel.critical {
			return;
		}
		let ctx = ui.ctx().clone();
		let state = self.updater.download_state();
		let fill = if rel.critical { Color32::from_rgb(0x7F, 0x1D, 0x1D) } else if p.dark { Color32::from_rgb(0x2A, 0x1C, 0x14) } else { Color32::from_rgb(0xFF, 0xF1, 0xEA) };
		egui::Panel::top("update-banner").frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			ui.horizontal(|ui| {
				ui.label(RichText::new(if rel.critical { i::CIRCLE_ALERT } else { i::SPARKLES }).font(theme::icon_font(15.0)).color(if rel.critical { Color32::WHITE } else { p.accent }));
				let title = if rel.critical { format!("Important update: Takeoff Desk {}", rel.version) } else { format!("Takeoff Desk {} is available", rel.version) };
				ui.label(RichText::new(title).font(theme::semibold(13.0)).color(if rel.critical { Color32::WHITE } else { p.text }));
				ui.label(RichText::new(format!("you have {}", env!("CARGO_PKG_VERSION"))).size(12.0).color(if rel.critical { Color32::from_gray(220) } else { p.text2 }));
				let first = rel.notes.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches(['-', '*', '#', ' ']).to_string();
				if !first.is_empty() {
					ui.add(egui::Label::new(RichText::new(format!("· {first}")).size(12.0).color(if rel.critical { Color32::from_gray(230) } else { p.text2 })).truncate());
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if !rel.critical && ui.small_button("Skip this version").clicked() {
						self.settings.skipped_update = rel.version.clone();
						self.settings.save();
					}
					if !rel.critical && ui.small_button("Later").clicked() {
						self.updater.dismissed = true;
					}
					if !rel.notes.trim().is_empty() && ui.small_button("What's new").clicked() {
						self.notes_open = true;
					}
					match &state {
						DownloadState::Downloading { done, total } => {
							let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
							ui.add(egui::ProgressBar::new(frac).desired_width(160.0).text(format!("{:.0} / {:.0} MB", *done as f64 / 1e6, *total as f64 / 1e6)).fill(p.accent_fill));
						}
						DownloadState::Done(path) => {
							if let Some(e) = &self.updater.install_error {
								if ui.add(egui::Button::new(RichText::new(format!("{}  Open installer", i::EXTERNAL_LINK)).color(p.on_accent)).fill(p.accent_fill)).on_hover_text(path.display().to_string()).clicked() {
									let _ = open::that(path);
								}
								ui.label(RichText::new("Couldn't update in place — quit Takeoff Desk, then install").size(12.0).color(p.text2)).on_hover_text(e);
							} else {
								if ui.add(egui::Button::new(RichText::new(format!("{}  Restart to update", i::DOWNLOAD)).color(p.on_accent).font(theme::semibold(12.5))).fill(p.accent_fill).corner_radius(4)).on_hover_text("Saves your work, installs the update and opens Takeoff Desk again").clicked() {
									match updates::install_on_exit(path) {
										Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
										Err(e) => self.updater.install_error = Some(e),
									}
								}
								ui.label(RichText::new("Downloaded").size(12.0).color(p.text2));
							}
						}
						DownloadState::Failed(e) => {
							if ui.button("Open download page").clicked() {
								let _ = open::that(if rel.page.is_empty() { updates::RELEASES_PAGE } else { &rel.page });
							}
							ui.label(RichText::new("Download failed").color(p.danger)).on_hover_text(e);
						}
						DownloadState::Idle => {
							if ui.add(egui::Button::new(RichText::new(format!("{}  Download & install", i::DOWNLOAD)).color(p.on_accent).font(theme::semibold(12.5))).fill(p.accent_fill).corner_radius(4)).clicked() {
								self.updater.download_and_open(&rel, &ctx);
							}
						}
					}
				});
			});
		});
		if self.notes_open {
			let mut open = true;
			egui::Window::new(format!("What's new in {}", rel.version)).open(&mut open).collapsible(false).resizable(true).default_width(460.0).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(&ctx, |ui| {
				if !rel.date.is_empty() {
					ui.label(RichText::new(&rel.date).color(p.muted).size(12.0));
				}
				egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| crate::ai_panel::markdown(ui, p, &rel.notes));
			});
			self.notes_open = open;
		}
	}
}
