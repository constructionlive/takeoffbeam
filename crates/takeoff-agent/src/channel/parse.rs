//! Stream parsers: each CLI's JSON event stream, reduced to what the Bench shows.

use super::*;

/// What a CLI's JSON stream says, reduced to what the Bench shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Ev {
	Text(String),
	Reasoning(String),
	/// The CLI's conversation id, to resume next turn.
	Thread(String),
	/// A call to our own tools (the host reports those itself).
	OwnToolUse,
	/// Any other tool the CLI ran.
	ToolStarted { id: String, name: String, args: Value },
	ToolFinished { id: String, ok: bool, summary: String },
	Usage { prompt: u64, completion: u64 },
	Error(String),
	Done,
}

pub(super) trait Parser: Send {
	fn line(&mut self, line: &str) -> Vec<Ev>;
}

pub(super) fn u(v: &Value, ptr: &str) -> u64 {
	v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0)
}

pub(super) fn s<'a>(v: &'a Value, ptr: &str) -> &'a str {
	v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// `claude -p --output-format stream-json --verbose --include-partial-messages`.
#[derive(Default)]
pub(super) struct ClaudeParser {
	pub(super) text_seen: bool,
}

impl Parser for ClaudeParser {
	fn line(&mut self, line: &str) -> Vec<Ev> {
		let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
		let mut out = Vec::new();
		match s(&v, "/type") {
			"system" if s(&v, "/subtype") == "init" => {
				if let Some(id) = v.get("session_id").and_then(Value::as_str) {
					out.push(Ev::Thread(id.to_string()));
				}
				let ours = v.get("mcp_servers").and_then(Value::as_array).and_then(|a| a.iter().find(|m| s(m, "/name") == SERVER).cloned());
				if let Some(m) = ours.filter(|m| s(m, "/status") == "failed") {
					out.push(Ev::Error(format!("The drawing tools did not connect to Claude Code (MCP status: {}).", s(&m, "/status"))));
				}
			}
			"stream_event" => {
				let e = &v["event"];
				match s(e, "/type") {
					"content_block_start" if s(e, "/content_block/type") == "text" && self.text_seen => out.push(Ev::Text("\n\n".into())),
					"content_block_delta" => match s(e, "/delta/type") {
						"text_delta" => {
							let t = s(e, "/delta/text");
							if !t.is_empty() {
								self.text_seen = true;
								out.push(Ev::Text(t.to_string()));
							}
						}
						"thinking_delta" => out.push(Ev::Reasoning(s(e, "/delta/thinking").to_string())),
						_ => {}
					},
					_ => {}
				}
			}
			"assistant" => {
				for b in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
					if s(b, "/type") != "tool_use" {
						continue;
					}
					let name = s(b, "/name");
					if name.starts_with(&format!("mcp__{SERVER}__")) {
						out.push(Ev::OwnToolUse);
					} else {
						out.push(Ev::ToolStarted { id: s(b, "/id").into(), name: name.into(), args: b.get("input").cloned().unwrap_or(Value::Null) });
					}
				}
			}
			"user" => {
				for b in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
					if s(b, "/type") != "tool_result" {
						continue;
					}
					let summary = match b.get("content") {
						Some(Value::String(t)) => t.clone(),
						Some(Value::Array(a)) => a.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "),
						_ => String::new(),
					};
					let ok = !b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
					out.push(Ev::ToolFinished { id: s(b, "/tool_use_id").into(), ok, summary: summary.lines().next().unwrap_or("").chars().take(300).collect() });
				}
			}
			"result" => {
				let usage = &v["usage"];
				let prompt = u(usage, "/input_tokens") + u(usage, "/cache_read_input_tokens") + u(usage, "/cache_creation_input_tokens");
				out.push(Ev::Usage { prompt, completion: u(usage, "/output_tokens") });
				if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || s(&v, "/subtype").starts_with("error") {
					let msg = v.get("result").and_then(Value::as_str).filter(|r| !r.is_empty()).map(str::to_string).unwrap_or_else(|| {
						let errs: Vec<&str> = v.get("errors").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
						if errs.is_empty() { s(&v, "/subtype").replace('_', " ") } else { errs.join("; ") }
					});
					out.push(Ev::Error(format!("Claude Code: {msg}")));
				}
				out.push(Ev::Done);
			}
			_ => {}
		}
		out
	}
}

/// `codex exec --json` (JSONL thread events).
#[derive(Default)]
pub(super) struct CodexParser {
	/// Text already shown per item id (items can stream through item.updated).
	pub(super) shown: std::collections::HashMap<String, usize>,
	pub(super) text_seen: bool,
	pub(super) last_error: Option<String>,
	pub(super) done: bool,
}

impl CodexParser {
	pub(super) fn item_text(&mut self, item: &Value) -> Option<String> {
		let id = s(item, "/id").to_string();
		let text = s(item, "/text");
		let seen = self.shown.entry(id).or_insert(0);
		if text.len() <= *seen || !text.is_char_boundary(*seen) {
			return None;
		}
		let new = text[*seen..].to_string();
		let first = *seen == 0;
		*seen = text.len();
		Some(if first && self.text_seen && s(item, "/type") == "agent_message" { format!("\n\n{new}") } else { new })
	}
}

impl Parser for CodexParser {
	fn line(&mut self, line: &str) -> Vec<Ev> {
		let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
		let mut out = Vec::new();
		let item = &v["item"];
		match s(&v, "/type") {
			"thread.started" => {
				if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
					out.push(Ev::Thread(id.to_string()));
				}
			}
			t @ ("item.started" | "item.updated" | "item.completed") => match s(item, "/type") {
				"agent_message" => {
					if let Some(text) = self.item_text(item) {
						self.text_seen = true;
						out.push(Ev::Text(text));
					}
				}
				"reasoning" => {
					if let Some(text) = self.item_text(item) {
						out.push(Ev::Reasoning(text));
					}
				}
				"mcp_tool_call" if s(item, "/server") == SERVER => {
					if t == "item.started" {
						out.push(Ev::OwnToolUse);
					}
				}
				kind @ ("command_execution" | "mcp_tool_call" | "web_search" | "file_change") => {
					let id = s(item, "/id").to_string();
					if t == "item.started" {
						let (name, args) = match kind {
							"command_execution" => ("shell".to_string(), json!({ "command": s(item, "/command") })),
							"mcp_tool_call" => (format!("{}.{}", s(item, "/server"), s(item, "/tool")), item.get("arguments").cloned().unwrap_or(Value::Null)),
							other => (other.to_string(), Value::Null),
						};
						out.push(Ev::ToolStarted { id, name, args });
					} else if t == "item.completed" {
						let status = s(item, "/status");
						let ok = status != "failed" && item.get("exit_code").and_then(Value::as_i64).is_none_or(|c| c == 0);
						let summary = s(item, "/aggregated_output").lines().next().unwrap_or(status).chars().take(300).collect();
						out.push(Ev::ToolFinished { id, ok, summary });
					}
				}
				_ => {}
			},
			"turn.completed" => {
				self.done = true;
				out.push(Ev::Usage { prompt: u(&v, "/usage/input_tokens"), completion: u(&v, "/usage/output_tokens") });
				out.push(Ev::Done);
			}
			"turn.failed" => {
				self.done = true;
				let msg = Some(s(&v, "/error/message")).filter(|m| !m.is_empty()).map(str::to_string).or(self.last_error.take()).unwrap_or_else(|| "the turn failed".into());
				out.push(Ev::Error(format!("Codex: {msg}")));
				out.push(Ev::Done);
			}
			"error" => {
				// Often a retry notice; reported only if the turn never completes.
				let msg = s(&v, "/message").to_string();
				self.last_error = Some(msg.clone());
				if !self.done && msg.to_lowercase().contains("not logged in") {
					out.push(Ev::Error(format!("Codex: {msg}")));
				}
			}
			_ => {}
		}
		out
	}
}

/// `opencode run --format json`: one JSON event per line (step_start, text,
/// reasoning, tool_use, step_finish, error), each with the session id.
#[derive(Default)]
pub(super) struct OpenCodeParser {
	pub(super) session: Option<String>,
	pub(super) text_seen: bool,
}

impl Parser for OpenCodeParser {
	fn line(&mut self, line: &str) -> Vec<Ev> {
		let mut out = Vec::new();
		let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return out };
		let sid = s(&v, "/sessionID");
		if !sid.is_empty() && self.session.as_deref() != Some(sid) {
			self.session = Some(sid.to_string());
			out.push(Ev::Thread(sid.to_string()));
		}
		match s(&v, "/type") {
			"text" => {
				let t = s(&v, "/part/text");
				if !t.trim().is_empty() {
					if self.text_seen {
						out.push(Ev::Text("\n\n".into()));
					}
					self.text_seen = true;
					out.push(Ev::Text(t.to_string()));
				}
			}
			"reasoning" => {
				let t = s(&v, "/part/text");
				if !t.is_empty() {
					out.push(Ev::Reasoning(t.to_string()));
				}
			}
			"tool_use" => {
				let name = s(&v, "/part/tool");
				if name.starts_with(&format!("{SERVER}_")) {
					out.push(Ev::OwnToolUse);
				} else {
					// Reported once it has finished: start and finish together.
					let id = Some(s(&v, "/part/callID")).filter(|x| !x.is_empty()).unwrap_or(s(&v, "/part/id")).to_string();
					let ok = s(&v, "/part/state/status") != "error";
					let detail = if ok { s(&v, "/part/state/output") } else { s(&v, "/part/state/error") };
					let summary = detail.lines().find(|l| !l.trim().is_empty()).unwrap_or(if ok { "done" } else { "failed" }).chars().take(300).collect();
					out.push(Ev::ToolStarted { id: id.clone(), name: name.to_string(), args: v.pointer("/part/state/input").cloned().unwrap_or(Value::Null) });
					out.push(Ev::ToolFinished { id, ok, summary });
				}
			}
			"step_finish" => {
				let prompt = u(&v, "/part/tokens/input") + u(&v, "/part/tokens/cache/read");
				out.push(Ev::Usage { prompt, completion: u(&v, "/part/tokens/output") });
				// A step that ends in tool calls is followed by another step.
				if s(&v, "/part/reason") != "tool-calls" {
					out.push(Ev::Done);
				}
			}
			"error" => {
				let msg = Some(s(&v, "/error/data/message")).filter(|m| !m.is_empty()).unwrap_or(s(&v, "/error/name"));
				out.push(Ev::Error(format!("OpenCode: {msg}")));
				out.push(Ev::Done);
			}
			_ => {}
		}
		out
	}
}
