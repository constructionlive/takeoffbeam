//! The AI tab: pick a model (local llama.cpp, construction.live cloud, the
//! estimator's own Claude Code / Codex subscription, or any OpenAI-compatible
//! endpoint), chat with the drawing agent, watch its steps.
//! Also the Models window: installed models with a memory-fit verdict,
//! downloads, sign-in, subscriptions, custom endpoint.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

mod chat;
mod models;
pub use chat::*;
use models::*;

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

	fn session(&mut self, path: &Path) -> Arc<Mutex<ChatSession>> {
		self.sessions
			.entry(path.to_path_buf())
			.or_insert_with(|| {
				let s = std::fs::read(chat_path(path)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
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

	#[doc(hidden)]
	pub fn debug_session(&mut self, path: &Path) -> Arc<Mutex<ChatSession>> {
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
