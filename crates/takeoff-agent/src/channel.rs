//! Subscription channels: run a Bench turn on the estimator's own Claude Code
//! or Codex CLI (their Claude / ChatGPT plan) instead of an API endpoint.
//!
//! The CLI runs headless (`claude -p --output-format stream-json`, `codex exec
//! --json`) in a scratch folder, with the Bench instructions in front of the
//! chat, and gets the drawing tools from the app over MCP: the CLI launches
//! this app's own executable with [`BRIDGE_ARG`], which pipes its stdio to a
//! [`ToolHost`] listening on localhost inside the app. Tools run against the
//! live document, so marks appear as they are placed, exactly as with the
//! built-in agent, and the turn streams the same [`AgentEvent`]s.
//!
//! No keys or OAuth here: the CLIs use whatever sign-in they already have.

use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Sender};
use parking_lot::Mutex;
use serde_json::{json, Value};
use takeoff_core::Document;

use crate::agent::{clip_tool_text, thumb, viewer_line};
use crate::llm::ToolCall;
use crate::prompt::system_prompt;
use crate::session::{ChatSession, ExternalThread, Msg};
use crate::tools::{self, ToolCtx};
use crate::{mcp, AgentEvent, ProviderConfig, TurnHandle};

mod host;
mod parse;
pub use host::*;
use parse::*;

/// `takeoff-desk --mcp-bridge <addr>`: the stdio MCP server the CLIs launch.
pub const BRIDGE_ARG: &str = "--mcp-bridge";
const TOKEN_ENV: &str = "TAKEOFF_BRIDGE_TOKEN";
/// MCP server name the CLIs see (Claude Code calls the tools `mcp__takeoff__view` …).
const SERVER: &str = "takeoff";
/// Sweeps that make their own vision calls to an OpenAI-compatible model.
const SWEEPS: [&str; 3] = ["detect", "calibrate", "trace"];
/// Sweeps tile a whole sheet; the CLIs' default MCP timeouts are far shorter.
const TOOL_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
	ClaudeCode,
	Codex,
	/// OpenCode: free models (DeepSeek and others) or the person's own provider keys.
	OpenCode,
}

impl Channel {
	pub const ALL: [Channel; 3] = [Channel::ClaudeCode, Channel::Codex, Channel::OpenCode];

	/// Stored in settings (`claude:<model>`) and in [`ExternalThread::channel`].
	pub fn key(self) -> &'static str {
		match self {
			Channel::ClaudeCode => "claude",
			Channel::Codex => "codex",
			Channel::OpenCode => "opencode",
		}
	}

	pub fn from_key(k: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|c| c.key() == k)
	}

	pub fn name(self) -> &'static str {
		match self {
			Channel::ClaudeCode => "Claude Code",
			Channel::Codex => "Codex",
			Channel::OpenCode => "OpenCode",
		}
	}

	pub fn plan(self) -> &'static str {
		match self {
			Channel::ClaudeCode => "Claude Pro, Max, Team or Enterprise",
			Channel::Codex => "ChatGPT Plus, Pro, Business or Enterprise",
			Channel::OpenCode => "Free models, or your own provider keys",
		}
	}

	pub fn install_command(self) -> &'static str {
		match self {
			Channel::ClaudeCode if cfg!(windows) => "irm https://claude.ai/install.ps1 | iex",
			Channel::ClaudeCode => "curl -fsSL https://claude.ai/install.sh | bash",
			Channel::Codex => "npm install -g @openai/codex",
			Channel::OpenCode if cfg!(windows) => "npm install -g opencode-ai",
			Channel::OpenCode => "curl -fsSL https://opencode.ai/install | bash",
		}
	}

	/// Installing it needs Node.js (npm) first.
	pub fn needs_node(self) -> bool {
		self == Channel::Codex || (self == Channel::OpenCode && cfg!(windows))
	}

	/// Oldest release with everything Takeoff Desk uses (`--tools`, `--mcp-config`,
	/// `dontAsk`, `auth status --json`… / `exec --json`, `workspace-write`, `debug models`).
	pub fn min_version(self) -> &'static str {
		match self {
			Channel::ClaudeCode => "2.1.40",
			Channel::Codex => "0.140.0",
			Channel::OpenCode => "1.18.33",
		}
	}

	pub fn update_command(self) -> &'static str {
		match self {
			Channel::ClaudeCode => "claude update",
			Channel::Codex => "npm install -g @openai/codex@latest",
			Channel::OpenCode => "opencode upgrade",
		}
	}

	pub fn login_command(self) -> &'static str {
		match self {
			Channel::ClaudeCode => "claude auth login",
			Channel::Codex => "codex login",
			Channel::OpenCode => "opencode auth login",
		}
	}

	fn exe_name(self) -> &'static str {
		self.key()
	}
}

// ── Finding and running the CLIs ────────────────────────────────────────

/// Where CLIs get installed. Apps opened from the Finder / Start menu don't
/// get the shell's PATH, so look in the usual places too.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
	let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
	if let Some(home) = dirs::home_dir() {
		for d in [".local/bin", ".opencode/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin", ".cargo/bin", "bin"] {
			dirs.push(home.join(d));
		}
		if let Ok(rd) = std::fs::read_dir(home.join(".nvm/versions/node")) {
			let mut nvm: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("bin")).collect();
			nvm.sort();
			dirs.extend(nvm.into_iter().rev());
		}
	}
	if cfg!(windows) {
		for var in ["APPDATA", "LOCALAPPDATA"] {
			if let Some(d) = std::env::var_os(var) {
				dirs.push(PathBuf::from(&d).join("npm"));
			}
		}
	} else {
		dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
	}
	let mut seen = HashSet::new();
	dirs.retain(|d| seen.insert(d.clone()));
	dirs
}

/// The CLI's executable, if it is installed.
pub fn find_exe(channel: Channel) -> Option<PathBuf> {
	let name = channel.exe_name();
	let names: Vec<String> = if cfg!(windows) { vec![format!("{name}.exe"), format!("{name}.cmd")] } else { vec![name.to_string()] };
	// The first one that actually runs: wrapper scripts and stale shims that
	// shadow the real binary on PATH are skipped.
	search_dirs()
		.into_iter()
		.flat_map(|d| names.iter().map(move |n| d.join(n)).collect::<Vec<_>>())
		.chain(bundled(channel))
		.filter(|p| p.is_file())
		.find(|p| runs(p))
}

fn runs(exe: &Path) -> bool {
	let mut cmd = command(exe);
	cmd.arg("--version");
	run(cmd, "", Duration::from_secs(8)).is_ok_and(|r| r.ok && !r.stdout.trim().is_empty())
}

/// CLIs that ship inside desktop apps (the ChatGPT / Codex apps bundle `codex`)
/// for people who never installed them on their own.
fn bundled(channel: Channel) -> Vec<PathBuf> {
	let mut out = Vec::new();
	if cfg!(target_os = "macos") {
		let apps: &[&str] = match channel {
			Channel::Codex => &["ChatGPT.app/Contents/Resources/codex", "Codex.app/Contents/Resources/codex"],
			Channel::ClaudeCode => &["Claude.app/Contents/Resources/claude"],
			Channel::OpenCode => &[],
		};
		for root in [PathBuf::from("/Applications"), dirs::home_dir().unwrap_or_default().join("Applications")] {
			out.extend(apps.iter().map(|a| root.join(a)));
		}
	}
	out
}

/// A command for a CLI: no console window, and a PATH that also finds the
/// `node` an npm-installed CLI needs.
fn command(exe: &Path) -> Command {
	#[allow(unused_mut)]
	let mut cmd = Command::new(exe);
	let mut path: Vec<PathBuf> = exe.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default();
	path.extend(search_dirs());
	if let Ok(p) = std::env::join_paths(path) {
		cmd.env("PATH", p);
	}
	// Started from a Claude Code terminal, these make the CLI refuse to run
	// ("cannot be launched inside another Claude Code session").
	cmd.env_remove("CLAUDECODE").env_remove("CLAUDE_CODE_ENTRYPOINT");
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		const CREATE_NO_WINDOW: u32 = 0x0800_0000;
		cmd.creation_flags(CREATE_NO_WINDOW);
	}
	cmd
}

struct Ran {
	ok: bool,
	stdout: String,
	stderr: String,
}

/// Run to completion with `stdin` fed in; killed after `timeout`.
fn run(mut cmd: Command, stdin: &str, timeout: Duration) -> Result<Ran, String> {
	let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
	if let Some(mut si) = child.stdin.take() {
		let _ = si.write_all(stdin.as_bytes());
	}
	let read = |r: Option<Box<dyn Read + Send>>| {
		std::thread::spawn(move || {
			let mut s = String::new();
			if let Some(mut r) = r {
				let _ = r.read_to_string(&mut s);
			}
			s
		})
	};
	let out = read(child.stdout.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
	let err = read(child.stderr.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
	let start = Instant::now();
	let status = loop {
		if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
			break s;
		}
		if start.elapsed() > timeout {
			let _ = child.kill();
			let _ = child.wait();
			return Err(format!("timed out after {}s", timeout.as_secs()));
		}
		std::thread::sleep(Duration::from_millis(50));
	};
	Ok(Ran { ok: status.success(), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

fn tail(s: &str, n: usize) -> String {
	let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
	lines[lines.len().saturating_sub(n)..].join("\n")
}

// ── Status and models ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelStatus {
	pub exe: Option<PathBuf>,
	pub version: Option<String>,
	/// None when it couldn't be told.
	pub signed_in: Option<bool>,
	/// "Max plan", "ChatGPT", "API key"…
	pub account: Option<String>,
	pub error: Option<String>,
	/// Older than [`Channel::min_version`]: it must be updated before use.
	pub outdated: bool,
}

/// The first `x.y.z` in a version line ("2.1.300 (Claude Code)", "codex-cli 0.162.1").
pub fn parse_version(line: &str) -> Option<(u64, u64, u64)> {
	line.split(|c: char| !(c.is_ascii_digit() || c == '.')).find_map(|w| {
		let mut it = w.split('.').map(|n| n.parse::<u64>().ok());
		Some((it.next()??, it.next()??, it.next()??))
	})
}

/// Whether a `--version` line is older than the channel's minimum.
pub fn too_old(channel: Channel, version_line: &str) -> bool {
	match (parse_version(version_line), parse_version(channel.min_version())) {
		(Some(v), Some(min)) => v < min,
		_ => false,
	}
}

/// "Update Claude Code" message for a CLI that is too old.
pub fn outdated_message(channel: Channel, version_line: &str) -> String {
	let v = parse_version(version_line).map(|(a, b, c)| format!("{a}.{b}.{c}")).unwrap_or_else(|| version_line.trim().to_string());
	format!(
		"{} {v} is too old for Takeoff Desk (it needs {} or newer). Update it under Models → Subscriptions, or run `{}` in a terminal.",
		channel.name(),
		channel.min_version(),
		channel.update_command()
	)
}

/// The installed CLI's version line, cached per executable (path + modified time).
fn installed_version(exe: &Path) -> Option<String> {
	use std::collections::HashMap;
	use std::sync::OnceLock;
	static CACHE: OnceLock<Mutex<HashMap<(PathBuf, Option<std::time::SystemTime>), String>>> = OnceLock::new();
	let key = (exe.to_path_buf(), std::fs::metadata(exe).and_then(|m| m.modified()).ok());
	if let Some(v) = CACHE.get_or_init(Default::default).lock().get(&key) {
		return Some(v.clone());
	}
	let mut c = command(exe);
	c.arg("--version");
	let r = run(c, "", Duration::from_secs(20)).ok().filter(|r| r.ok)?;
	let line = r.stdout.lines().next()?.trim().to_string();
	CACHE.get_or_init(Default::default).lock().insert(key, line.clone());
	Some(line)
}

/// npm is on this computer (Codex installs with it).
pub fn npm_available() -> bool {
	search_dirs().into_iter().any(|d| d.join(if cfg!(windows) { "npm.cmd" } else { "npm" }).is_file())
}

/// Run `command` in a new, visible terminal window, so installers and sign-in
/// flows can show progress, open the browser and ask questions.
pub fn open_in_terminal(command: &str) -> Result<(), String> {
	if cfg!(target_os = "macos") {
		// A .command file opens in Terminal and runs: no permission to script
		// Terminal is needed (a signed, hardened app would need one for AppleScript).
		let file = std::env::temp_dir().join(format!("takeoff-desk-{}.command", random_token()));
		std::fs::write(&file, format!("#!/bin/bash -l\n{command}\necho\necho 'Done. You can close this window and go back to Takeoff Desk.'\n")).map_err(|e| e.to_string())?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
		}
		Command::new("open").args(["-a", "Terminal"]).arg(&file).spawn().map_err(|e| e.to_string())?;
	} else if cfg!(windows) {
		Command::new("cmd").args(["/C", "start", "Takeoff Desk", "powershell", "-NoExit", "-Command", command]).spawn().map_err(|e| e.to_string())?;
	} else {
		let term = ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"]
			.into_iter()
			.find(|t| search_dirs().iter().any(|d| d.join(t).is_file()))
			.ok_or("no terminal app found — run the command yourself")?;
		let mut c = Command::new(term);
		if term == "gnome-terminal" {
			c.args(["--", "bash", "-lc"]);
		} else {
			c.args(["-e", "bash", "-lc"]);
		}
		c.arg(format!("{command}; exec bash")).spawn().map_err(|e| e.to_string())?;
	}
	Ok(())
}

/// The sign-in command for an installed CLI, with its full path (the
/// terminal's PATH may not include where it was found).
pub fn login_command_for(channel: Channel, exe: &Path) -> String {
	let quoted = if cfg!(windows) { format!("& \"{}\"", exe.display()) } else { format!("'{}'", exe.display().to_string().replace('\'', "'\\''")) };
	match channel {
		Channel::ClaudeCode => format!("{quoted} auth login"),
		Channel::Codex => format!("{quoted} login"),
		Channel::OpenCode => format!("{quoted} auth login"),
	}
}

/// Update the CLI in place (`claude update` / npm). Returns its output.
pub fn update(channel: Channel, exe: &Path) -> Result<String, String> {
	let cmd = match channel {
		Channel::ClaudeCode => {
			let mut c = command(exe);
			c.arg("update");
			c
		}
		Channel::OpenCode => {
			let mut c = command(exe);
			c.arg("upgrade");
			c
		}
		Channel::Codex => {
			if exe.components().any(|c| c.as_os_str().to_string_lossy().ends_with(".app")) {
				return Err("This Codex ships inside the ChatGPT app: update the ChatGPT app, or install the standalone CLI with `npm install -g @openai/codex@latest`.".into());
			}
			let npm = search_dirs().into_iter().map(|d| d.join(if cfg!(windows) { "npm.cmd" } else { "npm" })).find(|p| p.is_file()).ok_or("npm not found: install Node.js, or update Codex the way you installed it.")?;
			let mut c = command(&npm);
			c.args(["install", "-g", "@openai/codex@latest"]);
			c
		}
	};
	let r = run(cmd, "", Duration::from_secs(300))?;
	let out = tail(&format!("{}\n{}", r.stdout, r.stderr), 6);
	if r.ok { Ok(out) } else { Err(out) }
}

/// Installed? Which version? Signed in, and with what? Runs the CLI (a few seconds at most).
pub fn status(channel: Channel) -> ChannelStatus {
	let Some(exe) = find_exe(channel) else { return ChannelStatus::default() };
	let mut st = ChannelStatus { exe: Some(exe.clone()), ..Default::default() };
	let mut v = command(&exe);
	v.arg("--version");
	match run(v, "", Duration::from_secs(20)) {
		Ok(r) if r.ok => st.version = r.stdout.lines().next().map(|l| l.trim().to_string()),
		Ok(r) => st.error = Some(tail(&format!("{}\n{}", r.stdout, r.stderr), 3)),
		Err(e) => st.error = Some(e),
	}
	if st.version.as_deref().is_some_and(|v| too_old(channel, v)) {
		// Its sign-in check may not even exist yet; updating comes first.
		st.outdated = true;
		return st;
	}
	let mut a = command(&exe);
	match channel {
		Channel::ClaudeCode => {
			a.args(["auth", "status", "--json"]);
			if let Ok(r) = run(a, "", Duration::from_secs(20)) {
				if let Ok(j) = serde_json::from_str::<Value>(&r.stdout) {
					st.signed_in = j.get("loggedIn").and_then(Value::as_bool);
					let plan = j.get("subscriptionType").and_then(Value::as_str).filter(|s| !s.is_empty());
					let method = j.get("authMethod").and_then(Value::as_str).unwrap_or("");
					st.account = match plan {
						Some(p) => {
							let mut c = p.chars();
							Some(format!("{}{} plan", c.next().map(|f| f.to_uppercase().to_string()).unwrap_or_default(), c.as_str()))
						}
						None if method.to_lowercase().contains("key") => Some("API key (billed per token)".into()),
						None => None,
					};
				}
			}
		}
		Channel::OpenCode => {
			// Its free models need no account; providers people add are optional.
			let _ = a;
			st.signed_in = Some(true);
			st.account = Some("free models".into());
		}
		Channel::Codex => {
			a.args(["login", "status"]);
			if let Ok(r) = run(a, "", Duration::from_secs(20)) {
				let text = format!("{}\n{}", r.stdout, r.stderr);
				st.signed_in = Some(r.ok && text.to_lowercase().contains("logged in"));
				if r.ok {
					st.account = text.lines().find(|l| l.to_lowercase().contains("logged in")).map(|l| l.trim().trim_start_matches("Logged in using ").trim_start_matches("Logged in with ").to_string());
				}
			}
		}
	}
	st
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelModel {
	/// What goes to `--model`; empty = the CLI's own default.
	pub id: String,
	pub label: String,
	pub note: String,
}

/// Models the CLI offers. Codex asks the account's catalog; Claude Code takes
/// aliases that always point at the newest model of each tier.
pub fn models(channel: Channel, exe: &Path) -> Result<Vec<ChannelModel>, String> {
	let m = |id: &str, label: &str, note: &str| ChannelModel { id: id.into(), label: label.into(), note: note.into() };
	match channel {
		Channel::ClaudeCode => Ok(vec![
			m("", "Default", "Whatever your plan uses by default"),
			m("fable", "Fable", "Largest model"),
			m("opus", "Opus", "Strongest for careful takeoffs"),
			m("sonnet", "Sonnet", "Fast and capable"),
			m("haiku", "Haiku", "Fastest, lightest"),
		]),
		Channel::OpenCode => {
			let mut c = command(exe);
			c.args(["models", "--verbose"]);
			let r = run(c, "", Duration::from_secs(60))?;
			if !r.ok {
				return Err(tail(&r.stderr, 3));
			}
			Ok(parse_opencode_models(&r.stdout))
		}
		Channel::Codex => {
			let mut c = command(exe);
			c.args(["debug", "models"]);
			let r = run(c, "", Duration::from_secs(45))?;
			if !r.ok {
				return Err(tail(&r.stderr, 3));
			}
			let v: Value = serde_json::from_str(&r.stdout).map_err(|e| format!("unexpected model list: {e}"))?;
			Ok(parse_codex_models(&v))
		}
	}
}

/// `opencode models --verbose`: "provider/id" then that model's JSON, for each.
/// Keeps the ones that can see images and call tools (the drawing tools need
/// both); free ones first.
fn parse_opencode_models(text: &str) -> Vec<ChannelModel> {
	let lines: Vec<&str> = text.lines().collect();
	let mut out: Vec<(bool, ChannelModel)> = Vec::new();
	let mut i = 0;
	while i < lines.len() {
		let id = lines[i].trim();
		if id.contains('/') && !id.contains(' ') && lines.get(i + 1).is_some_and(|l| l.trim() == "{") {
			// Collect the JSON object that follows (until its closing brace at column 0).
			let mut j = i + 1;
			let mut body = String::new();
			while j < lines.len() {
				body.push_str(lines[j]);
				body.push('\n');
				if lines[j] == "}" {
					break;
				}
				j += 1;
			}
			if let Ok(v) = serde_json::from_str::<Value>(&body) {
				let image = v.pointer("/capabilities/input/image").and_then(Value::as_bool).unwrap_or(false);
				let tools = v.pointer("/capabilities/toolcall").and_then(Value::as_bool).unwrap_or(false);
				let free = v.pointer("/cost/input").and_then(Value::as_f64) == Some(0.0) && v.pointer("/cost/output").and_then(Value::as_f64) == Some(0.0);
				if image && tools {
					let name = s(&v, "/name");
					let provider = id.split('/').next().unwrap_or("");
					let note = if free { "Free".to_string() } else { format!("via {provider}") };
					out.push((free, ChannelModel { id: id.to_string(), label: if name.is_empty() { id.to_string() } else { name.to_string() }, note }));
				}
			}
			i = j + 1;
			continue;
		}
		i += 1;
	}
	out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label.cmp(&b.1.label)));
	out.into_iter().map(|(_, m)| m).collect()
}

fn parse_codex_models(v: &Value) -> Vec<ChannelModel> {
	let mut list: Vec<(i64, ChannelModel)> = v
		.get("models")
		.and_then(Value::as_array)
		.into_iter()
		.flatten()
		.filter(|m| m.get("visibility").and_then(Value::as_str).is_none_or(|s| s == "list"))
		// The Bench works from renders: text-only models can't do takeoff.
		.filter(|m| m.get("input_modalities").and_then(Value::as_array).is_none_or(|a| a.iter().any(|x| x == "image")))
		.filter_map(|m| {
			let id = m.get("slug").and_then(Value::as_str)?.to_string();
			let label = m.get("display_name").and_then(Value::as_str).unwrap_or(&id).to_string();
			let note = m.get("description").and_then(Value::as_str).unwrap_or("").to_string();
			Some((m.get("priority").and_then(Value::as_i64).unwrap_or(999), ChannelModel { id, label, note }))
		})
		.collect();
	list.sort_by_key(|(p, _)| *p);
	let mut out = vec![ChannelModel { id: String::new(), label: "Default".into(), note: "The model set in your Codex config".into() }];
	out.extend(list.into_iter().map(|(_, m)| m));
	out
}

/// Send a one-line prompt to see whether `model` works on this account.
/// Ok carries what to show ("Works · claude-opus-5-5").
pub fn test_model(channel: Channel, exe: &Path, model: &str, workdir: &Path) -> Result<String, String> {
	let _ = std::fs::create_dir_all(workdir);
	let mut c = command(exe);
	c.current_dir(workdir);
	let prompt = "Reply with the single word OK.";
	match channel {
		Channel::ClaudeCode => {
			c.args(["-p", "--output-format", "json", "--no-session-persistence", "--tools", ""]);
			if !model.is_empty() {
				c.args(["--model", model]);
			}
			let r = run(c, prompt, Duration::from_secs(120))?;
			let v: Value = serde_json::from_str(r.stdout.trim()).map_err(|_| tail(&format!("{}\n{}", r.stdout, r.stderr), 3))?;
			if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
				return Err(v.get("result").and_then(Value::as_str).unwrap_or("error").to_string());
			}
			let used: Vec<String> = v.get("modelUsage").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default();
			Ok(if used.is_empty() { "Works".into() } else { format!("Works · {}", used.join(", ")) })
		}
		Channel::OpenCode => {
			c.args(["run", "--format", "json"]);
			if !model.is_empty() {
				c.args(["-m", model]);
			}
			let r = run(c, prompt, Duration::from_secs(180))?;
			let mut p = OpenCodeParser::default();
			let mut err = None;
			let mut text = false;
			for line in r.stdout.lines() {
				for ev in p.line(line) {
					match ev {
						Ev::Error(e) => err = Some(e),
						Ev::Text(_) => text = true,
						_ => {}
					}
				}
			}
			match (text, err) {
				(_, Some(e)) => Err(e),
				(true, None) => Ok("Works".into()),
				(false, None) => Err(tail(&format!("{}\n{}", r.stdout, r.stderr), 3)),
			}
		}
		Channel::Codex => {
			c.args(["exec", "--json", "--skip-git-repo-check", "--ephemeral", "-s", "read-only"]);
			if !model.is_empty() {
				c.args(["-m", model]);
			}
			c.arg("-");
			let r = run(c, prompt, Duration::from_secs(120))?;
			let mut p = CodexParser::default();
			let mut err = None;
			let mut done = false;
			for line in r.stdout.lines() {
				for ev in p.line(line) {
					match ev {
						Ev::Error(e) => err = Some(e),
						Ev::Done => done = true,
						_ => {}
					}
				}
			}
			match (done, err) {
				(_, Some(e)) => Err(e),
				(true, None) => Ok("Works".into()),
				(false, None) => Err(tail(&r.stderr, 3)),
			}
		}
	}
}

// ── Running a turn ─────────────────────────────────────────────────────

/// One Bench turn on a subscription channel.
pub struct ChannelTurn {
	pub channel: Channel,
	pub exe: PathBuf,
	/// Empty = the CLI's default.
	pub model: String,
	/// Where the CLI runs (an empty folder of the app's; Claude Code keys its
	/// sessions by folder, so keep it stable).
	pub workdir: PathBuf,
	/// The AI workspace: when set, the CLI runs THERE with its own shell and
	/// file tools turned on (instead of in `workdir` with none).
	pub workspace: Option<PathBuf>,
	/// The estimator's other open drawings.
	pub others: Vec<tools::OtherDrawing>,
	/// Areas attached to this message.
	pub attachments: Vec<crate::ViewAttachment>,
	/// Pictures the estimator added; the CLI opens them by path.
	pub images: Vec<crate::UserImage>,
	/// Model for detect / calibrate / trace, which make their own vision
	/// calls; None leaves them out of the tool list.
	pub sweeps: Option<ProviderConfig>,
	pub current_page: u32,
	pub visible_region: Option<(f64, f64, f64, f64)>,
	pub selected_ids: Vec<String>,
}

/// Claude Code's built-in tools allowed when the workspace is on.
const CLAUDE_WORKSPACE_TOOLS: &str = "Bash,Read,Write,Edit,Glob,Grep";

impl ChannelTurn {
	/// Where the CLI runs.
	fn cwd(&self) -> &Path {
		self.workspace.as_deref().unwrap_or(&self.workdir)
	}
}

pub fn start_turn(host: &ToolHost, session: Arc<Mutex<ChatSession>>, turn: ChannelTurn, user_text: String) -> TurnHandle {
	let (tx, rx) = unbounded();
	let cancel = Arc::new(AtomicBool::new(false));
	let c2 = cancel.clone();
	let shared = host.shared.clone();
	let addr = host.addr;
	std::thread::Builder::new()
		.name("takeoff-channel".into())
		.spawn(move || {
			let steps = run_turn(&shared, addr, &session, turn, user_text, &tx, &c2);
			shared.end();
			let _ = tx.send(AgentEvent::Done { steps });
		})
		.expect("spawn channel thread");
	TurnHandle { events: rx, cancel }
}

fn channel_note(channel: Channel, sweeps: bool, workspace: bool) -> String {
	let names = match channel {
		Channel::ClaudeCode => " (here they are named mcp__takeoff__view, mcp__takeoff__add_marks and so on)",
		Channel::Codex => "",
		Channel::OpenCode => " (here they are named takeoff_view, takeoff_add_marks and so on)",
	};
	let mut s = format!(
		"\n\n# This session\n\nYou are running in {} on the estimator's own account. Your drawing tools come from the MCP server \"{SERVER}\"{names}. {reach} Each message starts with a [Viewer: …] line saying what the estimator is looking at.",
		channel.name(),
		reach = if workspace {
			"Your own shell and file tools run in the workspace folder (your working directory); use them there, and use the drawing tools for everything on the drawing."
		} else {
			"Use only those tools: no shell, files or web."
		}
	);
	if channel == Channel::ClaudeCode {
		s.push_str(" Your Agent tool starts subagents that run in parallel and have the same drawing tools: use it for long jobs such as mapping a set (one subagent per batch of sheets), with a self-contained brief each.");
	}
	if !sweeps {
		s.push_str(" detect, calibrate and trace are not available in this session (they need a construction.live or local vision model selected in Takeoff Desk): find symbols yourself in zoomed views and place them with add_marks, region by region on busy sheets.");
	}
	s
}

/// The chat so far, for a CLI that is joining it fresh (another model ran
/// the earlier turns, or the old session could not be resumed).
fn history_preamble(session: &ChatSession) -> String {
	let msgs = &session.messages[..session.messages.len().saturating_sub(1)];
	if msgs.is_empty() && session.summary.is_none() {
		return String::new();
	}
	let mut lines = Vec::new();
	for m in msgs {
		match m {
			Msg::User { text, .. } => lines.push(format!("Estimator: {text}")),
			Msg::Assistant { text, .. } if !text.trim().is_empty() => lines.push(format!("Bench: {text}")),
			Msg::Tool { name, text, .. } => lines.push(format!("  ({name} → {})", text.lines().next().unwrap_or("").chars().take(160).collect::<String>())),
			_ => {}
		}
	}
	let mut body = lines.join("\n");
	const MAX: usize = 8000;
	if body.len() > MAX {
		let cut = body.char_indices().map(|(i, _)| i).find(|&i| i >= body.len() - MAX).unwrap_or(0);
		body = format!("…{}", &body[cut..]);
	}
	let summary = session.summary.as_ref().map(|s| format!("Summary of earlier turns:\n{s}\n\n")).unwrap_or_default();
	format!("<earlier_in_this_chat>\n{summary}{body}\n</earlier_in_this_chat>\n\n")
}

fn run_turn(shared: &Arc<Shared>, addr: SocketAddr, session: &Arc<Mutex<ChatSession>>, turn: ChannelTurn, user_text: String, tx: &Sender<AgentEvent>, cancel: &Arc<AtomicBool>) -> u32 {
	if let Some(v) = installed_version(&turn.exe).filter(|v| too_old(turn.channel, v)) {
		let _ = tx.send(AgentEvent::Error(outdated_message(turn.channel, &v)));
		return 0;
	}
	if turn.channel == Channel::OpenCode && turn.workspace.is_none() {
		let _ = tx.send(AgentEvent::Error(
			"OpenCode needs the AI workspace turned on: its free models only run with OpenCode's own tools, including its shell, so it works inside the workspace folder. Turn it on under Models (Let the AI run commands in a workspace folder).".into(),
		));
		return 0;
	}
	let drawing = match takeoff_pdf::Drawing::open(&shared.pdf) {
		Ok(d) => d,
		Err(e) => {
			let _ = tx.send(AgentEvent::Error(format!("could not open the drawing: {e}")));
			return 0;
		}
	};
	let other_names: Vec<String> = turn.others.iter().filter_map(|o| o.path.file_name()).map(|f| f.to_string_lossy().into_owned()).collect();
	let ctx_line = viewer_line(&shared.doc, &drawing, turn.current_page, turn.visible_region, &turn.selected_ids, &other_names);
	let file = shared.pdf.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
	let system = system_prompt(&file, drawing.page_count(), true, turn.workspace.as_deref()) + &channel_note(turn.channel, turn.sweeps.is_some(), turn.workspace.is_some());
	drop(drawing);
	// TAKEOFF_BRIDGE_EXE: a built takeoff-desk to bridge through (tests, dev runs from another binary).
	let exe = match std::env::var_os("TAKEOFF_BRIDGE_EXE").map(|p| Ok(PathBuf::from(p))).unwrap_or_else(std::env::current_exe) {
		Ok(e) => e,
		Err(e) => {
			let _ = tx.send(AgentEvent::Error(format!("could not find Takeoff Desk's own executable for the tool bridge: {e}")));
			return 0;
		}
	};
	let _ = std::fs::create_dir_all(turn.cwd());
	let ctx_line = ctx_line + &crate::session::attachments_note(&turn.attachments, &shared.pdf) + &crate::session::images_note(&turn.images, true, true);
	session.lock().messages.push(Msg::User { text: user_text.clone(), context: Some(ctx_line.clone()), attachments: turn.attachments.clone(), images: turn.images.clone() });
	let pending = Arc::new(Mutex::new(String::new()));
	let dialect_model = match (turn.model.as_str(), turn.channel) {
		("", Channel::ClaudeCode) => "claude".to_string(),
		("", Channel::Codex) => "gpt".to_string(),
		("", Channel::OpenCode) => "opencode".to_string(),
		(m, _) => m.to_string(),
	};
	shared.begin(Link { tx: tx.clone(), session: session.clone(), cancel: cancel.clone(), current_page: turn.current_page, model: dialect_model, sweeps: turn.sweeps.clone(), pending: pending.clone(), workspace: turn.workspace.clone(), others: turn.others.clone() });
	let bridge = Bridge { exe, addr: addr.to_string(), token: shared.token.clone() };
	let mut resume = session.lock().external.clone().filter(|e| e.channel == turn.channel.key()).map(|e| e.id);
	let mut steps = 0;
	for attempt in 0..2 {
		let preamble = if resume.is_none() { history_preamble(&session.lock()) } else { String::new() };
		let prompt = match (turn.channel, resume.is_some()) {
			(Channel::Codex | Channel::OpenCode, false) => format!("<takeoff_desk_instructions>\n{system}\n</takeoff_desk_instructions>\n\n{preamble}{ctx_line}\n\n{user_text}"),
			_ => format!("{preamble}{ctx_line}\n\n{user_text}"),
		};
		let out = run_cli(&turn, &bridge, &system, resume.as_deref(), &prompt, session, &pending, tx, cancel);
		steps += out.steps;
		if attempt == 0 && resume.is_some() && out.failed && !out.produced && !cancel.load(Ordering::Relaxed) {
			// The CLI lost the old conversation (cleared, other machine…): start
			// a new one carrying the chat so far.
			session.lock().external = None;
			resume = None;
			continue;
		}
		if let Some(e) = out.error {
			let _ = tx.send(AgentEvent::Error(e));
		}
		break;
	}
	let rest = std::mem::take(&mut *pending.lock());
	if !rest.trim().is_empty() {
		session.lock().messages.push(Msg::Assistant { text: rest, reasoning: String::new(), tool_calls: vec![] });
	}
	steps
}

struct Bridge {
	exe: PathBuf,
	addr: String,
	token: String,
}

/// A TOML basic string (JSON's escapes are valid TOML).
fn toml_str(s: &str) -> String {
	Value::String(s.to_string()).to_string()
}

fn cli_command(turn: &ChannelTurn, bridge: &Bridge, system: &str, resume: Option<&str>) -> Command {
	let mut c = command(&turn.exe);
	c.current_dir(turn.cwd());
	match turn.channel {
		Channel::OpenCode => {
			// OpenCode as it ships (its free tier refuses changed agents or tool
			// sets), plus our drawing tools as an MCP server, in the workspace.
			let cfg = json!({ "mcp": { SERVER: {
				"type": "local",
				"command": [bridge.exe, BRIDGE_ARG, bridge.addr],
				"environment": { TOKEN_ENV: bridge.token },
				"enabled": true,
				"timeout": 30000,
			}}});
			c.env("OPENCODE_CONFIG_CONTENT", cfg.to_string());
			c.args(["run", "--format", "json", "--auto"]);
			c.arg("--dir").arg(turn.cwd());
			if !turn.model.is_empty() {
				c.args(["-m", &turn.model]);
			}
			if let Some(id) = resume {
				c.args(["-s", id]);
			}
			let _ = system; // sent in the first message (see run_turn)
		}
		Channel::ClaudeCode => {
			let mcp = json!({ "mcpServers": { SERVER: {
				"type": "stdio",
				"command": bridge.exe,
				"args": [BRIDGE_ARG, bridge.addr],
				"env": { TOKEN_ENV: bridge.token },
			}}});
			c.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--input-format", "text"]);
			c.args(["--system-prompt", system]);
			c.args(["--mcp-config", &mcp.to_string(), "--strict-mcp-config"]);
			// Agent: its own subagents, which share the drawing tools (long jobs such as mapping a set).
			if turn.workspace.is_some() {
				// Shell and file tools in the workspace (the working directory), pre-approved with the drawing tools.
				c.args(["--tools", &format!("{CLAUDE_WORKSPACE_TOOLS},Agent")]);
				c.args(["--allowedTools", &format!("mcp__{SERVER},{CLAUDE_WORKSPACE_TOOLS},Agent")]);
			} else if !turn.images.is_empty() {
				// The drawing tools plus Read, so it can open the attached pictures.
				c.args(["--tools", "Read,Agent"]);
				c.args(["--allowedTools", &format!("mcp__{SERVER},Read,Agent")]);
			} else {
				// No shell, files or web: only the drawing tools and subagents, pre-approved.
				c.args(["--tools", "Agent"]);
				c.args(["--allowedTools", &format!("mcp__{SERVER},Agent")]);
			}
			// The folders holding the attached pictures, so Read may open them.
			let mut dirs: Vec<&Path> = turn.images.iter().filter_map(|im| im.path.parent()).collect();
			dirs.dedup();
			for d in dirs {
				c.arg("--add-dir").arg(d);
			}
			c.args(["--permission-mode", "dontAsk"]);
			if !turn.model.is_empty() {
				c.args(["--model", &turn.model]);
			}
			if let Some(id) = resume {
				c.args(["--resume", id]);
			}
			c.env("MCP_TOOL_TIMEOUT", (TOOL_TIMEOUT_SECS * 1000).to_string());
			c.env("MCP_TIMEOUT", "30000");
		}
		Channel::Codex => {
			c.arg("exec");
			c.args(["--json", "--skip-git-repo-check"]);
			if turn.workspace.is_some() {
				// Codex's own sandbox: writes confined to the workspace; network allowed.
				c.args(["-s", "workspace-write", "-c", "sandbox_workspace_write.network_access=true"]);
			} else {
				c.args(["-s", "read-only"]);
			}
			c.arg("-C").arg(turn.cwd());
			c.args(["-c", "approval_policy=\"never\""]);
			let key = format!("mcp_servers.{SERVER}");
			c.args(["-c", &format!("{key}.command={}", toml_str(&bridge.exe.to_string_lossy()))]);
			c.args(["-c", &format!("{key}.args=[{}, {}]", toml_str(BRIDGE_ARG), toml_str(&bridge.addr))]);
			c.args(["-c", &format!("{key}.env={{ {TOKEN_ENV} = {} }}", toml_str(&bridge.token))]);
			c.args(["-c", &format!("{key}.tool_timeout_sec={TOOL_TIMEOUT_SECS}")]);
			c.args(["-c", &format!("{key}.startup_timeout_sec=30")]);
			if !turn.model.is_empty() {
				c.args(["-m", &turn.model]);
			}
			if let Some(id) = resume {
				c.args(["resume", id]);
			}
			c.arg("-");
		}
	}
	c
}

#[derive(Default)]
struct CliOutcome {
	steps: u32,
	/// Any text or tool call came back.
	produced: bool,
	failed: bool,
	error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn run_cli(
	turn: &ChannelTurn,
	bridge: &Bridge,
	system: &str,
	resume: Option<&str>,
	prompt: &str,
	session: &Arc<Mutex<ChatSession>>,
	pending: &Arc<Mutex<String>>,
	tx: &Sender<AgentEvent>,
	cancel: &Arc<AtomicBool>,
) -> CliOutcome {
	let mut out = CliOutcome::default();
	let mut cmd = cli_command(turn, bridge, system, resume);
	let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
		Ok(c) => c,
		Err(e) => {
			out.failed = true;
			out.error = Some(format!("could not start {}: {e}", turn.channel.name()));
			return out;
		}
	};
	if let Some(mut si) = child.stdin.take() {
		let _ = si.write_all(prompt.as_bytes());
	}
	let stderr = Arc::new(Mutex::new(String::new()));
	if let Some(mut e) = child.stderr.take() {
		let buf = stderr.clone();
		std::thread::spawn(move || {
			let mut s = String::new();
			let _ = e.read_to_string(&mut s);
			*buf.lock() = s;
		});
	}
	let stdout = child.stdout.take();
	let child = Arc::new(Mutex::new(child));
	let finished = Arc::new(AtomicBool::new(false));
	{
		// Stop: kill the CLI (its bridge exits with it); the session stays as it was.
		let (child, finished, cancel) = (child.clone(), finished.clone(), cancel.clone());
		std::thread::spawn(move || {
			while !finished.load(Ordering::Relaxed) {
				if cancel.load(Ordering::Relaxed) {
					let _ = child.lock().kill();
					break;
				}
				std::thread::sleep(Duration::from_millis(150));
			}
		});
	}
	let mut parser: Box<dyn Parser> = match turn.channel {
		Channel::ClaudeCode => Box::new(ClaudeParser::default()),
		Channel::Codex => Box::new(CodexParser::default()),
		Channel::OpenCode => Box::new(OpenCodeParser::default()),
	};
	let mut done = false;
	let mut foreign = Vec::new();
	if let Some(so) = stdout {
		for line in BufReader::new(so).lines() {
			let Ok(line) = line else { break };
			for ev in parser.line(&line) {
				match ev {
					Ev::Text(t) => {
						out.produced = true;
						pending.lock().push_str(&t);
						let _ = tx.send(AgentEvent::TextDelta(t));
					}
					Ev::Reasoning(t) => {
						let _ = tx.send(AgentEvent::ReasoningDelta(t));
					}
					Ev::Thread(id) => session.lock().external = Some(ExternalThread { channel: turn.channel.key().into(), id }),
					Ev::OwnToolUse => {
						out.produced = true;
						out.steps += 1;
					}
					Ev::ToolStarted { id, name, args } => {
						out.produced = true;
						out.steps += 1;
						foreign.push((id.clone(), name.clone(), args.clone()));
						let _ = tx.send(AgentEvent::ToolStarted { call_id: id, name, args });
					}
					Ev::ToolFinished { id, ok, summary } => {
						let Some(pos) = foreign.iter().position(|f| f.0 == id) else { continue };
						let (id, name, args) = foreign.remove(pos);
						let text_before = std::mem::take(&mut *pending.lock());
						let mut s = session.lock();
						s.messages.push(Msg::Assistant { text: text_before, reasoning: String::new(), tool_calls: vec![ToolCall { id: id.clone(), name: name.clone(), arguments: args.to_string() }] });
						s.messages.push(Msg::Tool { call_id: id.clone(), name: name.clone(), text: summary.clone(), ok, images: vec![], image_captions: vec![], thumb: None });
						drop(s);
						let _ = tx.send(AgentEvent::ToolFinished { call_id: id, name, ok, summary, image: None });
					}
					Ev::Usage { prompt, completion } => {
						let mut s = session.lock();
						s.prompt_tokens = prompt.max(s.prompt_tokens);
						s.completion_tokens += completion;
						drop(s);
						let _ = tx.send(AgentEvent::Usage { prompt_tokens: prompt, completion_tokens: completion });
					}
					Ev::Error(e) => {
						out.failed = true;
						out.error = Some(e);
					}
					Ev::Done => done = true,
				}
			}
		}
	}
	finished.store(true, Ordering::Relaxed);
	let status = child.lock().wait();
	if cancel.load(Ordering::Relaxed) {
		out.error = None;
		return out;
	}
	if !done && out.error.is_none() {
		out.failed = true;
		let err = tail(&stderr.lock(), 4);
		let code = status.ok().and_then(|s| s.code()).map(|c| format!(" (exit code {c})")).unwrap_or_default();
		out.error = Some(if err.is_empty() { format!("{} stopped without finishing{code}", turn.channel.name()) } else { format!("{}: {err}", turn.channel.name()) });
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn feed(p: &mut dyn Parser, lines: &[&str]) -> Vec<Ev> {
		lines.iter().flat_map(|l| p.line(l)).collect()
	}

	#[test]
	fn claude_stream() {
		let mut p = ClaudeParser::default();
		let evs = feed(
			&mut p,
			&[
				r#"{"type":"system","subtype":"init","session_id":"abc-123","mcp_servers":[{"name":"takeoff","status":"connected"}]}"#,
				r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}}"#,
				r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Look first."}}}"#,
				r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#,
				r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Rendering the page."}}}"#,
				r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Rendering the page."},{"type":"tool_use","id":"t1","name":"mcp__takeoff__view","input":{"page":1}}]}}"#,
				r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"v1 · page 1"}]}]}}"#,
				r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}"#,
				r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"12 receptacles."}}}"#,
				r#"{"type":"result","subtype":"success","is_error":false,"result":"12 receptacles.","session_id":"abc-123","usage":{"input_tokens":10,"cache_read_input_tokens":900,"cache_creation_input_tokens":90,"output_tokens":40}}"#,
			],
		);
		assert_eq!(
			evs,
			vec![
				Ev::Thread("abc-123".into()),
				Ev::Reasoning("Look first.".into()),
				Ev::Text("Rendering the page.".into()),
				Ev::OwnToolUse,
				Ev::ToolFinished { id: "t1".into(), ok: true, summary: "v1 · page 1".into() },
				Ev::Text("\n\n".into()),
				Ev::Text("12 receptacles.".into()),
				Ev::Usage { prompt: 1000, completion: 40 },
				Ev::Done,
			]
		);
	}

	#[test]
	fn claude_errors() {
		let mut p = ClaudeParser::default();
		let evs = feed(&mut p, &[r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login","usage":{}}"#]);
		assert!(evs.contains(&Ev::Error("Claude Code: Not logged in · Please run /login".into())));
		assert_eq!(evs.last(), Some(&Ev::Done));
		let evs = feed(&mut p, &[r#"{"type":"system","subtype":"init","session_id":"x","mcp_servers":[{"name":"takeoff","status":"failed"}]}"#]);
		assert!(matches!(evs[1], Ev::Error(_)));
	}

	#[test]
	fn codex_stream() {
		let mut p = CodexParser::default();
		let evs = feed(
			&mut p,
			&[
				r#"{"type":"thread.started","thread_id":"0199-thread"}"#,
				r#"{"type":"turn.started"}"#,
				r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"Plan the sweep."}}"#,
				r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Looking at page 1."}}"#,
				r#"{"type":"item.started","item":{"id":"item_2","type":"mcp_tool_call","server":"takeoff","tool":"view","arguments":{"page":1},"status":"in_progress"}}"#,
				r#"{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"takeoff","tool":"view","status":"completed"}}"#,
				r#"{"type":"item.started","item":{"id":"item_3","type":"command_execution","command":"ls","status":"in_progress"}}"#,
				r#"{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"ls","aggregated_output":"","exit_code":0,"status":"completed"}}"#,
				r#"{"type":"item.updated","item":{"id":"item_4","type":"agent_message","text":"Found "}}"#,
				r#"{"type":"item.completed","item":{"id":"item_4","type":"agent_message","text":"Found 12."}}"#,
				r#"{"type":"turn.completed","usage":{"input_tokens":2400,"cached_input_tokens":2000,"output_tokens":75}}"#,
			],
		);
		assert_eq!(
			evs,
			vec![
				Ev::Thread("0199-thread".into()),
				Ev::Reasoning("Plan the sweep.".into()),
				Ev::Text("Looking at page 1.".into()),
				Ev::OwnToolUse,
				Ev::ToolStarted { id: "item_3".into(), name: "shell".into(), args: json!({"command": "ls"}) },
				Ev::ToolFinished { id: "item_3".into(), ok: true, summary: "completed".into() },
				Ev::Text("\n\nFound ".into()),
				Ev::Text("12.".into()),
				Ev::Usage { prompt: 2400, completion: 75 },
				Ev::Done,
			]
		);
	}

	#[test]
	fn codex_failure_keeps_the_reason() {
		let mut p = CodexParser::default();
		let evs = feed(&mut p, &[r#"{"type":"error","message":"stream disconnected"}"#, r#"{"type":"turn.failed","error":{"message":"The model `gpt-x` does not exist or you do not have access to it."}}"#]);
		assert_eq!(evs, vec![Ev::Error("Codex: The model `gpt-x` does not exist or you do not have access to it.".into()), Ev::Done]);
	}

	#[test]
	fn codex_models_keep_listed_vision_models_in_priority_order() {
		let v = json!({"models": [
			{"slug": "b", "display_name": "B", "visibility": "list", "priority": 2, "input_modalities": ["text", "image"]},
			{"slug": "hidden", "visibility": "hide", "priority": 0},
			{"slug": "text-only", "visibility": "list", "priority": 1, "input_modalities": ["text"]},
			{"slug": "a", "display_name": "A", "description": "Top", "visibility": "list", "priority": 1, "input_modalities": ["text", "image"]},
		]});
		let ids: Vec<String> = parse_codex_models(&v).into_iter().map(|m| m.id).collect();
		assert_eq!(ids, vec!["", "a", "b"]);
	}

	#[test]
	fn preamble_carries_earlier_turns_but_not_the_new_message() {
		let mut s = ChatSession::new();
		s.messages.push(Msg::User { text: "Count outlets".into(), context: None, attachments: vec![], images: vec![] });
		s.messages.push(Msg::Assistant { text: "Found 4.".into(), reasoning: String::new(), tool_calls: vec![] });
		s.messages.push(Msg::User { text: "Now switches".into(), context: None, attachments: vec![], images: vec![] });
		let p = history_preamble(&s);
		assert!(p.contains("Estimator: Count outlets") && p.contains("Bench: Found 4."));
		assert!(!p.contains("Now switches"));
		assert_eq!(history_preamble(&ChatSession::new()), "");
	}

	#[test]
	fn opencode_stream() {
		let lines = [
			r#"{"type":"step_start","sessionID":"ses_a","part":{"type":"step-start"}}"#,
			r#"{"type":"tool_use","sessionID":"ses_a","part":{"type":"tool","tool":"takeoff_view","callID":"c1","state":{"status":"completed","input":{"page":1},"output":"View v1"}}}"#,
			r#"{"type":"tool_use","sessionID":"ses_a","part":{"type":"tool","tool":"bash","callID":"c2","state":{"status":"completed","input":{"command":"ls"},"output":"a.txt\nb.txt"}}}"#,
			r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"tool-calls","tokens":{"input":800,"output":40,"cache":{"read":15000}}}}"#,
			r#"{"type":"text","sessionID":"ses_a","part":{"type":"text","text":"3"}}"#,
			r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"stop","tokens":{"input":600,"output":2,"cache":{"read":16000}}}}"#,
		];
		let evs = feed(&mut OpenCodeParser::default(), &lines);
		assert!(matches!(&evs[0], Ev::Thread(id) if id == "ses_a"));
		assert!(evs.iter().any(|e| matches!(e, Ev::OwnToolUse)), "our tools are reported by the host");
		assert!(evs.iter().any(|e| matches!(e, Ev::ToolStarted { name, .. } if name == "bash")));
		assert!(evs.iter().any(|e| matches!(e, Ev::ToolFinished { ok: true, summary, .. } if summary == "a.txt")));
		assert!(evs.iter().any(|e| matches!(e, Ev::Usage { prompt: 15800, completion: 40 })));
		assert!(evs.iter().any(|e| matches!(e, Ev::Text(t) if t == "3")));
		assert_eq!(evs.iter().filter(|e| matches!(e, Ev::Done)).count(), 1, "done only after the final step");
		assert!(matches!(evs.last(), Some(Ev::Done)));
	}

	#[test]
	fn opencode_errors_are_shown() {
		let line = r#"{"type":"error","sessionID":"ses_b","error":{"name":"APIError","data":{"message":"OpenCode's free tier can only be used from within OpenCode.","statusCode":403}}}"#;
		let evs = feed(&mut OpenCodeParser::default(), &[line]);
		assert!(evs.iter().any(|e| matches!(e, Ev::Error(m) if m.contains("free tier"))));
		assert!(matches!(evs.last(), Some(Ev::Done)));
	}

	#[test]
	fn opencode_models_keep_free_vision_tool_models_first() {
		let text = "opencode/deepseek-v4.1-flash-free\n{\n  \"name\": \"DeepSeek V4.1 Flash Free\",\n  \"cost\": {\"input\": 0, \"output\": 0},\n  \"capabilities\": {\"toolcall\": true, \"input\": {\"image\": true}}\n}\nopencode/text-only-free\n{\n  \"name\": \"Text Only\",\n  \"cost\": {\"input\": 0, \"output\": 0},\n  \"capabilities\": {\"toolcall\": true, \"input\": {\"image\": false}}\n}\nanthropic/claude-x\n{\n  \"name\": \"Claude X\",\n  \"cost\": {\"input\": 3, \"output\": 15},\n  \"capabilities\": {\"toolcall\": true, \"input\": {\"image\": true}}\n}\n";
		let m = parse_opencode_models(text);
		assert_eq!(m.len(), 2, "text-only models are left out: {m:?}");
		assert_eq!(m[0].id, "opencode/deepseek-v4.1-flash-free");
		assert_eq!(m[0].note, "Free");
		assert_eq!(m[1].note, "via anthropic");
	}

	#[test]
	fn install_and_login_commands_fit_the_platform() {
		let c = Channel::ClaudeCode.install_command();
		if cfg!(windows) {
			assert!(c.contains("install.ps1"));
		} else {
			assert!(c.contains("install.sh"));
		}
		assert!(Channel::Codex.needs_node() && !Channel::ClaudeCode.needs_node());
		let l = login_command_for(Channel::ClaudeCode, Path::new("/Users/o'brien/.local/bin/claude"));
		if !cfg!(windows) {
			assert_eq!(l, "'/Users/o'\\''brien/.local/bin/claude' auth login");
		}
	}

	#[test]
	fn versions_compare_against_the_minimum() {
		assert_eq!(parse_version("2.1.300 (Claude Code)"), Some((2, 1, 300)));
		assert_eq!(parse_version("codex-cli 0.162.1"), Some((0, 162, 1)));
		assert_eq!(parse_version("no version here"), None);
		assert!(too_old(Channel::ClaudeCode, "2.0.77 (Claude Code)"));
		assert!(too_old(Channel::ClaudeCode, "2.1.39 (Claude Code)"));
		assert!(!too_old(Channel::ClaudeCode, "2.1.40 (Claude Code)"));
		assert!(!too_old(Channel::ClaudeCode, "2.1.300 (Claude Code)"));
		assert!(!too_old(Channel::ClaudeCode, "3.0.0 (Claude Code)"));
		assert!(too_old(Channel::Codex, "codex-cli 0.139.0"));
		assert!(!too_old(Channel::Codex, "codex-cli 0.162.1"));
		assert!(!too_old(Channel::Codex, "something odd"), "unknown format is let through, not blocked");
		assert!(outdated_message(Channel::ClaudeCode, "2.0.77 (Claude Code)").contains("2.0.77 is too old"));
	}

	#[test]
	fn toml_strings_escape_windows_paths() {
		assert_eq!(toml_str(r"C:\Program Files\Takeoff Desk\takeoff-desk.exe"), r#""C:\\Program Files\\Takeoff Desk\\takeoff-desk.exe""#);
	}
}
