//! The AI tab: pick a model (local llama.cpp, construction.live cloud, the
//! estimator's own Claude Code / Codex subscription, or any OpenAI-compatible
//! endpoint), chat with the drawing agent, watch its steps.
//! Also the Models window: installed models with a memory-fit verdict,
//! downloads, sign-in, subscriptions, custom endpoint.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, ScrollArea, TextureHandle};
use parking_lot::Mutex;
use takeoff_agent::channel::{self, Channel, ChannelModel, ChannelStatus, ToolHost};
use takeoff_agent::{AgentContext, AgentEvent, ChatSession, ProviderConfig, Role, TurnHandle};
use takeoff_models::catalog;
use takeoff_models::download::{DownloadHandle, DownloadState};
use takeoff_models::fit::{CacheType, FitReport, Verdict};
use takeoff_models::server::{LlamaServer, ServerState};
use takeoff_models::store::{self, InstalledModel};

use crate::canvas::Canvas;
use crate::docstate::OpenDoc;
use crate::icons as i;
use crate::settings::{config_dir, Settings};
use crate::theme::{self, Palette};
use crate::widgets;

struct Step {
	call_id: String,
	name: String,
	summary: String,
	ok: Option<bool>,
	thumb: Option<TextureHandle>,
}

#[derive(Default)]
struct Live {
	text: String,
	reasoning: String,
	steps: Vec<Step>,
	error: Option<String>,
	started: Option<Instant>,
	/// Transcript entries that existed when the turn began (the rest is drawn live).
	transcript_len: usize,
	/// The message being answered.
	asked: String,
}

/// What we know about one subscription CLI (filled in by background threads).
#[derive(Default)]
struct ChannelInfo {
	status: Option<ChannelStatus>,
	models: Option<Result<Vec<ChannelModel>, String>>,
	loading: bool,
	/// Model id → None while testing, then the result.
	tests: HashMap<String, Option<Result<String, String>>>,
	custom_model: String,
}

/// "claude:opus" → (Claude Code, "opus").
fn channel_model(model: &str) -> Option<(Channel, &str)> {
	let (k, m) = model.split_once(':')?;
	Some((Channel::from_key(k)?, m))
}

/// Where the CLIs run: an empty folder of ours, the same every time (Claude
/// Code files its sessions by folder).
fn channel_workdir() -> PathBuf {
	config_dir().join("bench")
}

enum CloudFlow {
	Idle,
	Waiting { code: takeoff_cloud::DeviceCode, started: Instant, last_poll: Instant },
	Failed(String),
}

pub struct AiPanel {
	sessions: HashMap<PathBuf, Arc<Mutex<ChatSession>>>,
	turn: Option<TurnHandle>,
	turn_doc: Option<PathBuf>,
	live: Live,
	input: String,
	/// Message waiting for the local model to finish loading.
	queued: Option<String>,
	pub server: Option<LlamaServer>,
	server_state: ServerState,
	server_polled: Instant,
	installed: Vec<InstalledModel>,
	fits: HashMap<String, FitReport>,
	downloads: Vec<DownloadHandle>,
	cloud: Option<(String, takeoff_cloud::Account)>,
	cloud_models: Vec<takeoff_cloud::CloudModel>,
	cloud_me: Option<takeoff_cloud::Me>,
	cloud_flow: CloudFlow,
	cloud_error: Option<String>,
	pub models_open: bool,
	models_tab: usize,
	probe: Option<Result<Vec<String>, String>>,
	thumbs: HashMap<String, TextureHandle>,
	usage: (u64, u64),
	/// Drawing tools served to Claude Code / Codex, one per open document.
	hosts: HashMap<PathBuf, ToolHost>,
	channels: HashMap<Channel, Arc<Mutex<ChannelInfo>>>,
}

impl AiPanel {
	pub fn new() -> Self {
		let cloud = takeoff_cloud::token::load();
		let mut s = Self {
			sessions: HashMap::new(),
			turn: None,
			turn_doc: None,
			live: Live::default(),
			input: String::new(),
			queued: None,
			server: None,
			server_state: ServerState::Stopped,
			server_polled: Instant::now(),
			installed: store::installed(),
			fits: HashMap::new(),
			downloads: vec![],
			cloud,
			cloud_models: vec![],
			cloud_me: None,
			cloud_flow: CloudFlow::Idle,
			cloud_error: None,
			models_open: false,
			models_tab: 0,
			probe: None,
			thumbs: HashMap::new(),
			usage: (0, 0),
			hosts: HashMap::new(),
			channels: Channel::ALL.into_iter().map(|c| (c, Arc::default())).collect(),
		};
		s.refresh_cloud_async();
		s
	}

	fn cloud_cfg(settings: &Settings) -> takeoff_cloud::CloudConfig {
		if settings.cloud_url.trim().is_empty() {
			takeoff_cloud::CloudConfig::default()
		} else {
			takeoff_cloud::CloudConfig { site_url: settings.cloud_url.trim().trim_end_matches('/').to_string() }
		}
	}

	fn refresh_cloud_async(&mut self) {
		// Cheap enough to do inline on open; the UI shows the cached list meanwhile.
		if let Some((token, _)) = self.cloud.clone() {
			let token = &token;
			let cfg = takeoff_cloud::CloudConfig::default();
			match takeoff_cloud::models(&cfg, token) {
				Ok(m) => self.cloud_models = m.into_iter().filter(|m| m.vision).collect(),
				Err(e) if takeoff_cloud::is_unauthorized(&e) => {
					takeoff_cloud::token::clear();
					self.cloud = None;
				}
				Err(e) => self.cloud_error = Some(e),
			}
			self.cloud_me = takeoff_cloud::me(&cfg, token).ok();
		}
	}

	fn session(&mut self, path: &PathBuf) -> Arc<Mutex<ChatSession>> {
		self.sessions
			.entry(path.clone())
			.or_insert_with(|| {
				let s = std::fs::read(chat_path(path)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(ChatSession::new);
				Arc::new(Mutex::new(s))
			})
			.clone()
	}

	fn save_session(&self, path: &PathBuf) {
		if let Some(s) = self.sessions.get(path) {
			if let Ok(json) = serde_json::to_vec(&*s.lock()) {
				let _ = std::fs::write(chat_path(path), json);
			}
		}
	}

	/// Put the open chat in history (if it has anything) and start an empty one.
	fn new_conversation(&mut self, path: &PathBuf) {
		let session = self.session(path);
		let old = std::mem::take(&mut *session.lock());
		archive_chat(path, &old);
		self.save_session(path);
		self.live = Live::default();
	}

	fn history_menu(&mut self, ui: &mut egui::Ui, path: &PathBuf, p: &Palette) {
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

	#[doc(hidden)]
	pub fn debug_session(&mut self, path: &PathBuf) -> Arc<Mutex<ChatSession>> {
		self.session(path)
	}

	#[doc(hidden)]
	pub fn debug_open(&mut self, tab: usize) {
		self.models_open = true;
		self.models_tab = tab;
	}

	pub fn busy(&self) -> bool {
		self.turn.is_some() || self.queued.is_some()
	}

	// ── Model selection ─────────────────────────────────────────────────

	fn model_label(&self, settings: &Settings) -> (String, Color32, &'static str) {
		let m = &settings.model;
		if let Some(id) = m.strip_prefix("local:") {
			let label = self.installed.iter().find(|x| x.id == id).map(|x| x.label.clone()).unwrap_or_else(|| id.to_string());
			let (color, state) = match &self.server_state {
				ServerState::Ready => (Color32::from_rgb(0x34, 0xD3, 0x99), "ready"),
				ServerState::Starting => (Color32::from_rgb(0xFB, 0xBF, 0x24), "loading"),
				ServerState::Failed(_) => (Color32::from_rgb(0xF8, 0x71, 0x71), "failed"),
				ServerState::Stopped => (Color32::from_gray(140), "not loaded"),
			};
			(label, color, state)
		} else if let Some(id) = m.strip_prefix("cloud:") {
			let label = self.cloud_models.iter().find(|x| x.id == id).map(|x| x.name.clone()).unwrap_or_else(|| id.to_string());
			let ok = self.cloud.is_some();
			(label, if ok { Color32::from_rgb(0x4C, 0xC2, 0xFF) } else { Color32::from_gray(140) }, if ok { "cloud" } else { "signed out" })
		} else if let Some((ch, model)) = channel_model(m) {
			let label = format!("{} · {}", ch.name(), if model.is_empty() { "default" } else { model });
			let color = match ch {
				Channel::ClaudeCode => Color32::from_rgb(0xD9, 0x77, 0x57),
				Channel::Codex => Color32::from_rgb(0x22, 0xD3, 0xEE),
			};
			(label, color, "subscription")
		} else if m == "custom" {
			(settings.custom_model.clone(), Color32::from_rgb(0xA7, 0x8B, 0xFA), "custom")
		} else {
			("Choose a model".into(), Color32::from_gray(140), "")
		}
	}

	/// First use: signed in → the account's default cloud model; else the biggest
	/// local vision model that fits right now.
	pub fn auto_pick_model(&mut self, settings: &mut Settings) {
		if !settings.model.is_empty() {
			return;
		}
		if self.cloud.is_some() {
			let default = self.cloud_me.as_ref().and_then(|m| m.default_model.clone()).filter(|id| self.cloud_models.iter().any(|c| &c.id == id));
			if let Some(id) = default.or_else(|| self.cloud_models.first().map(|m| m.id.clone())) {
				settings.model = format!("cloud:{id}");
				settings.save();
				return;
			}
		}
		let cache = Self::cache(settings);
		let mut best: Option<(u64, String)> = None;
		for m in self.installed.iter().filter(|m| m.vision) {
			let fit = takeoff_models::fit_for(m, settings.context_tokens, cache);
			if fit.verdict == Verdict::Fits && best.as_ref().is_none_or(|(sz, _)| m.size_bytes > *sz) {
				best = Some((m.size_bytes, m.id.clone()));
			}
		}
		if let Some((_, id)) = best {
			settings.model = format!("local:{id}");
			settings.save();
		}
	}

	fn provider(&mut self, settings: &Settings) -> Result<ProviderConfig, String> {
		let m = settings.model.clone();
		if let Some(id) = m.strip_prefix("local:") {
			let model = self.installed.iter().find(|x| x.id == id).cloned().ok_or("That model is no longer installed")?;
			let running = self.server.as_ref().is_some_and(|s| s.model_id() == model.id);
			if !running {
				self.start_server(&model, settings)?;
				return Err("loading".into());
			}
			if self.server_state != ServerState::Ready {
				return Err("loading".into());
			}
			let s = self.server.as_ref().unwrap();
			return Ok(ProviderConfig { base_url: s.base_url(), api_key: None, model: s.model_id(), context_tokens: settings.context_tokens, vision: model.vision });
		}
		if let Some(id) = m.strip_prefix("cloud:") {
			let (token, _) = self.cloud.clone().ok_or("Sign in to construction.live to use cloud models")?;
			let ctx = self.cloud_models.iter().find(|x| x.id == id).map(|x| x.context_tokens).unwrap_or(128_000);
			return Ok(ProviderConfig { base_url: takeoff_cloud::chat_base_url(&Self::cloud_cfg(settings)), api_key: Some(token), model: id.to_string(), context_tokens: ctx, vision: true });
		}
		if m == "custom" {
			if settings.custom_base_url.trim().is_empty() || settings.custom_model.trim().is_empty() {
				return Err("Set the custom endpoint URL and model in Models".into());
			}
			return Ok(ProviderConfig {
				base_url: settings.custom_base_url.trim().trim_end_matches('/').to_string(),
				api_key: (!settings.custom_api_key.is_empty()).then(|| settings.custom_api_key.clone()),
				model: settings.custom_model.trim().to_string(),
				context_tokens: settings.context_tokens,
				vision: true,
			});
		}
		Err("Choose a model first".into())
	}

	fn cache(settings: &Settings) -> CacheType {
		if settings.kv_q8 { CacheType::Q8_0 } else { CacheType::F16 }
	}

	fn start_server(&mut self, model: &InstalledModel, settings: &Settings) -> Result<(), String> {
		let fit = takeoff_models::fit_for(model, settings.context_tokens, Self::cache(settings));
		if fit.verdict == Verdict::TooBig {
			return Err(format!("{} won't fit in memory: {}", model.label, fit.summary()));
		}
		if let Some(mut s) = self.server.take() {
			s.stop();
		}
		let s = LlamaServer::start(model, settings.context_tokens, Self::cache(settings))?;
		self.server = Some(s);
		self.server_state = ServerState::Starting;
		Ok(())
	}

	fn poll_server(&mut self, ctx: &egui::Context) {
		if let Some(s) = &mut self.server {
			if self.server_state != ServerState::Ready && self.server_polled.elapsed() > Duration::from_millis(500) {
				self.server_polled = Instant::now();
				self.server_state = s.state();
				ctx.request_repaint_after(Duration::from_millis(500));
			}
		}
	}

	// ── Sending ─────────────────────────────────────────────────────────

	pub fn send(&mut self, text: String, doc: &OpenDoc, settings: &Settings) {
		if text.trim().is_empty() || self.turn.is_some() {
			return;
		}
		takeoff_agent::set_local_thinking(settings.local_thinking);
		if let Some((ch, model)) = channel_model(&settings.model) {
			let model = model.to_string();
			match self.start_channel_turn(ch, model, text, doc, settings) {
				Ok(()) => {}
				Err(e) => self.live = Live { error: Some(e), ..Default::default() },
			}
			return;
		}
		match self.provider(settings) {
			Ok(provider) => {
				let session = self.session(&doc.path);
				let ctx = AgentContext {
					doc: doc.doc.clone(),
					pdf_path: doc.path.clone(),
					current_page: doc.page,
					visible_region: visible_region(doc),
					selected_ids: doc.selection.iter().cloned().collect(),
					provider,
				};
				let transcript_len = session.lock().transcript().len();
				self.live = Live { started: Some(Instant::now()), transcript_len, asked: text.clone(), ..Default::default() };
				self.turn = Some(takeoff_agent::start_turn(session, ctx, text));
				self.turn_doc = Some(doc.path.clone());
				self.queued = None;
			}
			Err(e) if e == "loading" => self.queued = Some(text),
			Err(e) => {
				self.live = Live { error: Some(e), ..Default::default() };
				self.queued = None;
			}
		}
	}

	/// A turn on the estimator's Claude Code / Codex: the CLI runs the
	/// conversation, the app serves it the drawing tools.
	fn start_channel_turn(&mut self, ch: Channel, model: String, text: String, doc: &OpenDoc, settings: &Settings) -> Result<(), String> {
		let exe = channel::find_exe(ch).ok_or_else(|| format!("{} isn't installed on this computer. See Models → Subscriptions.", ch.name()))?;
		if !self.hosts.get(&doc.path).is_some_and(|h| h.serves(&doc.doc)) {
			let host = ToolHost::start(doc.doc.clone(), doc.path.clone())?;
			self.hosts.insert(doc.path.clone(), host);
		}
		let sweeps = self.sweep_provider(settings);
		let session = self.session(&doc.path);
		let turn = channel::ChannelTurn {
			channel: ch,
			exe,
			model,
			workdir: channel_workdir(),
			sweeps,
			current_page: doc.page,
			visible_region: visible_region(doc),
			selected_ids: doc.selection.iter().cloned().collect(),
		};
		let transcript_len = session.lock().transcript().len();
		self.live = Live { started: Some(Instant::now()), transcript_len, asked: text.clone(), ..Default::default() };
		self.turn = Some(channel::start_turn(&self.hosts[&doc.path], session, turn, text));
		self.turn_doc = Some(doc.path.clone());
		self.queued = None;
		Ok(())
	}

	/// The model detect / calibrate / trace call for their tiles when the
	/// chat itself runs on a subscription: the construction.live default when
	/// signed in, else a local vision model that is already loaded.
	fn sweep_provider(&self, settings: &Settings) -> Option<ProviderConfig> {
		if let Some((token, _)) = &self.cloud {
			let default = self.cloud_me.as_ref().and_then(|m| m.default_model.clone());
			let m = self.cloud_models.iter().find(|c| Some(&c.id) == default.as_ref()).or_else(|| self.cloud_models.first())?;
			return Some(ProviderConfig { base_url: takeoff_cloud::chat_base_url(&Self::cloud_cfg(settings)), api_key: Some(token.clone()), model: m.id.clone(), context_tokens: m.context_tokens, vision: true });
		}
		let s = self.server.as_ref().filter(|_| self.server_state == ServerState::Ready)?;
		let vision = self.installed.iter().any(|m| m.id == s.model_id() && m.vision);
		vision.then(|| ProviderConfig { base_url: s.base_url(), api_key: None, model: s.model_id(), context_tokens: settings.context_tokens, vision: true })
	}

	/// Drain agent events; returns true while a turn is running.
	pub fn poll(&mut self, ctx: &egui::Context, canvas: &mut Canvas, doc: Option<&mut OpenDoc>, settings: &Settings) {
		self.poll_server(ctx);
		for d in &self.downloads {
			if matches!(d.progress().state, DownloadState::Running) {
				ctx.request_repaint_after(Duration::from_millis(250));
			}
		}
		if self.downloads.iter().any(|d| d.progress().state == DownloadState::Done) {
			self.downloads.retain(|d| d.progress().state != DownloadState::Done);
			self.installed = store::installed();
			self.fits.clear();
		}
		if let CloudFlow::Waiting { code, started, last_poll } = &mut self.cloud_flow {
			if started.elapsed().as_secs() > code.expires_in {
				self.cloud_flow = CloudFlow::Failed("The sign-in code expired. Try again.".into());
			} else if last_poll.elapsed().as_secs() >= code.interval.max(2) {
				*last_poll = Instant::now();
				match takeoff_cloud::poll(&Self::cloud_cfg(settings), code) {
					Ok(takeoff_cloud::PollResult::Approved { token, account }) => {
						let _ = takeoff_cloud::token::save(&token, &account);
						self.cloud = Some((token, account));
						self.cloud_flow = CloudFlow::Idle;
						self.refresh_cloud_async();
					}
					Ok(takeoff_cloud::PollResult::Denied) => self.cloud_flow = CloudFlow::Failed("Sign-in was denied.".into()),
					Ok(takeoff_cloud::PollResult::Expired) => self.cloud_flow = CloudFlow::Failed("The sign-in code expired. Try again.".into()),
					Ok(takeoff_cloud::PollResult::Pending) => {}
					Err(e) => self.cloud_flow = CloudFlow::Failed(e),
				}
			}
			ctx.request_repaint_after(Duration::from_millis(500));
		}
		if let (Some(q), Some(d)) = (self.queued.clone(), doc.as_deref()) {
			if self.server_state == ServerState::Ready {
				self.send(q, d, settings);
			} else if let ServerState::Failed(e) = &self.server_state {
				self.live.error = Some(format!("The local model failed to start: {}", e.lines().last().unwrap_or(e)));
				self.queued = None;
			}
		}
		let Some(turn) = &self.turn else { return };
		let mut finished = false;
		let mut disconnected = false;
		loop {
			let ev = match turn.events.try_recv() {
				Ok(ev) => ev,
				Err(crossbeam_channel::TryRecvError::Empty) => break,
				Err(crossbeam_channel::TryRecvError::Disconnected) => {
					disconnected = true;
					break;
				}
			};
			match ev {
				AgentEvent::TextDelta(t) => self.live.text.push_str(&t),
				AgentEvent::ReasoningDelta(t) => self.live.reasoning.push_str(&t),
				AgentEvent::ToolStarted { call_id, name, args: _ } => {
					self.live.text.push_str("\n\n");
					self.live.steps.push(Step { call_id, name, summary: String::new(), ok: None, thumb: None });
				}
				AgentEvent::ToolFinished { call_id, name, ok, summary, image } => {
					let thumb = image.map(|img| {
						let small = image::imageops::thumbnail(&img, (img.width() as f32 * 240.0 / img.width().max(img.height()) as f32) as u32, (img.height() as f32 * 240.0 / img.width().max(img.height()) as f32) as u32);
						ctx.load_texture(format!("step-{call_id}"), egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw()), egui::TextureOptions::LINEAR)
					});
					if let Some(s) = self.live.steps.iter_mut().find(|s| s.call_id == call_id) {
						s.ok = Some(ok);
						s.summary = summary;
						s.thumb = thumb;
					} else {
						self.live.steps.push(Step { call_id, name, summary, ok: Some(ok), thumb });
					}
				}
				AgentEvent::Viewport { page, region, label } => canvas.ghost = Some((page, region, label, Instant::now())),
				AgentEvent::DocChanged => {}
				AgentEvent::Usage { prompt_tokens, completion_tokens } => {
					self.usage.0 += prompt_tokens;
					self.usage.1 += completion_tokens;
				}
				AgentEvent::Error(e) => self.live.error = Some(e),
				AgentEvent::Done { .. } => finished = true,
			}
		}
		if finished || disconnected {
			self.turn = None;
			if let Some(p) = self.turn_doc.take() {
				self.save_session(&p);
			}
			let keep_error = self.live.error.take();
			self.live = Live { error: keep_error, ..Default::default() };
			canvas.ghost = None;
		} else {
			ctx.request_repaint_after(Duration::from_millis(80));
		}
	}

	pub fn stop(&mut self) {
		if let Some(t) = &self.turn {
			t.cancel();
		}
		self.queued = None;
	}

	// ── Panel ───────────────────────────────────────────────────────────

	pub fn ui(&mut self, ui: &mut egui::Ui, doc: Option<&mut OpenDoc>, canvas: &mut Canvas, settings: &mut Settings, p: &Palette) {
		// Model picker: a full-width pill (provider badge · model · status · chevron).
		let (label, dot, state) = self.model_label(settings);
		let (badge, glyph) = provider_badge(&settings.model);
		ui.horizontal(|ui| {
			let w = (ui.available_width() - 72.0).max(180.0);
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
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				if let Some(d) = &doc {
					let path = d.path.clone();
					let r = widgets::icon_button(ui, p, i::HISTORY, "Past conversations", false);
					egui::Popup::menu(&r).show(|ui| self.history_menu(ui, &path, p));
					if widgets::icon_button(ui, p, i::PLUS, "New conversation (this one stays in history)", false).clicked() && self.turn.is_none() {
						self.new_conversation(&path);
					}
				}
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
		let composer_h = 118.0;
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
		ui.add_space(4.0);
		let sel = doc.selection.len();
		// Wrapped, so a long sheet name can't push the panel wider than it is.
		ui.horizontal_wrapped(|ui| {
			chip(ui, p, &format!("{} {}", i::FILE_TEXT, doc.page_label(doc.page)));
			if sel > 0 {
				chip(ui, p, &format!("{} {sel} selected", i::MOUSE_POINTER_2));
			}
		});
		let frame = egui::Frame::new().fill(p.card).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 8));
		frame.show(ui, |ui| {
			let te = egui::TextEdit::multiline(&mut self.input)
				.hint_text("Ask the drawing… e.g. “count the duplex outlets on this sheet”")
				.desired_rows(2)
				.desired_width(f32::INFINITY)
				.frame(egui::Frame::NONE)
				.id(egui::Id::new("ai-input"));
			let r = ui.add(te);
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
						let can = !self.input.trim().is_empty();
						let b = ui.add_enabled(can, egui::Button::new(RichText::new(format!("{}  Send", i::SEND)).color(p.on_accent).font(theme::semibold(12.5))).fill(p.accent_fill).corner_radius(4));
						if (b.clicked() || enter) && can {
							let text = std::mem::take(&mut self.input).trim().to_string();
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
		let _ = canvas;
	}

	/// The model picker's dropdown: every model you can switch to right now,
	/// grouped by where it runs, plus the Models window for setting more up.
	fn model_menu(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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
		group_header(ui, p, i::CLOUD, "construction.live", "uses your credits");
		if self.cloud.is_some() && !self.cloud_models.is_empty() {
			for m in self.cloud_models.clone() {
				let id = format!("cloud:{}", m.id);
				if model_row(ui, p, (p.accent_fill, i::CLOUD), &m.name, &format!("{}K context · sees drawings", m.context_tokens / 1000), settings.model == id, true).clicked() {
					chosen = Some(id);
				}
			}
		} else if action_row(ui, p, i::LOG_IN, "Sign in to construction.live", "Gemini, Claude, GPT and more on your account").clicked() {
			setup = Some(2);
		}

		for ch in Channel::ALL {
			ui.add_space(4.0);
			let (color, glyph, plan) = match ch {
				Channel::ClaudeCode => (Color32::from_rgb(0xD9, 0x77, 0x57), i::SPARKLES, "your Claude plan"),
				Channel::Codex => (Color32::from_rgb(0x10, 0xA3, 0x7F), i::BOT, "your ChatGPT plan"),
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
				(Some(st), Some(Ok(models))) if st.exe.is_some() && st.signed_in != Some(false) => {
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

	fn welcome(&mut self, ui: &mut egui::Ui, doc: &OpenDoc, settings: &Settings, p: &Palette) {
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

	// ── Models window ───────────────────────────────────────────────────

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

	fn verdict_chip(ui: &mut egui::Ui, p: &Palette, fit: &FitReport) {
		let (text, color) = match fit.verdict {
			Verdict::Fits => ("Fits", p.success),
			Verdict::Tight => ("Tight", p.warning),
			Verdict::TooBig => ("Not enough memory", p.danger),
		};
		ui.label(RichText::new(format!("● {text}")).color(color).font(theme::semibold(12.0))).on_hover_text(fit.summary());
	}

	fn installed_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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

	fn download_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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

	fn cloud_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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
				if self.cloud_models.is_empty() {
					if ui.button(format!("{}  Load models", i::REFRESH_CW)).clicked() {
						self.refresh_cloud_async();
					}
					if let Some(e) = &self.cloud_error {
						ui.label(RichText::new(e).color(p.danger));
					}
				}
				for m in self.cloud_models.clone() {
					ui.horizontal(|ui| {
						ui.label(RichText::new(&m.name).font(theme::medium(13.0)));
						ui.label(RichText::new(format!("{}K context", m.context_tokens / 1000)).size(11.5).color(p.text2));
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
	fn refresh_channels(&self, ctx: &egui::Context) {
		for (&ch, info) in &self.channels {
			if info.lock().loading {
				continue;
			}
			info.lock().loading = true;
			let (info, ctx) = (info.clone(), ctx.clone());
			std::thread::spawn(move || {
				let st = channel::status(ch);
				let models = st.exe.as_ref().filter(|_| st.signed_in != Some(false)).map(|exe| channel::models(ch, exe));
				let mut i = info.lock();
				i.status = Some(st);
				i.models = models;
				i.loading = false;
				ctx.request_repaint();
			});
		}
	}

	fn subscriptions_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
		if self.channels.values().any(|i| { let i = i.lock(); i.status.is_none() && !i.loading }) {
			self.refresh_channels(ui.ctx());
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
						widgets::hint(ui, p, "Install it in a terminal, then sign in once and press Check again:");
						for cmd in [ch.install_command(), ch.login_command()] {
							ui.horizontal(|ui| {
								ui.label(RichText::new(cmd).monospace().size(12.0));
								if ui.small_button(i::COPY).on_hover_text("Copy").clicked() {
									ui.ctx().copy_text(cmd.to_string());
								}
							});
						}
						return;
					}
					Some(st) => {
						let version = st.version.clone().unwrap_or_default();
						match st.signed_in {
							Some(false) => {
								ui.label(RichText::new(format!("{} Installed {version} · not signed in", i::CIRCLE_ALERT)).color(p.warning));
								ui.horizontal(|ui| {
									widgets::hint(ui, p, "Sign in once in a terminal:");
									ui.label(RichText::new(ch.login_command()).monospace().size(12.0));
									if ui.small_button(i::COPY).on_hover_text("Copy").clicked() {
										ui.ctx().copy_text(ch.login_command().to_string());
									}
								});
								return;
							}
							_ => {
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

	fn custom_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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
fn short_fit(f: &FitReport) -> String {
	let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
	format!("Needs {:.1} GB · this computer allows {:.1} GB · {:.1} GB free now", gb(f.estimate.total()), gb(f.budget), gb(f.available_now))
}

/// An earlier conversation about one drawing, kept in history.
struct PastChat {
	file: PathBuf,
	title: String,
	saved_ms: i64,
	messages: usize,
}

/// `chats/<drawing>.history/`: earlier conversations, one file each.
fn history_dir(pdf: &std::path::Path) -> PathBuf {
	chat_path(pdf).with_extension("history")
}

fn archive_chat(pdf: &std::path::Path, chat: &ChatSession) {
	if chat.messages.is_empty() {
		return;
	}
	let dir = history_dir(pdf);
	let _ = std::fs::create_dir_all(&dir);
	if let Ok(json) = serde_json::to_vec(chat) {
		let _ = std::fs::write(dir.join(format!("{}.json", takeoff_core::now_ms())), json);
	}
}

/// Newest first.
fn past_chats(pdf: &std::path::Path) -> Vec<PastChat> {
	let mut out: Vec<PastChat> = std::fs::read_dir(history_dir(pdf))
		.into_iter()
		.flatten()
		.flatten()
		.filter_map(|e| {
			let file = e.path();
			let saved_ms = file.file_stem()?.to_str()?.parse().ok()?;
			let chat: ChatSession = serde_json::from_slice(&std::fs::read(&file).ok()?).ok()?;
			let t = chat.transcript();
			let title = t.iter().find(|e| e.role == Role::User).map(|e| e.text.clone()).unwrap_or_else(|| "Conversation".into());
			let messages = t.iter().filter(|e| e.role != Role::Tool).count();
			Some(PastChat { file, title, saved_ms, messages })
		})
		.collect();
	out.sort_by_key(|c| std::cmp::Reverse(c.saved_ms));
	out
}

/// "5 min ago", "3 h ago", else the date.
fn ago(ms: i64) -> String {
	let mins = (takeoff_core::now_ms() - ms).max(0) / 60_000;
	match mins {
		0 => "just now".into(),
		1..=59 => format!("{mins} min ago"),
		60..=1439 => format!("{} h ago", mins / 60),
		_ => takeoff_core::time::date_from_ms(ms),
	}
}

fn chat_path(pdf: &std::path::Path) -> PathBuf {
	use std::hash::{Hash, Hasher};
	let mut h = std::collections::hash_map::DefaultHasher::new();
	pdf.hash(&mut h);
	let dir = config_dir().join("chats");
	let _ = std::fs::create_dir_all(&dir);
	dir.join(format!("{:016x}.json", h.finish()))
}

fn hostname() -> String {
	takeoff_models::quiet_command("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "this computer".into())
}

fn eta(secs: f64) -> String {
	let s = secs as u64;
	if s >= 3600 { format!("{}h {}m", s / 3600, s % 3600 / 60) } else if s >= 60 { format!("{}m", s / 60) } else { format!("{s}s") }
}

fn visible_region(doc: &OpenDoc) -> Option<(f64, f64, f64, f64)> {
	let pr = doc.page_rect();
	let vis = pr.intersect(doc.canvas_rect);
	if vis.width() <= 0.0 || pr.width() <= 0.0 {
		return None;
	}
	let r = ((vis.min.x - pr.min.x) / pr.width(), (vis.min.y - pr.min.y) / pr.height(), vis.width() / pr.width(), vis.height() / pr.height());
	// Whole page visible → no region hint.
	(r.2 < 0.98 || r.3 < 0.98).then_some((r.0 as f64, r.1 as f64, r.2 as f64, r.3 as f64))
}

fn short(s: &str, n: usize) -> String {
	let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
	if one.chars().count() > n { format!("{}…", one.chars().take(n).collect::<String>()) } else { one }
}

fn tool_title(name: &str) -> String {
	let mut s = name.replace('_', " ");
	if let Some(f) = s.get_mut(0..1) {
		f.make_ascii_uppercase();
	}
	s
}

fn chip(ui: &mut egui::Ui, p: &Palette, text: &str) {
	egui::Frame::new().fill(p.hover).corner_radius(10).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
		ui.add(egui::Label::new(RichText::new(text).size(11.5).color(p.text2)).truncate());
	});
}

fn user_bubble(ui: &mut egui::Ui, p: &Palette, text: &str) {
	ui.add_space(6.0);
	ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
		let max = ui.available_width() * 0.85;
		egui::Frame::new().fill(if p.dark { Color32::from_rgb(0x2A, 0x1E, 0x1A) } else { Color32::from_rgb(0xFD, 0xEE, 0xE7) }).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
			ui.set_max_width(max);
			ui.label(RichText::new(text).size(13.0).color(p.text));
		});
	});
	ui.add_space(4.0);
}

/// Light markdown: headings, bullets, numbered lists, **bold**, `code`, tables as monospace.
pub fn markdown(ui: &mut egui::Ui, p: &Palette, text: &str) {
	let mut in_code = false;
	for line in text.lines() {
		let t = line.trim_end();
		if t.trim_start().starts_with("```") {
			in_code = !in_code;
			continue;
		}
		if in_code || t.trim_start().starts_with('|') {
			ui.label(RichText::new(t).monospace().size(11.5).color(p.text));
			continue;
		}
		if t.trim().is_empty() {
			ui.add_space(4.0);
			continue;
		}
		let (indent, body) = {
			let trimmed = t.trim_start();
			((t.len() - trimmed.len()) as f32 * 4.0, trimmed)
		};
		ui.horizontal_wrapped(|ui| {
			ui.add_space(indent);
			ui.spacing_mut().item_spacing.x = 0.0;
			let (prefix, rest, size, strong) = if let Some(r) = body.strip_prefix("### ") {
				("", r, 13.5, true)
			} else if let Some(r) = body.strip_prefix("## ") {
				("", r, 14.5, true)
			} else if let Some(r) = body.strip_prefix("# ") {
				("", r, 15.5, true)
			} else if let Some(r) = body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
				("•  ", r, 13.0, false)
			} else {
				("", body, 13.0, false)
			};
			if !prefix.is_empty() {
				ui.label(RichText::new(prefix).size(size).color(p.accent));
			}
			// **bold** and `code` spans.
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
						ui.label(RichText::new(seg).monospace().size(12.0).color(p.accent));
						continue;
					}
					// *italic* spans (single asterisks; ** is handled above).
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
						ui.label(rt);
					}
				}
			}
		});
	}
}

/// The provider's badge for the current model id.
fn provider_badge(model: &str) -> (Color32, &'static str) {
	if model.starts_with("local:") {
		(Color32::from_rgb(0x10, 0xB9, 0x81), i::CPU)
	} else if model.starts_with("cloud:") {
		(Color32::from_rgb(0xE8, 0x53, 0x1A), i::CLOUD)
	} else if model.starts_with("claude:") {
		(Color32::from_rgb(0xD9, 0x77, 0x57), i::SPARKLES)
	} else if model.starts_with("codex:") {
		(Color32::from_rgb(0x10, 0xA3, 0x7F), i::BOT)
	} else if model == "custom" {
		(Color32::from_rgb(0xA7, 0x8B, 0xFA), i::SERVER)
	} else {
		(Color32::from_gray(110), i::SPARKLES)
	}
}

fn group_header(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, note: &str) {
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

/// One choice: badge, title, one-line description, check when selected.
fn model_row(ui: &mut egui::Ui, p: &Palette, badge: (Color32, &str), title: &str, sub: &str, selected: bool, enabled: bool) -> egui::Response {
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
fn action_row(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, sub: &str) -> egui::Response {
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
