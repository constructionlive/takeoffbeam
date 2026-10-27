//! Choosing a model: the header's model menu and the Models window with its
//! local, download, cloud, subscription and custom-endpoint tabs.

use super::*;

impl AiPanel {
	/// The model picker's dropdown: every model you can switch to right now,
	/// grouped by where it runs, plus the Models window for setting more up.
	pub(super) fn model_menu(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		ui.set_min_width(360.0);
		ui.spacing_mut().item_spacing.y = 2.0;
		if self.channels.values().any(|i| {
			let i = i.lock();
			i.status.is_none() && !i.loading
		}) {
			self.refresh_channels(ui.ctx());
		}
		let mut chosen: Option<String> = None;
		let mut setup: Option<usize> = None;
		let vision: Vec<_> = self.installed.iter().filter(|m| m.vision).cloned().collect();
		let cache = Self::cache(settings);

		group_header(ui, p, i::HARD_DRIVE, "On this computer", "private · free");
		if vision.is_empty() {
			if action_row(ui, p, i::DOWNLOAD, "Get a local model", "Runs offline; your drawings never leave this machine").clicked() {
				setup = Some(1);
			}
		}
		for m in vision {
			let id = format!("local:{}", m.id);
			let fit = self.fits.entry(m.id.clone()).or_insert_with(|| takeoff_models::fit_for(&m, settings.context_tokens, cache)).clone();
			let sub = format!("{:.1} GB · {}", m.size_bytes as f64 / (1u64 << 30) as f64, match fit.verdict {
				Verdict::Fits => "fits in memory",
				Verdict::Tight => "tight on memory",
				Verdict::TooBig => "not enough memory",
			});
			if model_row(ui, p, (Color32::from_rgb(0x10, 0xB9, 0x81), i::CPU), &m.label, &sub, settings.model == id, fit.verdict != Verdict::TooBig).clicked() {
				chosen = Some(id);
			}
		}

		ui.add_space(4.0);
		// Access granted on construction.live shows up as soon as the picker opens.
		self.refresh_cloud_if_older(settings, CLOUD_REFRESH_ON_OPEN);
		let refresh = self.cloud.is_some() && self.cloud_refreshing();
		if cloud_header(ui, p, refresh, self.cloud.is_some()) {
			self.refresh_cloud(Self::cloud_cfg(settings));
		}
		if self.cloud.is_some() && !self.cloud_models.is_empty() {
			for m in self.cloud_models.clone() {
				let id = format!("cloud:{}", m.id);
				let sub = format!("{}K context · {}", m.context_tokens / 1000, if m.vision { "sees drawings" } else { "text only: can't see drawings" });
				if model_row(ui, p, (p.accent_fill, i::CLOUD), &m.name, &sub, settings.model == id, true).clicked() {
					chosen = Some(id);
				}
			}
		} else if self.cloud.is_some() && (refresh || self.cloud_fetched.is_none()) {
			ui.horizontal(|ui| {
				ui.add_space(10.0);
				ui.spinner();
				widgets::hint(ui, p, "Loading your models…");
			});
		} else if self.cloud.is_some() {
			let sub = self.cloud_error.clone().map(|e| short(&e, 70)).unwrap_or_else(|| "Ask your admin to give you access, then refresh".into());
			if action_row(ui, p, i::REFRESH_CW, "No models on this account yet", &sub).clicked() {
				self.refresh_cloud(Self::cloud_cfg(settings));
			}
		} else if action_row(ui, p, i::LOG_IN, "Sign in to construction.live", "Gemini, Claude, GPT and more on your account").clicked() {
			setup = Some(2);
		}

		for ch in Channel::ALL {
			ui.add_space(4.0);
			let (color, glyph, plan) = match ch {
				Channel::ClaudeCode => (Color32::from_rgb(0xD9, 0x77, 0x57), i::SPARKLES, "your Claude plan"),
				Channel::Codex => (Color32::from_rgb(0x10, 0xA3, 0x7F), i::BOT, "your ChatGPT plan"),
				Channel::OpenCode => (Color32::from_rgb(0xD9, 0x9A, 0x06), i::KEYBOARD, "free models"),
			};
			group_header(ui, p, i::KEY_ROUND, ch.name(), plan);
			let info = self.channels[&ch].clone();
			let inf = info.lock();
			match (&inf.status, &inf.models) {
				(None, _) | (Some(_), None) if inf.loading => {
					ui.horizontal(|ui| {
						ui.add_space(10.0);
						ui.spinner();
						widgets::hint(ui, p, "Checking…");
					});
				}
				(Some(st), Some(Ok(models))) if st.exe.is_some() && st.signed_in != Some(false) && !st.outdated => {
					let account = st.account.clone().unwrap_or_default();
					for m in models {
						let id = format!("{}:{}", ch.key(), m.id);
						let sub = if m.note.is_empty() { account.clone() } else if account.is_empty() { m.note.clone() } else { format!("{} · {account}", m.note) };
						if model_row(ui, p, (color, glyph), &m.label, &sub, settings.model == id, true).clicked() {
							chosen = Some(id);
						}
					}
				}
				(Some(st), _) => {
					let (title, sub) = if st.exe.is_none() {
						(format!("Install {}", ch.name()), format!("Then use {plan} here"))
					} else if st.outdated {
						(format!("Update {}", ch.name()), format!("{} is too old (needs {} or newer)", st.version.clone().unwrap_or_default(), ch.min_version()))
					} else if st.signed_in == Some(false) {
						(format!("Sign in to {}", ch.name()), "Takes a minute — we'll show you how".to_string())
					} else {
						("Couldn't list models".to_string(), "See details".to_string())
					};
					if action_row(ui, p, glyph, &title, &sub).clicked() {
						setup = Some(3);
					}
				}
				_ => {}
			}
		}
		ui.add_space(6.0);
		ui.separator();
		if action_row(ui, p, i::SETTINGS, "Manage models…", "Downloads, memory check, sign-in, custom endpoints").clicked() {
			setup = Some(self.models_tab);
		}
		if let Some(id) = chosen {
			settings.model = id;
			settings.save();
			ui.close();
		}
		if let Some(tab) = setup {
			self.models_open = true;
			self.models_tab = tab;
			ui.close();
		}
	}

	pub fn models_window(&mut self, ctx: &egui::Context, settings: &mut Settings, p: &Palette) {
		if !self.models_open {
			return;
		}
		let mut open = true;
		egui::Window::new(RichText::new("Models").font(theme::semibold(15.0)))
			.open(&mut open)
			.collapsible(false)
			.resizable(true)
			.default_size([640.0, 520.0])
			.pivot(egui::Align2::CENTER_CENTER)
			.default_pos(ctx.content_rect().center())
			.show(ctx, |ui| {
				widgets::tabs(ui, p, &[(i::HARD_DRIVE, "On this computer"), (i::CLOUD_DOWNLOAD, "Download"), (i::CLOUD, "construction.live"), (i::KEY_ROUND, "Subscriptions"), (i::SERVER, "Custom endpoint")], &mut self.models_tab);
				ui.separator();
				ui.horizontal(|ui| {
					ui.label(RichText::new("Context").color(p.text2));
					let mut k = settings.context_tokens / 1024;
					egui::ComboBox::from_id_salt("ctx").selected_text(format!("{k}K tokens")).show_ui(ui, |ui| {
						for v in [8u32, 16, 32, 64, 128] {
							if ui.selectable_value(&mut k, v, format!("{v}K tokens")).clicked() {
								settings.context_tokens = v * 1024;
								self.fits.clear();
								settings.save();
							}
						}
					});
					if ui.checkbox(&mut settings.local_thinking, "Let local models think (slower)").on_hover_text("Better answers on hard sheets, but each step can take minutes on a laptop").changed() {
						takeoff_agent::set_local_thinking(settings.local_thinking);
						settings.save();
					}
					if ui.checkbox(&mut settings.kv_q8, "Compact context (Q8 cache)").on_hover_text("Halves the memory the conversation needs, with negligible quality loss").changed() {
						self.fits.clear();
						settings.save();
					}
				});
				self.workspace_row(ui, settings, p);
				ui.add_space(6.0);
				ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.models_tab {
					0 => self.installed_tab(ui, settings, p),
					1 => self.download_tab(ui, settings, p),
					2 => self.cloud_tab(ui, settings, p),
					3 => self.subscriptions_tab(ui, settings, p),
					_ => self.custom_tab(ui, settings, p),
				});
			});
		if !open {
			self.models_open = false;
		}
	}

	/// The AI's shell + files folder: on/off, which folder, open it.
	pub(super) fn workspace_row(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		ui.horizontal_wrapped(|ui| {
			if ui
				.checkbox(&mut settings.ai_workspace_on, "Let the AI run commands in a workspace folder")
				.on_hover_text("The AI gets a shell (bash / PowerShell) and file tools that start in this folder, for scripts, data and overlay files. Commands run as you, on this computer.")
				.changed()
			{
				settings.save();
			}
			if let Some(dir) = settings.workspace() {
				ui.label(RichText::new(dir.display().to_string()).monospace().size(11.5).color(p.text2));
				if ui.small_button("Change…").clicked() {
					if let Some(d) = rfd::FileDialog::new().set_directory(&dir).pick_folder() {
						settings.ai_workspace = d.display().to_string();
						settings.save();
					}
				}
				if ui.small_button("Open").clicked() {
					let _ = std::fs::create_dir_all(&dir);
					let _ = open::that(&dir);
				}
				if !settings.ai_workspace.is_empty() && ui.small_button("Default").on_hover_text(crate::settings::default_workspace().display().to_string()).clicked() {
					settings.ai_workspace.clear();
					settings.save();
				}
			}
		});
	}

	pub(super) fn verdict_chip(ui: &mut egui::Ui, p: &Palette, fit: &FitReport) {
		let (text, color) = match fit.verdict {
			Verdict::Fits => ("Fits", p.success),
			Verdict::Tight => ("Tight", p.warning),
			Verdict::TooBig => ("Not enough memory", p.danger),
		};
		ui.label(RichText::new(format!("● {text}")).color(color).font(theme::semibold(12.0))).on_hover_text(fit.summary());
	}

	pub(super) fn installed_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		if !self.installed.iter().any(|m| m.vision) {
			widgets::empty_state(ui, p, i::HARD_DRIVE, "No local vision models yet", "Download one in the next tab, or import a .gguf (with its mmproj vision file) you already have.");
		}
		let ctx = settings.context_tokens;
		let cache = Self::cache(settings);
		// Text-only models can't read drawings, so they aren't offered here.
		for m in self.installed.clone().into_iter().filter(|m| m.vision) {
			let fit = self.fits.entry(m.id.clone()).or_insert_with(|| takeoff_models::fit_for(&m, ctx, cache)).clone();
			widgets::card(ui, p, |ui| {
				ui.set_width(ui.available_width());
				ui.label(RichText::new(&m.label).font(theme::semibold(13.0)));
				let gb = m.size_bytes as f64 / (1u64 << 30) as f64;
				ui.label(RichText::new(format!("{gb:.1} GB · reads drawings")).size(11.5).color(p.text2));
				ui.horizontal(|ui| {
					widgets::hint(ui, p, &short_fit(&fit)).on_hover_text(fit.summary());
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						let active = settings.model == format!("local:{}", m.id);
						if active {
							ui.label(RichText::new(format!("{} In use", i::CIRCLE_CHECK)).color(p.success));
						} else if m.vision && fit.verdict != Verdict::TooBig {
							if widgets::primary_button(ui, p, "Use").clicked() {
								settings.model = format!("local:{}", m.id);
								settings.save();
								if let Err(e) = self.start_server(&m, settings) {
									self.live.error = Some(e);
								}
							}
						}
						Self::verdict_chip(ui, p, &fit);
					});
				});
				if fit.verdict == Verdict::TooBig {
					if let Some(c) = fit.max_ctx {
						widgets::hint(ui, p, &format!("Tip: it fits with a {}K context — change Context above.", c / 1024));
					}
				}
			});
			ui.add_space(4.0);
		}
		ui.horizontal(|ui| {
			if ui.button(format!("{}  Import a .gguf…", i::UPLOAD)).clicked() {
				if let Some(model) = rfd::FileDialog::new().add_filter("GGUF model", &["gguf"]).set_title("Choose the model file").pick_file() {
					let mmproj = rfd::FileDialog::new().add_filter("Vision projector (mmproj)", &["gguf"]).set_title("Optional: choose its vision projector (mmproj)").pick_file();
					match store::import(&model, mmproj.as_deref()) {
						Ok(m) if !m.vision => {
							let _ = store::remove(&m.id);
							self.live.error = Some("That model can't see images, so it can't read drawings. Import it together with its vision projector (the mmproj .gguf).".into());
						}
						Ok(_) => {
							self.installed = store::installed();
							self.fits.clear();
						}
						Err(e) => self.live.error = Some(e),
					}
				}
			}
			if ui.button(format!("{}  Refresh", i::REFRESH_CW)).clicked() {
				self.installed = store::installed();
				self.fits.clear();
			}
			if self.server.is_some() && ui.button(format!("{}  Unload model", i::SQUARE)).clicked() {
				if let Some(mut s) = self.server.take() {
					s.stop();
				}
				self.server_state = ServerState::Stopped;
			}
		});
		if let ServerState::Failed(e) = &self.server_state {
			ui.add_space(6.0);
			ui.label(RichText::new("The model failed to start:").color(p.danger));
			ui.label(RichText::new(e).monospace().size(11.0));
		}
	}

	pub(super) fn download_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		widgets::hint(ui, p, "Vision models that can read drawings. They run with llama.cpp on your machine — nothing leaves it.");
		ui.add_space(4.0);
		for m in catalog::builtin() {
			let installed = self.installed.iter().any(|x| x.id == m.id);
			let fit = takeoff_models::fit_for_catalog(&m, settings.context_tokens);
			widgets::card(ui, p, |ui| {
				ui.set_width(ui.available_width());
				ui.label(RichText::new(&m.label).font(theme::semibold(13.0)));
				ui.label(RichText::new(format!("{:.1} GB download · {}", m.size_bytes as f64 / 1e9, m.notes)).size(11.5).color(p.text2));
				ui.horizontal(|ui| {
					widgets::hint(ui, p, &short_fit(&fit)).on_hover_text(fit.summary());
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						if installed {
							ui.label(RichText::new(format!("{} Installed", i::CIRCLE_CHECK)).color(p.success));
						} else if let Some(d) = self.downloads.iter().find(|d| d.model_id == m.id) {
							let pr = d.progress();
							match &pr.state {
								DownloadState::Running => {
									if ui.small_button("Cancel").clicked() {
										d.cancel();
									}
								}
								DownloadState::Failed(e) => {
									ui.label(RichText::new("Failed").color(p.danger)).on_hover_text(e);
								}
								_ => {}
							}
						} else if widgets::primary_button(ui, p, &format!("{}  Download", i::DOWNLOAD)).clicked() {
							self.downloads.push(takeoff_models::download::start(&m));
						}
						Self::verdict_chip(ui, p, &fit);
					});
				});
				if let Some(d) = self.downloads.iter().find(|d| d.model_id == m.id) {
					let pr = d.progress();
					if pr.total > 0 {
						let frac = pr.done as f32 / pr.total as f32;
						let eta = if pr.bytes_per_sec > 0.0 { format!(" · {:.0} MB/s · {} left", pr.bytes_per_sec / 1e6, eta((pr.total - pr.done) as f64 / pr.bytes_per_sec)) } else { String::new() };
						ui.add(egui::ProgressBar::new(frac).text(format!("{:.1} / {:.1} GB{eta}", pr.done as f64 / 1e9, pr.total as f64 / 1e9)).fill(p.accent_fill));
					} else if matches!(pr.state, DownloadState::Running) {
						ui.add(egui::ProgressBar::new(0.0).animate(true).text("Starting…"));
					}
				}
			});
			ui.add_space(4.0);
		}
		let _ = settings;
	}

	pub(super) fn cloud_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		match &self.cloud {
			None => {
				widgets::card(ui, p, |ui| {
					ui.label(RichText::new("Use cloud models with your construction.live account").font(theme::semibold(13.5)));
					widgets::hint(ui, p, "Gemini, Claude, GPT and more, billed to your construction.live credits. Your drawing images are sent to the model provider.");
					ui.add_space(6.0);
					match &self.cloud_flow {
						CloudFlow::Idle | CloudFlow::Failed(_) => {
							if let CloudFlow::Failed(e) = &self.cloud_flow {
								ui.label(RichText::new(e).color(p.danger));
							} else if let Some(e) = &self.cloud_error {
								ui.label(RichText::new(e).color(p.danger));
							}
							if widgets::primary_button(ui, p, &format!("{}  Sign in with construction.live", i::LOG_IN)).clicked() {
								let name = format!("Takeoff Desk on {}", hostname());
								match takeoff_cloud::start_device_link(&Self::cloud_cfg(settings), &name) {
									Ok(code) => {
										let _ = open::that(&code.verify_url);
										self.cloud_flow = CloudFlow::Waiting { code, started: Instant::now(), last_poll: Instant::now() };
									}
									Err(e) => self.cloud_flow = CloudFlow::Failed(e),
								}
							}
						}
						CloudFlow::Waiting { code, .. } => {
							let code = code.clone();
							ui.label("Approve this device in your browser. Your code:");
							ui.label(RichText::new(&code.user_code).font(theme::bold(28.0)).color(p.accent));
							ui.horizontal(|ui| {
								ui.spinner();
								widgets::hint(ui, p, "Waiting for approval…");
								if ui.link("Open the page again").clicked() {
									let _ = open::that(&code.verify_url);
								}
								if ui.small_button("Cancel").clicked() {
									self.cloud_flow = CloudFlow::Idle;
								}
							});
						}
					}
				});
			}
			Some((_, account)) => {
				let account = account.clone();
				widgets::card(ui, p, |ui| {
					ui.horizontal(|ui| {
						ui.label(RichText::new(i::USER).font(theme::icon_font(18.0)).color(p.accent));
						ui.vertical(|ui| {
							ui.label(RichText::new(account.name.clone().unwrap_or(account.email.clone())).font(theme::semibold(13.5)));
							ui.label(RichText::new(format!("{}{}", account.email, account.team_name.as_ref().map(|t| format!(" · {t}")).unwrap_or_default())).size(11.5).color(p.text2));
						});
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
							if ui.button(format!("{}  Sign out", i::LOG_OUT)).clicked() {
								takeoff_cloud::token::clear();
								self.cloud = None;
								self.cloud_models.clear();
								self.cloud_me = None;
								self.cloud_fetch = None;
								self.cloud_fetched = None;
								if settings.model.starts_with("cloud:") {
									settings.model.clear();
								}
							}
						});
					});
					if let Some(me) = &self.cloud_me {
						if let Some(c) = me.balance_cents {
							widgets::hint(ui, p, &format!("Credits: ${:.2}", c as f64 / 100.0));
						}
					}
				});
				ui.add_space(6.0);
				self.refresh_cloud_if_older(settings, CLOUD_REFRESH_ON_OPEN);
				ui.horizontal(|ui| {
					widgets::section_label(ui, p, "Models on your account");
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						if self.cloud_refreshing() {
							ui.spinner();
						} else if ui.button(format!("{}  Refresh", i::REFRESH_CW)).on_hover_text("Check construction.live for models you were just given access to").clicked() {
							self.refresh_cloud(Self::cloud_cfg(settings));
						}
					});
				});
				if let Some(e) = &self.cloud_error {
					ui.label(RichText::new(e).color(p.danger));
				}
				if self.cloud_models.is_empty() && !self.cloud_refreshing() {
					widgets::hint(ui, p, "No models are enabled for this account yet. Once an admin gives you access on construction.live, press Refresh.");
				}
				for m in self.cloud_models.clone() {
					ui.horizontal(|ui| {
						ui.label(RichText::new(&m.name).font(theme::medium(13.0)));
						ui.label(RichText::new(format!("{}K context · {}", m.context_tokens / 1000, if m.vision { "sees drawings" } else { "text only" })).size(11.5).color(p.text2));
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
							let id = format!("cloud:{}", m.id);
							if settings.model == id {
								ui.label(RichText::new(format!("{} In use", i::CIRCLE_CHECK)).color(p.success));
							} else if ui.button("Use").clicked() {
								settings.model = id;
								settings.save();
							}
						});
					});
				}
			}
		}
		ui.add_space(8.0);
		ui.collapsing("Advanced", |ui| {
			ui.horizontal(|ui| {
				ui.label("Service URL");
				if ui.add(egui::TextEdit::singleline(&mut settings.cloud_url).hint_text(takeoff_cloud::DEFAULT_SITE_URL).desired_width(300.0)).changed() {
					settings.save();
				}
			});
		});
	}

	/// Look up both CLIs again (install, sign-in, models) in the background.
	pub(super) fn refresh_channels(&self, ctx: &egui::Context) {
		for (&ch, info) in &self.channels {
			if info.lock().loading {
				continue;
			}
			info.lock().loading = true;
			let (info, ctx) = (info.clone(), ctx.clone());
			std::thread::spawn(move || {
				let st = channel::status(ch);
				let models = st.exe.as_ref().filter(|_| st.signed_in != Some(false) && !st.outdated).map(|exe| channel::models(ch, exe));
				let mut i = info.lock();
				i.status = Some(st);
				i.models = models;
				i.loading = false;
				ctx.request_repaint();
			});
		}
	}

	pub(super) fn subscriptions_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		if self.channels.values().any(|i| { let i = i.lock(); i.status.is_none() && !i.loading }) {
			self.refresh_channels(ui.ctx());
		}
		// While someone finishes an install or sign-in in Terminal, look again
		// every few seconds (for up to 10 minutes) so the card turns ready by itself.
		let waiting: Vec<Instant> = self.channels.values().filter_map(|i| i.lock().waiting).collect();
		if let Some(&since) = waiting.iter().max() {
			if since.elapsed() < Duration::from_secs(600) {
				if self.last_channel_poll.is_none_or(|t| t.elapsed() > Duration::from_secs(4)) && !self.channels.values().any(|i| i.lock().loading) {
					self.last_channel_poll = Some(Instant::now());
					self.refresh_channels(ui.ctx());
				}
				ui.ctx().request_repaint_after(Duration::from_secs(1));
			}
		}
		ui.horizontal(|ui| {
			widgets::hint(ui, p, "Use the Claude or ChatGPT plan you already pay for. Takeoff Desk runs Claude Code or Codex on this computer and hands it the drawing tools; usage counts against your plan's limits, not construction.live credits.");
		});
		ui.horizontal(|ui| {
			if ui.button(format!("{}  Check again", i::REFRESH_CW)).clicked() {
				self.refresh_channels(ui.ctx());
			}
		});
		ui.add_space(4.0);
		for ch in Channel::ALL {
			let info = self.channels[&ch].clone();
			widgets::card(ui, p, |ui| {
				let mut i = info.lock();
				ui.horizontal(|ui| {
					ui.label(RichText::new(ch.name()).font(theme::semibold(13.5)));
					ui.label(RichText::new(ch.plan()).size(11.5).color(p.text2));
				});
				let st = i.status.clone();
				match &st {
					None => {
						ui.horizontal(|ui| {
							ui.spinner();
							widgets::hint(ui, p, &format!("Looking for {}…", ch.name()));
						});
						return;
					}
					Some(st) if st.exe.is_none() => {
						ui.label(RichText::new(format!("{} Not installed", i::CIRCLE_ALERT)).color(p.text2));
						if ch.needs_node() && !channel::npm_available() {
							// Codex installs with npm, which comes with Node.js.
							widgets::hint(ui, p, "Codex installs with npm, which comes with Node.js. Install Node.js first (the LTS version), then come back here.");
							if ui.button(format!("{}  Get Node.js", i::EXTERNAL_LINK)).clicked() {
								let _ = open::that("https://nodejs.org/en/download");
								i.waiting = Some(Instant::now());
							}
						} else {
							ui.horizontal(|ui| {
								if ui.button(format!("{}  Install {}", i::DOWNLOAD, ch.name())).on_hover_text("Opens a Terminal window that runs the installer").clicked() {
									i.launch_error = channel::open_in_terminal(ch.install_command()).err();
									i.waiting = Some(Instant::now());
								}
								if i.waiting.is_some() {
									ui.spinner();
									widgets::hint(ui, p, "Finish in the Terminal window; this updates by itself.");
								}
							});
						}
						ui.horizontal(|ui| {
							widgets::hint(ui, p, "Or in a terminal:");
							let cmd = ch.install_command();
							ui.label(RichText::new(cmd).monospace().size(12.0));
							if ui.small_button(i::COPY).on_hover_text("Copy").clicked() {
								ui.ctx().copy_text(cmd.to_string());
							}
						});
						if let Some(e) = &i.launch_error {
							ui.label(RichText::new(format!("Couldn't open a terminal: {e}")).size(11.5).color(p.danger));
						}
						return;
					}
					Some(st) if st.outdated => {
						let version = st.version.clone().unwrap_or_default();
						ui.label(RichText::new(format!("{} Installed {version} · too old (Takeoff Desk needs {} or newer)", i::CIRCLE_ALERT, ch.min_version())).color(p.warning));
						ui.horizontal(|ui| {
							match &i.update {
								Some(None) => {
									ui.spinner();
									widgets::hint(ui, p, &format!("Updating {}…", ch.name()));
								}
								_ => {
									if ui.button(format!("{}  Update", i::REFRESH_CW)).clicked() {
										i.update = Some(None);
										let (info, ctx, exe) = (info.clone(), ui.ctx().clone(), st.exe.clone().expect("installed"));
										std::thread::spawn(move || {
											let r = channel::update(ch, &exe);
											let mut g = info.lock();
											g.update = Some(Some(r));
											// Look again: version, sign-in, models.
											g.status = None;
											ctx.request_repaint();
										});
									}
									widgets::hint(ui, p, "or in a terminal:");
									ui.label(RichText::new(ch.update_command()).monospace().size(12.0));
									if ui.small_button(i::COPY).on_hover_text("Copy").clicked() {
										ui.ctx().copy_text(ch.update_command().to_string());
									}
								}
							}
						});
						if let Some(Some(Err(e))) = &i.update {
							ui.label(RichText::new(format!("Update failed: {e}")).size(11.5).color(p.danger));
						}
						return;
					}
					Some(st) => {
						let version = st.version.clone().unwrap_or_default();
						match st.signed_in {
							Some(false) => {
								ui.label(RichText::new(format!("{} Installed {version} · not signed in", i::CIRCLE_ALERT)).color(p.warning));
								let exe = st.exe.clone().expect("installed");
								ui.horizontal(|ui| {
									if ui.button(format!("{}  Sign in", i::LOG_IN)).on_hover_text("Opens a Terminal window; it takes you to your browser to sign in").clicked() {
										i.launch_error = channel::open_in_terminal(&channel::login_command_for(ch, &exe)).err();
										i.waiting = Some(Instant::now());
									}
									if i.waiting.is_some() {
										ui.spinner();
										widgets::hint(ui, p, "Finish signing in; this updates by itself.");
									}
								});
								ui.horizontal(|ui| {
									widgets::hint(ui, p, "Or in a terminal:");
									ui.label(RichText::new(ch.login_command()).monospace().size(12.0));
									if ui.small_button(i::COPY).on_hover_text("Copy").clicked() {
										ui.ctx().copy_text(ch.login_command().to_string());
									}
								});
								if let Some(e) = &i.launch_error {
									ui.label(RichText::new(format!("Couldn't open a terminal: {e}")).size(11.5).color(p.danger));
								}
								return;
							}
							_ => {
								// Done: stop the automatic re-checks.
								i.waiting = None;
								let who = st.account.clone().map(|a| format!(" · {a}")).unwrap_or_default();
								ui.label(RichText::new(format!("{} Ready {version}{who}", i::CIRCLE_CHECK)).color(p.success));
							}
						}
						if let Some(e) = &st.error {
							ui.label(RichText::new(e).size(11.5).color(p.danger));
						}
					}
				}
				let exe = st.and_then(|s| s.exe).expect("installed");
				ui.add_space(4.0);
				let models = match &i.models {
					Some(Ok(m)) => m.clone(),
					Some(Err(e)) => {
						ui.label(RichText::new(format!("Couldn't list models: {e}")).size(11.5).color(p.danger));
						vec![]
					}
					None => {
						ui.horizontal(|ui| {
							ui.spinner();
							widgets::hint(ui, p, "Loading models…");
						});
						vec![]
					}
				};
				let mut rows: Vec<ChannelModel> = models;
				let current = channel_model(&settings.model).filter(|(c, _)| *c == ch).map(|(_, m)| m.to_string());
				if let Some(cur) = current.as_ref().filter(|c| !rows.iter().any(|r| &r.id == *c)) {
					rows.push(ChannelModel { id: cur.clone(), label: cur.clone(), note: "Typed in".into() });
				}
				for m in rows {
					ui.horizontal(|ui| {
						ui.label(RichText::new(&m.label).font(theme::medium(13.0)));
						ui.label(RichText::new(&m.note).size(11.5).color(p.text2));
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
							let key = format!("{}:{}", ch.key(), m.id);
							if settings.model == key {
								ui.label(RichText::new(format!("{} In use", i::CIRCLE_CHECK)).color(p.success));
							} else if ui.button("Use").clicked() {
								settings.model = key;
								settings.save();
							}
							match i.tests.get(&m.id) {
								Some(None) => {
									ui.spinner();
								}
								Some(Some(Ok(t))) => {
									ui.label(RichText::new(format!("{} {t}", i::CIRCLE_CHECK)).size(11.5).color(p.success));
								}
								Some(Some(Err(e))) => {
									ui.label(RichText::new(format!("{} Not available", i::CIRCLE_ALERT)).size(11.5).color(p.danger)).on_hover_text(e);
								}
								None => {
									if ui.small_button("Test").on_hover_text("Send a one-line message to check this model works on your plan").clicked() {
										i.tests.insert(m.id.clone(), None);
										let (info, ctx, exe, id) = (info.clone(), ui.ctx().clone(), exe.clone(), m.id.clone());
										std::thread::spawn(move || {
											let r = channel::test_model(ch, &exe, &id, &channel_workdir());
											info.lock().tests.insert(id, Some(r));
											ctx.request_repaint();
										});
									}
								}
							}
						});
					});
				}
				ui.horizontal(|ui| {
					ui.add(egui::TextEdit::singleline(&mut i.custom_model).hint_text("other model id").desired_width(220.0));
					let id = i.custom_model.trim().to_string();
					if ui.add_enabled(!id.is_empty(), egui::Button::new("Use")).clicked() {
						settings.model = format!("{}:{id}", ch.key());
						settings.save();
						i.custom_model.clear();
					}
				});
			});
			ui.add_space(6.0);
		}
		widgets::hint(ui, p, "The model works only through Takeoff Desk's drawing tools: no shell, files or web. detect, calibrate and trace sweeps call a vision model of their own, so they are available when you are signed in to construction.live (billed there) or have a local model loaded; without one, the agent marks from its own zoomed views.");
	}

	pub(super) fn custom_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		widgets::hint(ui, p, "Any OpenAI-compatible server: LM Studio, Ollama (/v1), vLLM, OpenRouter, a llama-server you run yourself…");
		egui::Grid::new("custom").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
			ui.label("Base URL");
			ui.add(egui::TextEdit::singleline(&mut settings.custom_base_url).hint_text("http://localhost:1234/v1").desired_width(340.0));
			ui.end_row();
			ui.label("API key");
			ui.add(egui::TextEdit::singleline(&mut settings.custom_api_key).password(true).hint_text("optional").desired_width(340.0));
			ui.end_row();
			ui.label("Model");
			ui.add(egui::TextEdit::singleline(&mut settings.custom_model).hint_text("model id").desired_width(340.0));
			ui.end_row();
		});
		ui.horizontal(|ui| {
			if ui.button(format!("{}  Test & list models", i::REFRESH_CW)).clicked() {
				self.probe = Some(takeoff_agent::probe_models(settings.custom_base_url.trim(), (!settings.custom_api_key.is_empty()).then_some(settings.custom_api_key.as_str())));
			}
			if widgets::primary_button(ui, p, "Use this endpoint").clicked() && !settings.custom_model.trim().is_empty() {
				settings.model = "custom".into();
				settings.save();
			}
		});
		match &self.probe {
			Some(Ok(models)) => {
				ui.label(RichText::new(format!("{} Connected · {} models", i::CIRCLE_CHECK, models.len())).color(p.success));
				ui.horizontal_wrapped(|ui| {
					for m in models.iter().take(40) {
						if ui.small_button(m).clicked() {
							settings.custom_model = m.clone();
						}
					}
				});
			}
			Some(Err(e)) => {
				ui.label(RichText::new(format!("{} {e}", i::CIRCLE_ALERT)).color(p.danger));
			}
			None => {}
		}
	}
}

/// "Needs 23.1 GB · this Mac allows 28.1 GB · 9.9 GB free now"
pub(super) fn short_fit(f: &FitReport) -> String {
	let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
	format!("Needs {:.1} GB · this computer allows {:.1} GB · {:.1} GB free now", gb(f.estimate.total()), gb(f.budget), gb(f.available_now))
}

/// The provider's badge for the current model id.
pub(super) fn provider_badge(model: &str) -> (Color32, &'static str) {
	if model.starts_with("local:") {
		(Color32::from_rgb(0x10, 0xB9, 0x81), i::CPU)
	} else if model.starts_with("cloud:") {
		(Color32::from_rgb(0xE8, 0x53, 0x1A), i::CLOUD)
	} else if model.starts_with("claude:") {
		(Color32::from_rgb(0xD9, 0x77, 0x57), i::SPARKLES)
	} else if model.starts_with("codex:") {
		(Color32::from_rgb(0x10, 0xA3, 0x7F), i::BOT)
	} else if model.starts_with("opencode:") {
		(Color32::from_rgb(0xD9, 0x9A, 0x06), i::KEYBOARD)
	} else if model == "custom" {
		(Color32::from_rgb(0xA7, 0x8B, 0xFA), i::SERVER)
	} else {
		(Color32::from_gray(110), i::SPARKLES)
	}
}

pub(super) fn group_header(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, note: &str) {
	ui.horizontal(|ui| {
		ui.add_space(6.0);
		ui.label(RichText::new(icon).font(theme::icon_font(11.0)).color(p.muted));
		ui.label(RichText::new(title.to_uppercase()).font(theme::semibold(10.5)).color(p.muted).extra_letter_spacing(0.6));
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			ui.add_space(6.0);
			ui.label(RichText::new(note).size(11.0).color(p.muted));
		});
	});
}

/// The construction.live group header, with a refresh button when signed in.
/// Returns true when refresh was clicked.
pub(super) fn cloud_header(ui: &mut egui::Ui, p: &Palette, refreshing: bool, signed_in: bool) -> bool {
	let mut clicked = false;
	ui.horizontal(|ui| {
		ui.add_space(6.0);
		ui.label(RichText::new(i::CLOUD).font(theme::icon_font(11.0)).color(p.muted));
		ui.label(RichText::new("CONSTRUCTION.LIVE").font(theme::semibold(10.5)).color(p.muted).extra_letter_spacing(0.6));
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			ui.add_space(6.0);
			if signed_in {
				if refreshing {
					ui.add(egui::Spinner::new().size(12.0));
				} else {
					let r = ui.add(egui::Button::new(RichText::new(i::REFRESH_CW).font(theme::icon_font(12.0)).color(p.muted)).frame(false));
					clicked = r.on_hover_text("Refresh: check for models you were just given access to").clicked();
				}
			}
			ui.label(RichText::new("uses your credits").size(11.0).color(p.muted));
		});
	});
	clicked
}

/// One choice: badge, title, one-line description, check when selected.
pub(super) fn model_row(ui: &mut egui::Ui, p: &Palette, badge: (Color32, &str), title: &str, sub: &str, selected: bool, enabled: bool) -> egui::Response {
	let w = ui.available_width();
	let (rect, r) = ui.allocate_exact_size(egui::vec2(w, 44.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
	let painter = ui.painter();
	if selected {
		painter.rect_filled(rect, 6.0, if p.dark { Color32::from_rgb(0x2A, 0x1E, 0x18) } else { Color32::from_rgb(0xFD, 0xEE, 0xE7) });
	} else if r.hovered() && enabled {
		painter.rect_filled(rect, 6.0, p.hover);
	}
	let alpha = if enabled { 1.0 } else { 0.45 };
	let fade = |c: Color32| c.gamma_multiply(alpha);
	let b = egui::Rect::from_min_size(rect.left_center() + egui::vec2(8.0, -13.0), egui::vec2(26.0, 26.0));
	painter.rect_filled(b, 6.0, fade(badge.0));
	painter.text(b.center(), egui::Align2::CENTER_CENTER, badge.1, theme::icon_font(13.0), fade(Color32::WHITE));
	let x = b.right() + 10.0;
	let maxw = rect.right() - 34.0 - x;
	let t = painter.layout(title.to_string(), theme::semibold(13.0), fade(p.text), maxw);
	painter.galley(egui::pos2(x, rect.top() + 5.0), t, fade(p.text));
	let s = painter.layout(sub.to_string(), egui::FontId::proportional(11.5), fade(p.text2), maxw);
	painter.galley(egui::pos2(x, rect.top() + 24.0), s, fade(p.text2));
	if selected {
		painter.text(rect.right_center() - egui::vec2(16.0, 0.0), egui::Align2::CENTER_CENTER, i::CHECK, theme::icon_font(15.0), p.accent);
	}
	if r.hovered() && enabled {
		ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
	}
	r
}

/// A row that leads somewhere (set up, sign in, manage).
pub(super) fn action_row(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, sub: &str) -> egui::Response {
	let w = ui.available_width();
	let (rect, r) = ui.allocate_exact_size(egui::vec2(w, 40.0), egui::Sense::click());
	let painter = ui.painter();
	if r.hovered() {
		painter.rect_filled(rect, 6.0, p.hover);
		ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
	}
	let b = egui::Rect::from_min_size(rect.left_center() + egui::vec2(8.0, -13.0), egui::vec2(26.0, 26.0));
	painter.rect_stroke(b, 6.0, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
	painter.text(b.center(), egui::Align2::CENTER_CENTER, icon, theme::icon_font(13.0), p.accent);
	let x = b.right() + 10.0;
	painter.text(egui::pos2(x, rect.top() + 5.0), egui::Align2::LEFT_TOP, title, theme::semibold(12.5), p.accent);
	painter.text(egui::pos2(x, rect.top() + 23.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(11.5), p.text2);
	painter.text(rect.right_center() - egui::vec2(16.0, 0.0), egui::Align2::CENTER_CENTER, i::CHEVRON_RIGHT, theme::icon_font(13.0), p.muted);
	r
}
