//! A chat with the bench about one drawing: stored messages (persistable),
//! the transcript the UI shows, and the API message list (with images
//! injected after each run of tool results, newest few only).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::llm::{image_part, ToolCall};

pub const MAX_LIVE_IMAGES: usize = 6;

/// An area of a drawing the estimator attached to a message ("look here").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewAttachment {
	pub drawing: std::path::PathBuf,
	pub page: u32,
	/// Page fractions (x, y, w, h).
	pub region: (f64, f64, f64, f64),
}

impl ViewAttachment {
	/// What the model is told about one attachment.
	pub fn describe(&self, n: usize, this_drawing: &std::path::Path) -> String {
		let (x, y, w, h) = self.region;
		let file = self.drawing.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
		let other = if self.drawing != this_drawing { " (another open drawing: switch_drawing to it first)" } else { "" };
		format!("A{n}: \"{file}\"{other}, page {}, region x={x:.4} y={y:.4} w={w:.4} h={h:.4}", self.page)
	}
}

/// The note sent with a message that has attachments.
pub fn attachments_note(atts: &[ViewAttachment], this_drawing: &std::path::Path) -> String {
	if atts.is_empty() {
		return String::new();
	}
	let lines: Vec<String> = atts.iter().enumerate().map(|(i, a)| a.describe(i + 1, this_drawing)).collect();
	format!(
		"\n[The estimator attached {} view{} of the drawing: exactly what they are asking about. Look at each FIRST with view (that page and region) before answering, and keep your work to those areas unless they say otherwise.\n{}]",
		atts.len(),
		if atts.len() == 1 { "" } else { "s" },
		lines.join("\n")
	)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolImage {
	pub png: Arc<Vec<u8>>,
	pub width: u32,
	pub height: u32,
	pub caption: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Msg {
	User {
		text: String,
		/// Viewer state at send time (page, selection) — sent, not shown.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		context: Option<String>,
		/// Areas the estimator attached (shown as chips under the message).
		#[serde(default, skip_serializing_if = "Vec::is_empty")]
		attachments: Vec<ViewAttachment>,
	},
	Assistant {
		#[serde(default)]
		text: String,
		#[serde(default, skip_serializing_if = "String::is_empty")]
		reasoning: String,
		#[serde(default, skip_serializing_if = "Vec::is_empty")]
		tool_calls: Vec<ToolCall>,
	},
	Tool {
		call_id: String,
		name: String,
		text: String,
		ok: bool,
		/// Images are kept in memory only (a reloaded chat sees eviction notes).
		#[serde(skip)]
		images: Vec<ToolImage>,
		#[serde(default, skip_serializing_if = "Vec::is_empty")]
		image_captions: Vec<String>,
		#[serde(skip)]
		thumb: Option<Arc<Vec<u8>>>,
	},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
	User,
	Assistant,
	Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptEntry {
	pub role: Role,
	pub text: String,
	pub tool_name: Option<String>,
	pub tool_ok: Option<bool>,
	pub image_png: Option<Vec<u8>>,
	pub attachments: Vec<ViewAttachment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatSession {
	pub messages: Vec<Msg>,
	/// Rolling summary of compacted turns.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub summary: Option<String>,
	#[serde(default)]
	pub prompt_tokens: u64,
	#[serde(default)]
	pub completion_tokens: u64,
	/// The Claude Code / Codex conversation this chat continues, when it runs
	/// through a subscription channel.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub external: Option<ExternalThread>,
}

/// A conversation held by an outside agent CLI, resumed by id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalThread {
	/// [`crate::channel::Channel::key`]: "claude" or "codex".
	pub channel: String,
	pub id: String,
}

impl ChatSession {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn clear(&mut self) {
		*self = Self::default();
	}

	pub fn transcript(&self) -> Vec<TranscriptEntry> {
		let mut out = Vec::new();
		for m in &self.messages {
			match m {
				Msg::User { text, attachments, .. } => out.push(TranscriptEntry { role: Role::User, text: text.clone(), tool_name: None, tool_ok: None, image_png: None, attachments: attachments.clone() }),
				Msg::Assistant { text, .. } => {
					if !text.trim().is_empty() {
						out.push(TranscriptEntry { role: Role::Assistant, text: text.clone(), tool_name: None, tool_ok: None, image_png: None, attachments: vec![] });
					}
				}
				Msg::Tool { name, text, ok, thumb, .. } => out.push(TranscriptEntry {
					role: Role::Tool,
					text: first_line(text),
					tool_name: Some(name.clone()),
					tool_ok: Some(*ok),
					image_png: thumb.as_ref().map(|t| t.as_ref().clone()),
					attachments: vec![],
				}),
			}
		}
		out
	}

	/// The `messages` array for the API.
	pub fn api_messages(&self, system: &str, model: &str, vision: bool) -> Vec<Value> {
		let mut sys = system.to_string();
		if let Some(s) = &self.summary {
			sys.push_str("\n\n# Earlier in this conversation (summary)\n");
			sys.push_str(s);
		}
		let mut out = vec![json!({"role": "system", "content": sys})];
		// Which images are still live: the newest MAX_LIVE_IMAGES across all tool results.
		let mut live = std::collections::HashSet::new();
		let mut seen = 0;
		'outer: for (mi, m) in self.messages.iter().enumerate().rev() {
			if let Msg::Tool { images, .. } = m {
				for ii in (0..images.len()).rev() {
					if seen >= MAX_LIVE_IMAGES {
						break 'outer;
					}
					live.insert((mi, ii));
					seen += 1;
				}
			}
		}
		let mut pending_images: Vec<Value> = Vec::new();
		let flush = |out: &mut Vec<Value>, parts: &mut Vec<Value>| {
			if !parts.is_empty() {
				let mut content = vec![json!({"type": "text", "text": "Renders returned by the tool calls above:"})];
				content.append(parts);
				out.push(json!({"role": "user", "content": content}));
			}
		};
		for (mi, m) in self.messages.iter().enumerate() {
			if !matches!(m, Msg::Tool { .. }) {
				flush(&mut out, &mut pending_images);
			}
			match m {
				Msg::User { text, context, .. } => {
					let content = match context {
						Some(c) => format!("{c}\n\n{text}"),
						None => text.clone(),
					};
					out.push(json!({"role": "user", "content": content}));
				}
				Msg::Assistant { text, tool_calls, .. } => {
					let mut v = json!({"role": "assistant", "content": text});
					if !tool_calls.is_empty() {
						v["tool_calls"] = Value::Array(
							tool_calls
								.iter()
								.map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}}))
								.collect(),
						);
					}
					out.push(v);
				}
				Msg::Tool { call_id, text, images, image_captions, .. } => {
					out.push(json!({"role": "tool", "tool_call_id": call_id, "content": text}));
					let n = images.len().max(image_captions.len());
					for ii in 0..n {
						let caption = images.get(ii).map(|i| i.caption.clone()).or_else(|| image_captions.get(ii).cloned()).unwrap_or_default();
						match images.get(ii) {
							Some(img) if vision && live.contains(&(mi, ii)) => {
								pending_images.push(json!({"type": "text", "text": caption}));
								pending_images.push(image_part(&img.png, model));
							}
							_ if vision => pending_images.push(json!({"type": "text", "text": format!("[image no longer in context: {} — run view again to see it]", short(&caption))})),
							_ => {}
						}
					}
				}
			}
		}
		flush(&mut out, &mut pending_images);
		out
	}
}

fn first_line(t: &str) -> String {
	let l = t.lines().next().unwrap_or("").trim();
	if l.chars().count() > 240 {
		format!("{}…", l.chars().take(240).collect::<String>())
	} else {
		l.to_string()
	}
}

fn short(t: &str) -> String {
	t.split(" · ").take(2).collect::<Vec<_>>().join(" · ")
}

/// Rough token estimate for compaction (4 chars/token, images ≈ w*h/750).
pub fn estimate_tokens(messages: &[Value]) -> u64 {
	let mut t = 0u64;
	for m in messages {
		match m.get("content") {
			Some(Value::String(s)) => t += s.len() as u64 / 4,
			Some(Value::Array(parts)) => {
				for p in parts {
					if let Some(s) = p.get("text").and_then(Value::as_str) {
						t += s.len() as u64 / 4;
					} else if p.get("image_url").is_some() {
						t += 1600;
					}
				}
			}
			_ => {}
		}
		if let Some(calls) = m.get("tool_calls") {
			t += calls.to_string().len() as u64 / 4;
		}
		t += 4;
	}
	t
}

#[cfg(test)]
mod tests {
	use super::*;

	fn img(c: &str) -> ToolImage {
		ToolImage { png: Arc::new(vec![1, 2, 3]), width: 10, height: 10, caption: c.into() }
	}

	#[test]
	fn images_follow_the_tool_run_and_old_ones_are_evicted() {
		let mut s = ChatSession::new();
		s.messages.push(Msg::User { text: "count".into(), context: Some("[page 1]".into()), attachments: vec![] });
		for round in 0..4 {
			s.messages.push(Msg::Assistant { text: String::new(), reasoning: String::new(), tool_calls: vec![ToolCall { id: format!("c{round}a"), name: "view".into(), arguments: "{}".into() }, ToolCall { id: format!("c{round}b"), name: "view".into(), arguments: "{}".into() }] });
			for k in ["a", "b"] {
				s.messages.push(Msg::Tool { call_id: format!("c{round}{k}"), name: "view".into(), text: "ok".into(), ok: true, images: vec![img(&format!("View v{round}{k}"))], image_captions: vec![], thumb: None });
			}
		}
		let m = s.api_messages("sys", "claude", true);
		// Tool messages are never split by an image message.
		for w in m.windows(2) {
			if w[1]["role"] == "tool" {
				assert!(w[0]["role"] == "tool" || w[0]["tool_calls"].is_array(), "{w:?}");
			}
		}
		let image_parts: usize = m.iter().filter_map(|x| x["content"].as_array()).map(|p| p.iter().filter(|q| q.get("image_url").is_some()).count()).sum();
		assert_eq!(image_parts, MAX_LIVE_IMAGES);
		let text = serde_json::to_string(&m).unwrap();
		assert!(text.contains("no longer in context"));
		assert!(m[1]["content"].as_str().unwrap().starts_with("[page 1]"));
		// Without vision: no images and no notes.
		let m = s.api_messages("sys", "claude", false);
		assert!(!serde_json::to_string(&m).unwrap().contains("image_url"));
	}

	#[test]
	fn session_round_trips_without_images() {
		let mut s = ChatSession::new();
		s.messages.push(Msg::Tool { call_id: "c".into(), name: "view".into(), text: "t".into(), ok: true, images: vec![img("cap")], image_captions: vec!["cap".into()], thumb: None });
		let back: ChatSession = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
		let Msg::Tool { images, image_captions, .. } = &back.messages[0] else { panic!() };
		assert!(images.is_empty());
		assert_eq!(image_captions, &vec!["cap".to_string()]);
	}

	#[test]
	fn attachments_are_spelled_out_and_old_chats_still_load() {
		let here = std::path::Path::new("/d/Plans.pdf");
		let a = ViewAttachment { drawing: here.into(), page: 2, region: (0.1, 0.2, 0.3, 0.25) };
		let b = ViewAttachment { drawing: "/d/Electrical.pdf".into(), page: 1, region: (0.0, 0.0, 1.0, 1.0) };
		let note = attachments_note(&[a.clone(), b], here);
		assert!(note.contains("attached 2 views"), "{note}");
		assert!(note.contains("A1: \"Plans.pdf\", page 2, region x=0.1000 y=0.2000 w=0.3000 h=0.2500"), "{note}");
		assert!(note.contains("A2: \"Electrical.pdf\" (another open drawing: switch_drawing to it first)"), "{note}");
		assert_eq!(attachments_note(&[], here), "");

		let mut s = ChatSession::new();
		s.messages.push(Msg::User { text: "what is this".into(), context: Some(note.clone()), attachments: vec![a.clone()] });
		let back: ChatSession = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
		assert_eq!(back.transcript()[0].attachments, vec![a]);
		let api = back.api_messages("sys", "m", true);
		assert!(api.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("attached 2 views") && c.ends_with("what is this"))));
		// Saved before attachments existed.
		let old: ChatSession = serde_json::from_str(r#"{"messages":[{"role":"user","text":"hi"}]}"#).unwrap();
		assert!(old.transcript()[0].attachments.is_empty());
	}
}
