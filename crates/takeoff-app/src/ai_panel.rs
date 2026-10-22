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
use takeoff_agent::{AgentContext, AgentEvent, ChatSession, ProviderConfig, Role, TurnHandle, UserImage, ViewAttachment};
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
	asked_attachments: Vec<ViewAttachment>,
	asked_images: Vec<UserImage>,
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
	/// `Update` pressed: None while it runs, then its result.
	update: Option<Option<Result<String, String>>>,
	/// Install / Sign in opened a terminal at this time: check again every few seconds.
	waiting: Option<Instant>,
	/// The terminal couldn't be opened.
	launch_error: Option<String>,
}

/// What Graph view shows about the chat that maps the set.
#[derive(Debug, Clone, PartialEq)]
pub struct MapChat {
	/// The chat's model, as its picker shows it.
	pub model: String,
	pub color: Color32,
	/// The AI is mapping this drawing's set right now.
	pub running_here: bool,
	/// The AI is busy with something else.
	pub busy_elsewhere: bool,
	/// No model chosen yet.
	pub unset: bool,
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

/// One background read of the account's cloud models and details.
struct CloudFetch {
	models: Result<Vec<takeoff_cloud::CloudModel>, String>,
	me: Option<takeoff_cloud::Me>,
}

/// How often the cloud model list is re-read on its own (access granted or
/// revoked on construction.live shows up without a restart).
const CLOUD_REFRESH_EVERY: Duration = Duration::from_secs(300);
/// Opening the model picker re-reads the list if it is older than this.
const CLOUD_REFRESH_ON_OPEN: Duration = Duration::from_secs(20);

enum CloudFlow {
	Idle,
	Waiting { code: takeoff_cloud::DeviceCode, started: Instant, last_poll: Instant },
	Failed(String),
}

pub struct AiPanel {
	/// The other open tabs, set by the app each frame; the agent can switch to them.
	pub others: Vec<takeoff_agent::tools::OtherDrawing>,
	/// Views attached to the message being written.
	attached: Vec<ViewAttachment>,
	/// Pictures added to the message being written (file, drop or paste).
	attached_images: Vec<UserImage>,
	/// Why the last picture could not be added.
	image_error: Option<String>,
	/// A V key press reached us since the last release (so a release without
	/// one was Cmd/Ctrl+V, which egui swallows).
	v_pressed_seen: bool,
	/// When egui last pasted text (then Cmd+V was a text paste, not a picture).
	last_text_paste: Option<Instant>,
	/// Where the AI just looked, for the app to follow: (drawing, page, region).
	pub follow_request: Option<(PathBuf, u32, (f64, f64, f64, f64))>,
	/// Last automatic subscription re-check (while waiting on Install / Sign in).
	last_channel_poll: Option<Instant>,
	/// The composer's height last frame (it grows as you type); the chat gets the rest.
	composer_h: f32,
	sessions: HashMap<PathBuf, Arc<Mutex<ChatSession>>>,
	turn: Option<TurnHandle>,
	turn_doc: Option<PathBuf>,
	/// The drawing whose set the running turn is mapping (started from Graph view).
	mapping: Option<PathBuf>,
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
	/// The model list being re-read in the background.
	cloud_fetch: Option<Arc<Mutex<Option<CloudFetch>>>>,
	/// When the list was last asked for.
	cloud_fetched: Option<Instant>,
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
			others: Vec::new(),
			attached: Vec::new(),
			attached_images: Vec::new(),
			image_error: None,
			v_pressed_seen: false,
			last_text_paste: None,
			follow_request: None,
			last_channel_poll: None,
			composer_h: 118.0,
			sessions: HashMap::new(),
			turn: None,
			turn_doc: None,
			mapping: None,
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
			cloud_fetch: None,
			cloud_fetched: None,
			models_open: false,
			models_tab: 0,
			probe: None,
			thumbs: HashMap::new(),
			usage: (0, 0),
			hosts: HashMap::new(),
			channels: Channel::ALL.into_iter().map(|c| (c, Arc::default())).collect(),
		};
		s.refresh_cloud(takeoff_cloud::CloudConfig::default());
		s
	}

	fn cloud_cfg(settings: &Settings) -> takeoff_cloud::CloudConfig {
		if settings.cloud_url.trim().is_empty() {
			takeoff_cloud::CloudConfig::default()
		} else {
			takeoff_cloud::CloudConfig { site_url: settings.cloud_url.trim().trim_end_matches('/').to_string() }
		}
	}

	/// Re-read the account's models and details on a background thread; the
	/// UI keeps showing the last list meanwhile. Picked up in [`Self::poll`].
	fn refresh_cloud(&mut self, cfg: takeoff_cloud::CloudConfig) {
		if self.cloud_fetch.is_some() {
			return;
		}
		let Some((token, _)) = self.cloud.clone() else { return };
		let slot: Arc<Mutex<Option<CloudFetch>>> = Arc::default();
		let out = slot.clone();
		let spawned = std::thread::Builder::new().name("cloud-models".into()).spawn(move || {
			let models = takeoff_cloud::models(&cfg, &token);
			let me = if models.as_ref().is_err_and(|e| takeoff_cloud::is_unauthorized(e)) { None } else { takeoff_cloud::me(&cfg, &token).ok() };
			*out.lock() = Some(CloudFetch { models, me });
		});
		if spawned.is_ok() {
			self.cloud_fetch = Some(slot);
			self.cloud_fetched = Some(Instant::now());
		}
	}

	/// Refresh unless a read is running or the list is fresher than `max_age`.
	fn refresh_cloud_if_older(&mut self, settings: &Settings, max_age: Duration) {
		if self.cloud.is_some() && self.cloud_fetched.is_none_or(|t| t.elapsed() >= max_age) {
			self.refresh_cloud(Self::cloud_cfg(settings));
		}
	}

	fn cloud_refreshing(&self) -> bool {
		self.cloud_fetch.is_some()
	}

	/// Take a finished background read.
	fn poll_cloud_fetch(&mut self, ctx: &egui::Context) {
		let Some(slot) = &self.cloud_fetch else { return };
		let Some(done) = slot.lock().take() else {
			ctx.request_repaint_after(Duration::from_millis(200));
			return;
		};
		self.cloud_fetch = None;
		match done.models {
			Ok(mut m) => {
				// Models that can see the drawing first; the rest are text only.
				m.sort_by_key(|x| !x.vision);
				self.cloud_models = m;
				self.cloud_error = None;
			}
			Err(e) if takeoff_cloud::is_unauthorized(&e) => {
				takeoff_cloud::token::clear();
				self.cloud = None;
				self.cloud_models.clear();
				self.cloud_me = None;
				self.cloud_error = Some("Your construction.live sign-in has ended. Sign in again.".into());
			}
			Err(e) => self.cloud_error = Some(e),
		}
		if done.me.is_some() {
			self.cloud_me = done.me;
		}
		ctx.request_repaint();
	}

	/// Whether a cloud model can see images (unknown ids are assumed to).
	fn cloud_vision(&self, id: &str) -> bool {
		self.cloud_models.iter().find(|x| x.id == id).is_none_or(|x| x.vision)
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
			let state = if !ok { "signed out" } else if self.cloud_vision(id) { "cloud" } else { "cloud · text only" };
			(label, if ok { Color32::from_rgb(0x4C, 0xC2, 0xFF) } else { Color32::from_gray(140) }, state)
		} else if let Some((ch, model)) = channel_model(m) {
			// "opencode/deepseek-v4.1-flash-free" reads as "deepseek-v4.1-flash-free".
			let shown = model.strip_prefix("opencode/").unwrap_or(model);
			let label = format!("{} · {}", ch.name(), if shown.is_empty() { "default" } else { shown });
			let color = match ch {
				Channel::ClaudeCode => Color32::from_rgb(0xD9, 0x77, 0x57),
				Channel::Codex => Color32::from_rgb(0x22, 0xD3, 0xEE),
				Channel::OpenCode => Color32::from_rgb(0xFB, 0xBF, 0x24),
			};
			let state = if ch == Channel::OpenCode { if model.ends_with("-free") { "free" } else { "your key" } } else { "subscription" };
			(label, color, state)
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
			let default = self.cloud_me.as_ref().and_then(|m| m.default_model.clone()).filter(|id| self.cloud_models.iter().any(|c| &c.id == id && c.vision));
			if let Some(id) = default.or_else(|| self.cloud_models.iter().find(|m| m.vision).or(self.cloud_models.first()).map(|m| m.id.clone())) {
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
		self.provider_for(&m, settings)
	}

	fn provider_for(&mut self, m: &str, settings: &Settings) -> Result<ProviderConfig, String> {
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
			let vision = self.cloud_vision(id);
			return Ok(ProviderConfig { base_url: takeoff_cloud::chat_base_url(&Self::cloud_cfg(settings)), api_key: Some(token), model: id.to_string(), context_tokens: ctx, vision });
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

	// ── Document maps ───────────────────────────────────────────────────

	/// The chat as Graph view describes it, for the drawing at `path`.
	pub fn map_chat(&self, settings: &Settings, path: &PathBuf) -> MapChat {
		let (model, color, _) = self.model_label(settings);
		let running = self.busy();
		let here = running && self.mapping.as_ref() == Some(path);
		MapChat { model, color, running_here: here, busy_elsewhere: running && !here, unset: settings.model.is_empty() }
	}

	/// Start mapping the set in a fresh conversation (the current one goes to history).
	pub fn start_map_chat(&mut self, text: String, doc: &OpenDoc, settings: &Settings) -> Result<(), String> {
		if self.busy() {
			return Err("The AI is still working on another message. Stop it or let it finish, then start mapping.".into());
		}
		self.new_conversation(&doc.path);
		self.send(text, doc, settings);
		match self.live.error.clone() {
			Some(e) => Err(e),
			None => {
				self.mapping = Some(doc.path.clone());
				Ok(())
			}
		}
	}

	/// Attach an area to the next message and put a question in the prompt
	/// (unless the estimator is already writing one).
	pub fn ask_about(&mut self, att: ViewAttachment, question: &str) {
		if !self.attached.contains(&att) {
			self.attached.push(att);
		}
		if self.input.trim().is_empty() {
			self.input = question.to_string();
		}
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
					workspace: settings.workspace(),
					others: self.others.iter().filter(|o| o.path != doc.path).cloned().collect(),
					attachments: self.attached.clone(),
					images: self.attached_images.clone(),
				};
				let transcript_len = session.lock().transcript().len();
				self.live = Live { started: Some(Instant::now()), transcript_len, asked: text.clone(), asked_attachments: std::mem::take(&mut self.attached), asked_images: std::mem::take(&mut self.attached_images), ..Default::default() };
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
			workspace: settings.workspace(),
			others: self.others.iter().filter(|o| o.path != doc.path).cloned().collect(),
			attachments: self.attached.clone(),
			images: self.attached_images.clone(),
			sweeps,
			current_page: doc.page,
			visible_region: visible_region(doc),
			selected_ids: doc.selection.iter().cloned().collect(),
		};
		let transcript_len = session.lock().transcript().len();
		self.live = Live { started: Some(Instant::now()), transcript_len, asked: text.clone(), asked_attachments: std::mem::take(&mut self.attached), asked_images: std::mem::take(&mut self.attached_images), ..Default::default() };
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
			// Sweeps look at tiles, so only a model that can see will do.
			let m = self.cloud_models.iter().filter(|c| c.vision).find(|c| Some(&c.id) == default.as_ref()).or_else(|| self.cloud_models.iter().find(|c| c.vision))?;
			return Some(ProviderConfig { base_url: takeoff_cloud::chat_base_url(&Self::cloud_cfg(settings)), api_key: Some(token.clone()), model: m.id.clone(), context_tokens: m.context_tokens, vision: true });
		}
		let s = self.server.as_ref().filter(|_| self.server_state == ServerState::Ready)?;
		let vision = self.installed.iter().any(|m| m.id == s.model_id() && m.vision);
		vision.then(|| ProviderConfig { base_url: s.base_url(), api_key: None, model: s.model_id(), context_tokens: settings.context_tokens, vision: true })
	}

	/// Drain agent events; returns true while a turn is running.
	pub fn poll(&mut self, ctx: &egui::Context, canvas: &mut Canvas, doc: Option<&mut OpenDoc>, settings: &Settings) {
		self.poll_server(ctx);
		self.poll_cloud_fetch(ctx);
		self.refresh_cloud_if_older(settings, CLOUD_REFRESH_EVERY);
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
						self.cloud_error = None;
						self.refresh_cloud(Self::cloud_cfg(settings));
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
					// One paragraph break between bursts of text, however many steps run in between.
					if !self.live.text.trim().is_empty() && !self.live.text.ends_with("\n\n") {
						self.live.text.push_str("\n\n");
					}
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
				AgentEvent::Viewport { page, region, label, drawing } => {
					let here = doc.as_deref().map(|d| d.path.clone());
					let target = drawing.clone().or_else(|| self.turn_doc.clone());
					if settings.follow_agent {
						// The app switches tab and zooms there; show the frame there too.
						if let Some(t) = target {
							self.follow_request = Some((t, page, region));
						}
						canvas.ghost = Some((page, region, label, Instant::now()));
					} else if drawing.is_none() || drawing == here {
						// Only on the drawing it is working in (it may be in another tab).
						canvas.ghost = Some((page, region, label, Instant::now()));
					}
				}
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
			self.mapping = None;
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

	// ── Pictures in the message ─────────────────────────────────────────

	/// Add image files (picked, or dropped on the window) to the message being written.
	pub fn attach_image_files(&mut self, paths: &[PathBuf]) {
		for path in paths {
			let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
			match image::open(path) {
				Ok(img) => self.attach_image(img, &name),
				Err(e) => self.image_error = Some(format!("Couldn't add {name}: only PNG and JPEG pictures work ({e})")),
			}
		}
	}

	fn attach_image(&mut self, img: image::DynamicImage, name: &str) {
		if self.attached_images.len() >= MAX_IMAGES_PER_MESSAGE {
			self.image_error = Some(format!("At most {MAX_IMAGES_PER_MESSAGE} pictures per message."));
			return;
		}
		match store_chat_image(img, name) {
			Ok(u) => {
				self.attached_images.push(u);
				self.image_error = None;
			}
			Err(e) => self.image_error = Some(format!("Couldn't add {name}: {e}")),
		}
	}

	/// Add the picture on the clipboard, if there is one.
	fn paste_image(&mut self) {
		let Ok(mut cb) = arboard::Clipboard::new() else { return };
		let Ok(data) = cb.get_image() else { return };
		let Some(rgba) = image::RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned()) else { return };
		self.attach_image(image::DynamicImage::ImageRgba8(rgba), "Pasted image");
	}

	/// Whether Cmd/Ctrl+V was pressed this frame. egui turns it into a text
	/// paste and drops the key press, and with only a picture on the
	/// clipboard there is no paste either; the key release still arrives.
	fn saw_paste_shortcut(&mut self, ui: &egui::Ui) -> bool {
		let mut cmd_v = false;
		ui.input(|i| {
			for e in &i.events {
				match e {
					egui::Event::Paste(_) => self.last_text_paste = Some(Instant::now()),
					egui::Event::Key { key: egui::Key::V, pressed: true, .. } => self.v_pressed_seen = true,
					egui::Event::Key { key: egui::Key::V, pressed: false, .. } => {
						cmd_v |= !self.v_pressed_seen;
						self.v_pressed_seen = false;
					}
					_ => {}
				}
			}
		});
		cmd_v
	}

	// ── Panel ───────────────────────────────────────────────────────────

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
	fn workspace_row(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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
	fn refresh_channels(&self, ctx: &egui::Context) {
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

	fn subscriptions_tab(&mut self, ui: &mut egui::Ui, settings: &mut Settings, p: &Palette) {
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

/// "p1 · 6% of the sheet", or "Other.pdf p2 · whole page".
fn attachment_label(a: &ViewAttachment, here: &std::path::Path) -> String {
	let area = a.region.2 * a.region.3;
	let what = if area >= 0.95 { "whole page".to_string() } else { format!("{:.0}% of the sheet", (area * 100.0).max(1.0)) };
	let file = if a.drawing == here { String::new() } else { format!("{} ", a.drawing.file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()) };
	format!("{file}p{} · {what}", a.page)
}

/// A sent message's attachments; clicking one goes back to that area.
fn attachment_chips(ui: &mut egui::Ui, p: &Palette, atts: &[ViewAttachment], here: &std::path::Path, jump: &mut Option<ViewAttachment>) {
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

/// Picture files the chat accepts (what the image crate decodes here).
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg"];
const MAX_IMAGES_PER_MESSAGE: usize = 8;

pub fn is_image_file(path: &std::path::Path) -> bool {
	path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Shrink a picture to a size models take (2048 px on the long edge), encode
/// it (PNG; JPEG when that is much smaller, as for photos) and keep it in the
/// app's folder, where the saved chat points to it.
fn store_chat_image(img: image::DynamicImage, name: &str) -> Result<UserImage, String> {
	use std::sync::atomic::{AtomicU32, Ordering};
	static N: AtomicU32 = AtomicU32::new(0);
	const MAX_EDGE: u32 = 2048;
	if img.width() == 0 || img.height() == 0 {
		return Err("the picture is empty".into());
	}
	let img = if img.width().max(img.height()) > MAX_EDGE { img.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Lanczos3) } else { img };
	let mut png = Vec::new();
	img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
	let (bytes, ext) = if png.len() > 1_500_000 {
		let mut jpg = Vec::new();
		image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 88).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
		if jpg.len() < png.len() { (jpg, "jpg") } else { (png, "png") }
	} else {
		(png, "png")
	};
	let dir = config_dir().join("chat-images");
	std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
	let path = dir.join(format!("{}-{}.{ext}", takeoff_core::now_ms(), N.fetch_add(1, Ordering::Relaxed)));
	std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
	Ok(UserImage { path, name: name.to_string() })
}

/// Thumbnails of a message's pictures; click one to open it. With
/// `removable`, each has an x and the index of the one clicked is returned.
fn image_strip(ui: &mut egui::Ui, p: &Palette, images: &[UserImage], thumbs: &mut HashMap<String, TextureHandle>, removable: bool) -> Option<usize> {
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

fn chip(ui: &mut egui::Ui, p: &Palette, text: &str) -> egui::Response {
	egui::Frame::new()
		.fill(p.hover)
		.corner_radius(10)
		.inner_margin(egui::Margin::symmetric(8, 2))
		.show(ui, |ui| {
			ui.add(egui::Label::new(RichText::new(text).size(11.5).color(p.text2)).truncate());
		})
		.response
}

fn user_bubble(ui: &mut egui::Ui, p: &Palette, text: &str) {
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
	} else if model.starts_with("opencode:") {
		(Color32::from_rgb(0xD9, 0x9A, 0x06), i::KEYBOARD)
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

/// The construction.live group header, with a refresh button when signed in.
/// Returns true when refresh was clicked.
fn cloud_header(ui: &mut egui::Ui, p: &Palette, refreshing: bool, signed_in: bool) -> bool {
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

#[cfg(test)]
mod tests {
	use super::*;

	/// A long user message must wrap inside the panel, and must not shift the
	/// transcript that follows it off the panel's left edge.
	/// Render `text` as markdown in a `width`-wide column; the space it used.
	fn markdown_size(text: &str, width: f32) -> egui::Rect {
		let ctx = egui::Context::default();
		theme::install_fonts(&ctx);
		let p = theme::palette(&ctx);
		let col = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 4000.0));
		let mut used = egui::Rect::NOTHING;
		let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 4000.0))), ..Default::default() }, |ui| {
			let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(col).layout(egui::Layout::top_down(egui::Align::Min)));
			markdown(&mut ui, &p, text);
			used = ui.min_rect();
		});
		used
	}

	#[test]
	fn blank_line_runs_take_one_gap_and_prose_fits_the_width() {
		let one = markdown_size("Starting with page 44.\n\nPage 45 next.", 300.0);
		let many = markdown_size(&format!("Starting with page 44.{}Page 45 next.", "\n\n".repeat(200)), 300.0);
		assert!((many.height() - one.height()).abs() < 1.0, "200 tool-step breaks add no extra gap: {} vs {}", many.height(), one.height());
		let prose = "1. Mainline Proposed Tracks 1, 2, 3 on RP-03 to RP-23 (pages 44–64)\n- **Track 1** runs the full sheet (144+00→159+00); Proposed Tracks 2 and 3 begin at STA 156+11. Measuring those and loading page 46 at the same time.\n### Scope 3 it is — all proposed tracks, in feet";
		let r = markdown_size(prose, 300.0);
		assert!(r.width() <= 300.5, "prose wraps inside the column (no sideways scroll): {}", r.width());
	}

	#[test]
	fn wrapped_lines_are_evenly_spaced_and_code_scrolls_in_its_own_block() {
		let para = "I haven't marked or measured anything yet. If you want a takeoff, tell me what to count or measure (for example shoring towers, post shores, or beam lengths) and I'll set the scale first.";
		// The same text as one plain wrapped label: no extra gap after the first row.
		let ctx = egui::Context::default();
		theme::install_fonts(&ctx);
		let mut plain = 0.0;
		let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 4000.0))), ..Default::default() }, |ui| {
			let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 4000.0))));
			plain = ui.add(egui::Label::new(RichText::new(para).size(13.0)).wrap()).rect.height();
		});
		let md = markdown_size(para, 300.0).height();
		assert!(plain > 40.0, "it wraps onto several lines: {plain}");
		assert!((md - plain).abs() < 1.0, "markdown {md} vs plain label {plain}");
		let code = format!("Here:\n```\n{}\n```\nDone.", "x".repeat(400));
		assert!(markdown_size(&code, 300.0).width() <= 300.5, "a long code line scrolls inside its block");
	}

	/// Visual check: `PREVIEW_PNG=/path.png cargo test -p takeoff-app --lib chat_preview -- --ignored`
	#[test]
	#[ignore]
	fn chat_preview() {
		let mut fonts = false;
		let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(380.0, 420.0)).build_ui(move |ui| {
			// Fonts apply from the next frame.
			if !fonts {
				theme::install_fonts(ui.ctx());
				fonts = true;
				return;
			}
			let p = theme::palette(ui.ctx());
			egui::Frame::new().fill(p.panel).inner_margin(12).show(ui, |ui| {
				markdown(ui, &p, "I haven't marked or measured anything yet. If you want a takeoff, tell me what to count or measure (for example shoring towers, post shores, or beam lengths) and I'll set the scale first.");
				user_bubble(ui, &p, "can you draw 3d html\n, so i can understand what what is suspended slab and how shoring works");
				markdown(ui, &p, "### Shoring\n- **Post shores** under the slab edge, spaced at `1.5x2.4`\n- Towers at the cores\n\n| Item | Qty |\n|---|---|\n| 1.5x2.4 Tower with a very long description that does not fit | 12 |");
			});
		});
		h.run();
		let img = h.render().expect("render");
		img.save(std::env::var("PREVIEW_PNG").unwrap_or_else(|_| "/tmp/chat_preview.png".into())).unwrap();
	}

	#[test]
	fn long_user_bubble_stays_inside_the_panel() {
		let ctx = egui::Context::default();
		let p = theme::palette(&ctx);
		let long = "can check the scale on other drawings and see how that converts to the pixel measurement on this one, then use it here ".repeat(3);
		let panel = egui::Rect::from_min_size(egui::pos2(100.0, 0.0), egui::vec2(300.0, 800.0));
		let mut after = egui::Rect::NOTHING;
		let mut used = egui::Rect::NOTHING;
		let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 800.0))), ..Default::default() }, |ui| {
			let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(panel).layout(egui::Layout::top_down(egui::Align::Min)));
			user_bubble(&mut ui, &p, &long);
			after = ui.label("next line").rect;
			used = ui.min_rect();
		});
		assert!(used.left() >= panel.left() - 0.5 && used.right() <= panel.right() + 0.5, "bubble overflowed: {used:?} vs {panel:?}");
		assert!((after.left() - panel.left()).abs() < 0.5, "following text shifted to x={}", after.left());
	}
}
