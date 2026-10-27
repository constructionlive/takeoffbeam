//! App preferences, persisted as JSON in the OS config dir.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use takeoff_core::Category;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
	#[default]
	System,
	Light,
	Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
	pub author: String,
	pub theme: ThemeChoice,
	/// Mouse wheel zooms (Bluebeam default) instead of scrolling.
	pub wheel_zooms: bool,
	pub recent: Vec<PathBuf>,
	pub snap: bool,
	pub show_badges: bool,
	/// "local:<id>", "cloud:<id>" or "custom".
	pub model: String,
	pub custom_base_url: String,
	pub custom_api_key: String,
	pub custom_model: String,
	pub cloud_url: String,
	pub llama_server: String,
	pub context_tokens: u32,
	pub kv_q8: bool,
	/// Let local models reason before answering (better, much slower).
	pub local_thinking: bool,
	/// Saved takeoff items (Bluebeam Tool Chest), reusable across drawings.
	pub tool_chest: Vec<Category>,
	pub left_panel: bool,
	pub right_panel: bool,
	pub markups_list: bool,
	pub onboarded: bool,
	/// "Skip this version" for update notices.
	pub skipped_update: String,
	/// Let the AI run shell commands and keep files in one folder.
	pub ai_workspace_on: bool,
	/// That folder; empty = [`default_workspace`].
	pub ai_workspace: String,
	/// The canvas follows the page and area the AI is looking at.
	pub follow_agent: bool,
	/// Callouts the AI mapped are links on the sheet.
	pub map_links: bool,
	/// Single page, continuous, two-page spreads (View → Page layout).
	pub page_layout: crate::layout::PageLayout,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			author: whoami(),
			theme: ThemeChoice::System,
			wheel_zooms: true,
			recent: vec![],
			snap: true,
			show_badges: true,
			model: String::new(),
			custom_base_url: String::new(),
			custom_api_key: String::new(),
			custom_model: String::new(),
			cloud_url: String::new(),
			llama_server: String::new(),
			context_tokens: 32_768,
			kv_q8: true,
			local_thinking: false,
			tool_chest: vec![],
			left_panel: true,
			right_panel: true,
			markups_list: true,
			onboarded: false,
			skipped_update: String::new(),
			ai_workspace_on: true,
			ai_workspace: String::new(),
			follow_agent: false,
			map_links: true,
			page_layout: Default::default(),
		}
	}
}

fn whoami() -> String {
	std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "Me".into())
}

/// Where the AI's workspace is unless the person picks another folder.
pub fn default_workspace() -> PathBuf {
	dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("Takeoff Desk").join("Workspace")
}

impl Settings {
	/// The AI's workspace folder, or None when it is turned off.
	pub fn workspace(&self) -> Option<PathBuf> {
		if !self.ai_workspace_on {
			return None;
		}
		Some(if self.ai_workspace.trim().is_empty() { default_workspace() } else { PathBuf::from(self.ai_workspace.trim()) })
	}
}

pub fn config_dir() -> PathBuf {
	let d = dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Takeoff Desk");
	let _ = std::fs::create_dir_all(&d);
	d
}

impl Settings {
	fn path() -> PathBuf {
		config_dir().join("settings.json")
	}
	pub fn load() -> Self {
		std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
	}
	pub fn save(&self) {
		if let Ok(json) = serde_json::to_vec_pretty(self) {
			let tmp = Self::path().with_extension("json.tmp");
			if std::fs::write(&tmp, json).is_ok() {
				let _ = std::fs::rename(tmp, Self::path());
			}
		}
	}
	pub fn push_recent(&mut self, p: PathBuf) {
		self.recent.retain(|x| x != &p);
		self.recent.insert(0, p);
		self.recent.truncate(12);
	}
}
