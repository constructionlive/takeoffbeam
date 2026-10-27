//! Takeoff Desk's agentic harness: an LLM loop with tools that see, count,
//! mark, measure and read the ONE drawing loaded in the viewer.
//!
//! - [`start_turn`] runs one user turn on a background thread and streams
//!   [`AgentEvent`]s; the [`ChatSession`] is updated as it goes.
//! - [`tools`] is the tool registry on its own (used by the MCP server).
//! - Any OpenAI-compatible endpoint works: llama-server, the construction.live
//!   desktop gateway, or a hosted API.
//! - [`channel`] runs the turn on the person's own Claude Code or Codex
//!   subscription instead, with the same tools served to it over MCP.
//! - [`eval`] scores the agent on drawings with known answers.

mod agent;
pub mod channel;
pub mod eval;
pub mod llm;
pub mod mcp;
pub mod prompt;
pub mod session;
pub mod tools;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

pub use agent::{start_turn, MAX_STEPS};
pub use session::{ChatSession, ExternalThread, Role, TranscriptEntry, UserImage, ViewAttachment};

/// Which model to talk to.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderConfig {
	/// OpenAI-compatible base, e.g. `http://127.0.0.1:8080/v1` or `https://x.convex.site/desktop/v1`.
	pub base_url: String,
	pub api_key: Option<String>,
	pub model: String,
	pub context_tokens: u32,
	pub vision: bool,
}

/// What the agent works on, and what the user is looking at right now.
pub struct AgentContext {
	pub doc: Arc<Mutex<takeoff_core::Document>>,
	pub pdf_path: PathBuf,
	pub current_page: u32,
	/// Visible part of the current page, in page fractions (x, y, w, h).
	pub visible_region: Option<(f64, f64, f64, f64)>,
	pub selected_ids: Vec<String>,
	pub provider: ProviderConfig,
	/// The agent's workspace folder (bash runs there); None = no shell.
	pub workspace: Option<PathBuf>,
	/// The estimator's other open drawings, which the agent can switch to.
	pub others: Vec<tools::OtherDrawing>,
	/// Areas attached to this message.
	pub attachments: Vec<ViewAttachment>,
	/// Pictures the estimator added to this message.
	pub images: Vec<UserImage>,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
	TextDelta(String),
	ReasoningDelta(String),
	ToolStarted { call_id: String, name: String, args: serde_json::Value },
	ToolFinished { call_id: String, name: String, ok: bool, summary: String, image: Option<image::RgbaImage> },
	/// Where the agent is looking or working (page fractions) — the viewer's ghost frame.
	/// `drawing`: which open drawing (None = the chat's own).
	Viewport { page: u32, region: (f64, f64, f64, f64), label: String, drawing: Option<PathBuf> },
	/// The takeoff changed (repaint, refresh lists).
	DocChanged,
	Usage { prompt_tokens: u64, completion_tokens: u64 },
	Error(String),
	Done { steps: u32 },
}

pub struct TurnHandle {
	pub events: crossbeam_channel::Receiver<AgentEvent>,
	cancel: Arc<AtomicBool>,
}

impl TurnHandle {
	/// Stop after the current chunk / tool; the session stays consistent.
	pub fn cancel(&self) {
		self.cancel.store(true, Ordering::Relaxed);
	}
	pub fn is_cancelled(&self) -> bool {
		self.cancel.load(Ordering::Relaxed)
	}
}

static LOCAL_THINKING: AtomicBool = AtomicBool::new(false);

/// Let local models (llama-server: Qwen3, Gemma…) "think" before each step.
/// Off by default: on a laptop a 27B model thinks at ~8 tokens/s, so one
/// step can take minutes. Cloud models are never affected.
pub fn set_local_thinking(enabled: bool) {
	LOCAL_THINKING.store(enabled, Ordering::Relaxed);
}

pub fn local_thinking() -> bool {
	LOCAL_THINKING.load(Ordering::Relaxed)
}

/// llama-server, LM Studio, Ollama… on this machine.
pub fn is_local_endpoint(base_url: &str) -> bool {
	let u = base_url.to_lowercase();
	u.contains("127.0.0.1") || u.contains("localhost") || u.contains("0.0.0.0") || u.contains("[::1]")
}

/// Model ids a server offers (`GET {base}/models`).
pub fn probe_models(provider_base_url: &str, api_key: Option<&str>) -> Result<Vec<String>, String> {
	llm::models(provider_base_url, api_key)
}
