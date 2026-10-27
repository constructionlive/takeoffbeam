//! The chat itself: the panel's layout, the transcript, the composer and how
//! messages (markdown, images, tool steps) are drawn.

use super::*;

impl AiPanel {
	pub(super) fn history_menu(&mut self, ui: &mut egui::Ui, path: &PathBuf, p: &Palette) {
		ui.set_min_width(320.0);
		let past = past_chats(path);
		if past.is_empty() {
			widgets::hint(ui, p, "No earlier conversations on this drawing yet. Start a new one with + and this one is kept here.");
			return;
		}
		widgets::section_label(ui, p, "Earlier conversations on this drawing");
		ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
			for c in past.iter().take(50) {
				let label = format!("{}\n{} · {} message{}", short(&c.title, 60), ago(c.saved_ms), c.messages, if c.messages == 1 { "" } else { "s" });
				let enabled = self.turn.is_none();
				if ui.add_enabled(enabled, egui::Button::new(RichText::new(label).size(12.0)).frame(false).wrap()).clicked() {
					if let Some(chat) = std::fs::read(&c.file).ok().and_then(|b| serde_json::from_slice::<ChatSession>(&b).ok()) {
						let session = self.session(path);
						let current = std::mem::replace(&mut *session.lock(), chat);
						archive_chat(path, &current);
						let _ = std::fs::remove_file(&c.file);
						self.save_session(path);
						self.live = Live::default();
					}
					ui.close();
				}
				ui.separator();
			}
		});
	}

	pub fn ui(&mut self, ui: &mut egui::Ui, doc: Option<&mut OpenDoc>, canvas: &mut Canvas, settings: &mut Settings, p: &Palette) {
		let panel_hovered = ui.ui_contains_pointer();
		let cmd_v = self.saw_paste_shortcut(ui);
		// Model picker: a full-width pill (provider badge · model · status · chevron).
		let (label, dot, state) = self.model_label(settings);
		let (badge, glyph) = provider_badge(&settings.model);
		ui.horizontal(|ui| {
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				// The pill's height, so the icons centre on it.
				ui.set_min_height(40.0);
				if let Some(d) = &doc {
					let path = d.path.clone();
					if widgets::icon_button(ui, p, i::CROSSHAIR, if settings.follow_agent { "Following the AI: the canvas shows what it looks at. Click (or pan yourself) to stop." } else { "Follow the AI: show the page and area it is looking at" }, settings.follow_agent).clicked() {
						settings.follow_agent = !settings.follow_agent;
						settings.save();
					}
					let r = widgets::icon_button(ui, p, i::HISTORY, "Past conversations", false);
					egui::Popup::menu(&r).show(|ui| self.history_menu(ui, &path, p));
					if widgets::icon_button(ui, p, i::PLUS, "New conversation (this one stays in history)", false).clicked() && self.turn.is_none() {
						self.new_conversation(&path);
					}
				}
				// The icons go first (right to left); the pill takes what is left.
				let w = ui.available_width().max(120.0);
				let (rect, r) = ui.allocate_exact_size(egui::vec2(w, 40.0), egui::Sense::click());
				let a11y = format!("Model: {label}");
				r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &a11y));
				let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&r));
				let painter = ui.painter();
				painter.rect(rect, 6.0, if r.hovered() || open { p.hover } else { p.card }, egui::Stroke::new(1.0, if open { p.accent } else { p.border }), egui::StrokeKind::Inside);
				let b = egui::Rect::from_min_size(rect.left_center() + egui::vec2(8.0, -13.0), egui::vec2(26.0, 26.0));
				painter.rect_filled(b, 6.0, badge);
				painter.text(b.center(), egui::Align2::CENTER_CENTER, glyph, theme::icon_font(14.0), Color32::WHITE);
				let tx = b.right() + 10.0;
				let max_w = rect.right() - 28.0 - tx;
				let g = painter.layout(label.clone(), theme::semibold(13.0), p.text, max_w);
				painter.galley(egui::pos2(tx, rect.top() + 5.0), g, p.text);
				painter.circle_filled(egui::pos2(tx + 4.0, rect.bottom() - 11.0), 3.5, dot);
				painter.text(egui::pos2(tx + 12.0, rect.bottom() - 11.0), egui::Align2::LEFT_CENTER, if state.is_empty() { "pick one to start" } else { state }, egui::FontId::proportional(11.5), p.text2);
				painter.text(rect.right_center() - egui::vec2(14.0, 0.0), egui::Align2::CENTER_CENTER, if open { i::CHEVRON_UP } else { i::CHEVRON_DOWN }, theme::icon_font(14.0), p.text2);
				if r.hovered() {
					ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
				}
				egui::Popup::menu(&r).width(w.max(360.0)).show(|ui| self.model_menu(ui, settings, p));
			});
		});
		if let ServerState::Starting = self.server_state {
			if settings.model.starts_with("local:") {
				ui.horizontal(|ui| {
					ui.spinner();
					widgets::hint(ui, p, "Loading the model into memory… first load can take a minute.");
				});
			}
		}
		ui.separator();

		let Some(doc) = doc else {
			widgets::empty_state(ui, p, i::SPARKLES, "Open a drawing", "The AI works on the drawing you have open.");
			return;
		};
		let session = self.session(&doc.path);
		let transcript = session.lock().transcript();
		let avail = ui.available_height();
		let composer_h = self.composer_h;
		let mut jump: Option<ViewAttachment> = None;
		// Vertical only: text wraps to the panel; code and tables scroll sideways on their own.
		ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).max_height((avail - composer_h).max(80.0)).show(ui, |ui| {
			if transcript.is_empty() && self.turn.is_none() && self.live.text.is_empty() {
				self.welcome(ui, doc, settings, p);
			}
			let mut tool_run: Vec<&takeoff_agent::TranscriptEntry> = Vec::new();
			let flush = |ui: &mut egui::Ui, run: &mut Vec<&takeoff_agent::TranscriptEntry>, thumbs: &mut HashMap<String, TextureHandle>| {
				if run.is_empty() {
					return;
				}
				let n = run.len();
				let failed = run.iter().filter(|e| e.tool_ok == Some(false)).count();
				let id = ui.make_persistent_id(("steps", run[0].text.len(), run[0].tool_name.clone(), n));
				egui::CollapsingHeader::new(RichText::new(format!("{}  {n} step{}{}", i::WAND_SPARKLES, if n == 1 { "" } else { "s" }, if failed > 0 { format!(" · {failed} failed") } else { String::new() })).size(12.0).color(p.text2))
					.id_salt(id)
					.default_open(false)
					.show(ui, |ui| {
						for (k, e) in run.iter().enumerate() {
							ui.horizontal_wrapped(|ui| {
								let ok = e.tool_ok.unwrap_or(true);
								ui.label(RichText::new(if ok { i::CHECK } else { i::CIRCLE_ALERT }).font(theme::icon_font(12.0)).color(if ok { p.success } else { p.danger }));
								ui.label(RichText::new(tool_title(e.tool_name.as_deref().unwrap_or("tool"))).font(theme::semibold(12.0)));
								ui.label(RichText::new(short(&e.text, 160)).size(11.5).color(p.text2));
							});
							if let Some(png) = &e.image_png {
								let key = format!("t{}-{}-{k}", png.len(), e.text.len());
								let tex = thumbs.entry(key.clone()).or_insert_with(|| {
									let img = image::load_from_memory(png).map(|i| i.to_rgba8()).unwrap_or_else(|_| image::RgbaImage::new(1, 1));
									ui.ctx().load_texture(key, egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()), egui::TextureOptions::LINEAR)
								});
								let size = tex.size_vec2();
								let w = size.x.min(ui.available_width());
								ui.image((tex.id(), egui::vec2(w, size.y * w / size.x)));
							}
						}
					});
				run.clear();
			};
			let upto = if self.turn.is_some() { self.live.transcript_len.min(transcript.len()) } else { transcript.len() };
			for e in &transcript[..upto] {
				match e.role {
					Role::Tool => tool_run.push(e),
					Role::User => {
						flush(ui, &mut tool_run, &mut self.thumbs);
						user_bubble(ui, p, &e.text);
						image_strip(ui, p, &e.images, &mut self.thumbs, false);
						attachment_chips(ui, p, &e.attachments, &doc.path, &mut jump);
					}
					Role::Assistant => {
						flush(ui, &mut tool_run, &mut self.thumbs);
						if !e.text.trim().is_empty() {
							markdown(ui, p, &e.text);
						}
					}
				}
			}
			flush(ui, &mut tool_run, &mut self.thumbs);
			if let Some(q) = &self.queued {
				user_bubble(ui, p, q);
				ui.horizontal(|ui| {
					ui.spinner();
					widgets::hint(ui, p, "Waiting for the model to load…");
				});
			}
			if self.turn.is_some() {
				if !self.live.asked.is_empty() {
					user_bubble(ui, p, &self.live.asked);
					image_strip(ui, p, &self.live.asked_images, &mut self.thumbs, false);
					attachment_chips(ui, p, &self.live.asked_attachments, &doc.path, &mut jump);
				}
				// Finished steps fold away like they do once the turn is over; the
				// running one shows as a single line.
				let done: Vec<&Step> = self.live.steps.iter().filter(|s| s.ok.is_some()).collect();
				if !done.is_empty() {
					let n = done.len();
					let failed = done.iter().filter(|s| s.ok == Some(false)).count();
					egui::CollapsingHeader::new(RichText::new(format!("{}  {n} step{}{}", i::WAND_SPARKLES, if n == 1 { "" } else { "s" }, if failed > 0 { format!(" · {failed} failed") } else { String::new() })).size(12.0).color(p.text2))
						.id_salt("live-steps")
						.default_open(false)
						.show(ui, |ui| {
							for s in &done {
								ui.horizontal_wrapped(|ui| {
									let ok = s.ok == Some(true);
									ui.label(RichText::new(if ok { i::CHECK } else { i::CIRCLE_ALERT }).font(theme::icon_font(12.0)).color(if ok { p.success } else { p.danger }));
									ui.label(RichText::new(tool_title(&s.name)).font(theme::semibold(12.0)));
									ui.label(RichText::new(short(&s.summary, 140)).size(11.5).color(p.text2));
								});
								if let Some(t) = &s.thumb {
									let size = t.size_vec2();
									let w = size.x.min(ui.available_width());
									ui.image((t.id(), egui::vec2(w, size.y * w / size.x)));
								}
							}
						});
				}
				for s in self.live.steps.iter().filter(|s| s.ok.is_none()) {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.label(RichText::new(format!("{}…", tool_title(&s.name))).size(12.0).color(p.text2));
					});
				}
				if !self.live.text.trim().is_empty() {
					markdown(ui, p, self.live.text.trim());
				} else if self.live.steps.iter().all(|s| s.ok.is_some()) {
					ui.horizontal(|ui| {
						ui.spinner();
						let secs = self.live.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
						widgets::hint(ui, p, &format!("Thinking… {secs}s"));
					});
				}
			}
			if let Some(e) = &self.live.error {
				ui.add_space(6.0);
				egui::Frame::new().fill(Color32::from_rgba_unmultiplied(0xF8, 0x71, 0x71, 28)).corner_radius(4).inner_margin(8).show(ui, |ui| {
					ui.label(RichText::new(format!("{}  {e}", i::CIRCLE_ALERT)).color(p.danger).size(12.5));
				});
			}
		});

		// Composer.
		let composer_top = ui.cursor().top();
		ui.add_space(4.0);
		let sel = doc.selection.len();
		// Wrapped, so a long sheet name can't push the panel wider than it is.
		ui.horizontal_wrapped(|ui| {
			chip(ui, p, &format!("{} {}", i::FILE_TEXT, doc.page_label(doc.page)));
			if sel > 0 {
				chip(ui, p, &format!("{} {sel} selected", i::MOUSE_POINTER_2));
			}
			let others: Vec<String> = self.others.iter().filter(|o| o.path != doc.path).filter_map(|o| o.path.file_name()).map(|f| f.to_string_lossy().into_owned()).collect();
			if !others.is_empty() {
				chip(ui, p, &format!("{} +{} open drawing{}", i::FILES, others.len(), if others.len() == 1 { "" } else { "s" }))
					.on_hover_text(format!("The AI can also work in:\n{}", others.join("\n")));
			}
			// Attach what's on screen, so the AI works on exactly this area.
			if ui.small_button(format!("{} Attach view", i::SCAN)).on_hover_text("Attach the area you're looking at. Pan or zoom and attach again for more areas — even on other pages or drawings.").clicked() {
				let a = ViewAttachment { drawing: doc.path.clone(), page: doc.page, region: visible_region(doc).unwrap_or((0.0, 0.0, 1.0, 1.0)) };
				if !self.attached.contains(&a) {
					self.attached.push(a);
				}
			}
			if ui
				.small_button(format!("{} Add image", i::IMAGE))
				.on_hover_text(format!("Add photos or screenshots to your message. You can also drop image files on the window, or copy a picture and press {} in the message box.", if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" }))
				.clicked()
			{
				if let Some(files) = rfd::FileDialog::new().add_filter("Images", IMAGE_EXTS).set_title("Add images to your message").pick_files() {
					self.attach_image_files(&files);
				}
			}
		});
		// Cmd/Ctrl+V with a picture on the clipboard (text pastes go to the box as usual).
		let composer_focused = ui.memory(|m| m.has_focus(egui::Id::new("ai-input")));
		let text_just_pasted = self.last_text_paste.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
		if cmd_v && (composer_focused || panel_hovered) && !text_just_pasted {
			self.paste_image();
		}
		if !self.attached_images.is_empty() {
			if let Some(k) = image_strip(ui, p, &self.attached_images, &mut self.thumbs, true) {
				self.attached_images.remove(k);
			}
		}
		if let Some(e) = self.image_error.clone() {
			ui.horizontal_wrapped(|ui| {
				ui.label(RichText::new(format!("{}  {e}", i::CIRCLE_ALERT)).size(11.5).color(p.danger));
				if ui.small_button(i::X).on_hover_text("Dismiss").clicked() {
					self.image_error = None;
				}
			});
		}
		if !self.attached.is_empty() {
			let mut remove = None;
			ui.horizontal_wrapped(|ui| {
				for (k, a) in self.attached.iter().enumerate() {
					let r = chip(ui, p, &format!("{} {}", i::SCAN, attachment_label(a, &doc.path))).interact(egui::Sense::click());
					if r.on_hover_text("Show this area").clicked() {
						jump = Some(a.clone());
					}
					if ui.small_button(i::X).on_hover_text("Remove").clicked() {
						remove = Some(k);
					}
				}
			});
			if let Some(k) = remove {
				self.attached.remove(k);
			}
		}
		if let Some(a) = jump.take() {
			if a.drawing == doc.path {
				doc.zoom_to(a.page, a.region);
			}
		}
		let frame = egui::Frame::new().fill(p.card).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 8));
		frame.show(ui, |ui| {
			let te = egui::TextEdit::multiline(&mut self.input)
				.hint_text("Ask the drawing… e.g. “count the duplex outlets on this sheet”")
				.desired_rows(2)
				.desired_width(f32::INFINITY)
				.frame(egui::Frame::NONE)
				.id(egui::Id::new("ai-input"));
			// Grows with what you type, up to ~40% of the panel, then scrolls.
			let r = ScrollArea::vertical().id_salt("ai-input-scroll").max_height((avail * 0.4).max(80.0)).stick_to_bottom(true).show(ui, |ui| ui.add(te)).inner;
			let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
			ui.horizontal(|ui| {
				if self.usage.0 > 0 {
					widgets::hint(ui, p, &format!("{}k tokens", (self.usage.0 + self.usage.1) / 1000));
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if self.busy() {
						if widgets::secondary_button(ui, p, &format!("{}  Stop", i::SQUARE)).clicked() {
							self.stop();
						}
					} else {
						let can = !self.input.trim().is_empty() || !self.attached_images.is_empty();
						let b = ui.add_enabled(can, egui::Button::new(RichText::new(format!("{}  Send", i::SEND)).color(p.on_accent).font(theme::semibold(12.5))).fill(p.accent_fill).corner_radius(4));
						if (b.clicked() || enter) && can {
							let mut text = std::mem::take(&mut self.input).trim().to_string();
							if text.is_empty() {
								text = if self.attached_images.len() == 1 { "Have a look at the attached image." } else { "Have a look at the attached images." }.to_string();
							}
							self.auto_pick_model(settings);
							if settings.model.is_empty() {
								self.input = text;
								self.models_open = true;
							} else {
								self.send(text, doc, settings);
							}
						}
					}
				});
			});
			if enter {
				// Swallow the newline the multiline editor just added.
				self.input = self.input.trim_end_matches('\n').to_string();
			}
		});
		let h = ui.min_rect().bottom() - composer_top + 6.0;
		if (h - self.composer_h).abs() > 0.5 {
			// Lay the chat out again with the composer's new height.
			self.composer_h = h;
			ui.ctx().request_repaint();
		}
		let _ = canvas;
	}

	pub(super) fn welcome(&mut self, ui: &mut egui::Ui, doc: &OpenDoc, settings: &Settings, p: &Palette) {
		ui.add_space(10.0);
		ui.label(RichText::new(format!("{}  Drawing Bench", i::SPARKLES)).font(theme::semibold(15.0)).color(p.text));
		ui.label(RichText::new("The agent looks at this drawing, counts and measures on it, reads notes and title blocks, and checks its own work. Everything it adds is marked “to review” until you confirm it.").color(p.text2).size(12.5));
		ui.add_space(8.0);
		widgets::section_label(ui, p, "Try");
		let page = doc.page_label(doc.page);
		let prompts = [
			format!("Read the title block and set the scale for {page}"),
			"Count every receptacle on this sheet by type".to_string(),
			"Measure the total length of the walls in the selected area".to_string(),
			"Check my takeoff for anything I missed".to_string(),
			"Summarise the quantities so far as a table".to_string(),
		];
		for pr in prompts {
			if ui.add(egui::Button::new(RichText::new(&pr).size(12.5).color(p.text)).fill(p.card).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(4).wrap()).clicked() {
				self.input = pr;
			}
		}
		if settings.model.is_empty() {
			ui.add_space(8.0);
			widgets::card(ui, p, |ui| {
				ui.label(RichText::new("Pick a model to start").font(theme::semibold(13.0)));
				widgets::hint(ui, p, "Run one on this computer (private, free), sign in to construction.live for cloud models, or use your own Claude or ChatGPT subscription.");
				if widgets::primary_button(ui, p, "Choose a model…").clicked() {
					self.models_open = true;
				}
			});
		}
	}
}

pub(super) fn tool_title(name: &str) -> String {
	let mut s = name.replace('_', " ");
	if let Some(f) = s.get_mut(0..1) {
		f.make_ascii_uppercase();
	}
	s
}

/// "p1 · 6% of the sheet", or "Other.pdf p2 · whole page".
pub(super) fn attachment_label(a: &ViewAttachment, here: &std::path::Path) -> String {
	let area = a.region.2 * a.region.3;
	let what = if area >= 0.95 { "whole page".to_string() } else { format!("{:.0}% of the sheet", (area * 100.0).max(1.0)) };
	let file = if a.drawing == here { String::new() } else { format!("{} ", a.drawing.file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()) };
	format!("{file}p{} · {what}", a.page)
}

/// A sent message's attachments; clicking one goes back to that area.
pub(super) fn attachment_chips(ui: &mut egui::Ui, p: &Palette, atts: &[ViewAttachment], here: &std::path::Path, jump: &mut Option<ViewAttachment>) {
	if atts.is_empty() {
		return;
	}
	ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
		for a in atts.iter().rev() {
			let r = chip(ui, p, &format!("{} {}", i::SCAN, attachment_label(a, here))).interact(egui::Sense::click());
			let tip = if a.drawing == here { "Show this area".to_string() } else { format!("In {}", a.drawing.display()) };
			if r.on_hover_text(tip).clicked() {
				*jump = Some(a.clone());
			}
		}
	});
}

/// Thumbnails of a message's pictures; click one to open it. With
/// `removable`, each has an x and the index of the one clicked is returned.
pub(super) fn image_strip(ui: &mut egui::Ui, p: &Palette, images: &[UserImage], thumbs: &mut HashMap<String, TextureHandle>, removable: bool) -> Option<usize> {
	if images.is_empty() {
		return None;
	}
	let mut remove = None;
	let mut one = |ui: &mut egui::Ui, k: usize, im: &UserImage| {
		let key = format!("user-img:{}", im.path.display());
		let tex = thumbs.entry(key.clone()).or_insert_with(|| {
			let img = image::open(&im.path).map(|i| i.thumbnail(240, 240).to_rgba8()).unwrap_or_else(|_| image::RgbaImage::from_pixel(48, 48, image::Rgba([120, 120, 120, 255])));
			ui.ctx().load_texture(key, egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()), egui::TextureOptions::LINEAR)
		});
		let size = tex.size_vec2();
		let h = 64.0;
		let w = (size.x * h / size.y.max(1.0)).clamp(24.0, 180.0);
		let r = ui.add(egui::Image::new((tex.id(), egui::vec2(w, h))).corner_radius(4).sense(egui::Sense::click()));
		if r.on_hover_text(format!("{} (click to open)", im.name)).clicked() {
			let _ = open::that(&im.path);
		}
		if removable && ui.add(egui::Button::new(RichText::new(i::X).color(p.text2)).small()).on_hover_text("Remove").clicked() {
			remove = Some(k);
		}
	};
	if removable {
		ui.horizontal_wrapped(|ui| {
			for (k, im) in images.iter().enumerate() {
				one(ui, k, im);
			}
		});
	} else {
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
			for (k, im) in images.iter().enumerate().rev() {
				one(ui, k, im);
			}
		});
	}
	remove
}

pub(super) fn chip(ui: &mut egui::Ui, p: &Palette, text: &str) -> egui::Response {
	egui::Frame::new()
		.fill(p.hover)
		.corner_radius(10)
		.inner_margin(egui::Margin::symmetric(8, 2))
		.show(ui, |ui| {
			ui.add(egui::Label::new(RichText::new(text).size(11.5).color(p.text2)).truncate());
		})
		.response
}

pub(super) fn user_bubble(ui: &mut egui::Ui, p: &Palette, text: &str) {
	ui.add_space(6.0);
	ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
		let max = ui.available_width() * 0.85;
		egui::Frame::new().fill(if p.dark { Color32::from_rgb(0x2A, 0x1E, 0x1A) } else { Color32::from_rgb(0xFD, 0xEE, 0xE7) }).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
			ui.set_max_width(max);
			// Explicit wrap: labels in a right-to-left (horizontal) layout extend by
			// default, which overflows the panel's left edge and drags the rest of the
			// transcript with it.
			ui.add(egui::Label::new(RichText::new(text).size(13.0).color(p.text)).wrap().halign(egui::Align::LEFT));
		});
	});
	ui.add_space(4.0);
}

/// Light markdown: headings, bullets, numbered lists, **bold**, `code`, tables as monospace.
pub fn markdown(ui: &mut egui::Ui, p: &Palette, text: &str) {
	let mut code: Vec<&str> = Vec::new();
	let mut in_fence = false;
	let mut blank = false;
	let mut block_no = 0;
	let flush = |ui: &mut egui::Ui, code: &mut Vec<&str>, block_no: &mut usize| {
		if code.is_empty() {
			return;
		}
		// Code and tables keep their columns: unwrapped, scrolling sideways on their own.
		*block_no += 1;
		ScrollArea::horizontal().id_salt(("md-block", *block_no)).auto_shrink([false, true]).show(ui, |ui| {
			for l in code.drain(..) {
				ui.add(egui::Label::new(RichText::new(l).monospace().size(11.5).color(p.text)).extend());
			}
		});
	};
	for line in text.lines() {
		let t = line.trim_end();
		if t.trim_start().starts_with("```") {
			in_fence = !in_fence;
			if !in_fence {
				flush(ui, &mut code, &mut block_no);
			}
			continue;
		}
		if in_fence || t.trim_start().starts_with('|') {
			code.push(t);
			continue;
		}
		flush(ui, &mut code, &mut block_no);
		if t.trim().is_empty() {
			// Runs of blank lines take the space of one.
			if !blank {
				ui.add_space(4.0);
			}
			blank = true;
			continue;
		}
		blank = false;
		let trimmed = t.trim_start();
		let indent = (t.len() - trimmed.len()) as f32 * 4.0;
		let (prefix, rest, size, strong) = if let Some(r) = trimmed.strip_prefix("### ") {
			("", r, 13.5, true)
		} else if let Some(r) = trimmed.strip_prefix("## ") {
			("", r, 14.5, true)
		} else if let Some(r) = trimmed.strip_prefix("# ") {
			("", r, 15.5, true)
		} else if let Some(r) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
			("•  ", r, 13.0, false)
		} else {
			("", trimmed, 13.0, false)
		};
		// One wrapped text block per line, so every wrapped row is spaced the same.
		let mut job = egui::text::LayoutJob::default();
		let style = ui.style().clone();
		let mut add = |rt: RichText| rt.append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Center);
		if !prefix.is_empty() {
			add(RichText::new(prefix).size(size).color(p.accent));
		}
		// **bold**, `code` and *italic* spans.
		let mut bold = strong;
		for (k, part) in rest.split("**").enumerate() {
			if k > 0 {
				bold = !bold;
			}
			for (j, seg) in part.split('`').enumerate() {
				if seg.is_empty() {
					continue;
				}
				if j % 2 == 1 {
					add(RichText::new(seg).monospace().size(12.0).color(p.accent));
					continue;
				}
				for (m, it) in seg.split('*').enumerate() {
					if it.is_empty() {
						continue;
					}
					let mut rt = RichText::new(it).size(size).color(p.text);
					if bold {
						rt = rt.font(theme::semibold(size));
					}
					if m % 2 == 1 {
						rt = rt.italics();
					}
					add(rt);
				}
			}
		}
		let label = egui::Label::new(job).wrap();
		if indent > 0.0 {
			egui::Frame::new().inner_margin(egui::Margin { left: indent.min(120.0) as i8, ..Default::default() }).show(ui, |ui| ui.add(label));
		} else {
			ui.add(label);
		}
	}
	flush(ui, &mut code, &mut block_no);
}
