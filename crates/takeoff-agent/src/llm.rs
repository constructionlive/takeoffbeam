//! OpenAI-compatible chat completions: streaming (SSE) with tool calls and
//! reasoning deltas, plus a one-shot call for vision pipelines and compaction.
//! Works against llama-server, the construction.live desktop gateway, or any
//! OpenAI-style endpoint.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::ProviderConfig;

/// One tool call as the model wrote it (arguments are a JSON string).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
	pub id: String,
	pub name: String,
	pub arguments: String,
}

/// What one streamed completion produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completion {
	pub text: String,
	pub reasoning: String,
	pub tool_calls: Vec<ToolCall>,
	pub finish_reason: Option<String>,
	pub prompt_tokens: u64,
	pub completion_tokens: u64,
}

/// Incremental parser for one SSE stream of `chat.completion.chunk`s.
#[derive(Debug, Default)]
pub struct StreamAssembler {
	pub out: Completion,
	partial: Vec<ToolCall>,
}

/// A piece of the stream worth showing live.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
	Text(String),
	Reasoning(String),
}

impl StreamAssembler {
	/// Feed one SSE line. Returns live deltas, and `true` once the stream ends.
	pub fn line(&mut self, line: &str) -> (Vec<Delta>, bool) {
		let line = line.trim_end_matches(['\r', '\n']);
		let Some(data) = line.strip_prefix("data:") else { return (vec![], false) };
		let data = data.trim();
		if data == "[DONE]" {
			return (vec![], true);
		}
		let Ok(v) = serde_json::from_str::<Value>(data) else { return (vec![], false) };
		(self.chunk(&v), false)
	}

	pub fn chunk(&mut self, v: &Value) -> Vec<Delta> {
		let mut deltas = Vec::new();
		if let Some(err) = v.get("error") {
			let msg = err.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| err.to_string());
			self.out.finish_reason = Some(format!("error: {msg}"));
		}
		if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
			self.out.prompt_tokens = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(self.out.prompt_tokens);
			self.out.completion_tokens = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(self.out.completion_tokens);
		}
		let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else { return deltas };
		if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
			self.out.finish_reason = Some(fr.to_string());
		}
		// Non-streamed responses come through here too ("message" instead of "delta").
		let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) else { return deltas };
		if let Some(t) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
			self.out.text.push_str(t);
			deltas.push(Delta::Text(t.to_string()));
		}
		for key in ["reasoning_content", "reasoning"] {
			if let Some(t) = delta.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()) {
				self.out.reasoning.push_str(t);
				deltas.push(Delta::Reasoning(t.to_string()));
				break;
			}
		}
		if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
			for (pos, c) in calls.iter().enumerate() {
				// A garbage index (a broken server) would grow the list without end.
				let idx = c.get("index").and_then(Value::as_u64).filter(|i| *i < 128).map(|i| i as usize).unwrap_or(pos);
				while self.partial.len() <= idx {
					self.partial.push(ToolCall::default());
				}
				let slot = &mut self.partial[idx];
				if let Some(id) = c.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
					slot.id = id.to_string();
				}
				if let Some(f) = c.get("function") {
					if let Some(n) = f.get("name").and_then(Value::as_str) {
						slot.name.push_str(n);
					}
					match f.get("arguments") {
						Some(Value::String(a)) => slot.arguments.push_str(a),
						Some(other) if !other.is_null() => slot.arguments.push_str(&other.to_string()),
						_ => {}
					}
				}
			}
		}
		deltas
	}

	pub fn finish(mut self) -> Completion {
		let mut n = 0;
		for mut c in self.partial.drain(..) {
			if c.name.is_empty() {
				continue;
			}
			n += 1;
			if c.id.is_empty() {
				c.id = format!("call_{n}_{}", takeoff_core::now_ms());
			}
			if c.arguments.trim().is_empty() {
				c.arguments = "{}".into();
			}
			self.out.tool_calls.push(c);
		}
		if !self.out.tool_calls.is_empty() && self.out.finish_reason.as_deref().is_none_or(|f| f == "stop") {
			// Some servers report "stop" alongside tool calls; the calls win.
			self.out.finish_reason = Some("tool_calls".into());
		}
		self.out
	}
}

fn client(timeout: Option<Duration>) -> Result<reqwest::blocking::Client, String> {
	reqwest::blocking::Client::builder()
		.timeout(timeout)
		.connect_timeout(Duration::from_secs(20))
		.build()
		.map_err(|e| format!("could not set up the HTTP client: {e}"))
}

fn endpoint(base: &str, path: &str) -> String {
	format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn request(p: &ProviderConfig, body: &Value, timeout: Option<Duration>) -> Result<reqwest::blocking::Response, String> {
	let mut req = client(timeout)?.post(endpoint(&p.base_url, "chat/completions")).json(body);
	if let Some(k) = p.api_key.as_deref().filter(|k| !k.is_empty()) {
		req = req.bearer_auth(k);
	}
	let resp = req.send().map_err(|e| format!("could not reach the model server at {}: {e}", p.base_url))?;
	if !resp.status().is_success() {
		let status = resp.status();
		let body = resp.text().unwrap_or_default();
		let msg = serde_json::from_str::<Value>(&body)
			.ok()
			.and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
			.unwrap_or_else(|| body.chars().take(600).collect());
		return Err(format!("model server returned {status}: {msg}"));
	}
	Ok(resp)
}

/// Stream one completion. `on_delta` sees text/reasoning as it arrives.
pub fn stream(
	p: &ProviderConfig,
	messages: &[Value],
	tools: &[Value],
	cancel: &AtomicBool,
	mut on_delta: impl FnMut(Delta),
) -> Result<Completion, String> {
	let mut body = json!({
		"model": p.model,
		"messages": messages,
		"stream": true,
		"stream_options": { "include_usage": true },
	});
	if !tools.is_empty() {
		body["tools"] = Value::Array(tools.to_vec());
		body["tool_choice"] = json!("auto");
	}
	if crate::is_local_endpoint(&p.base_url) {
		// Bound one step on a local model; thinking only when asked for.
		body["max_tokens"] = json!(8192);
		body["chat_template_kwargs"] = json!({ "enable_thinking": crate::local_thinking() });
	}
	let resp = request(p, &body, None)?;
	let mut asm = StreamAssembler::default();
	let mut reader = BufReader::new(resp);
	let mut line = String::new();
	loop {
		if cancel.load(Ordering::Relaxed) {
			return Err("cancelled".into());
		}
		line.clear();
		match reader.read_line(&mut line) {
			Ok(0) => break,
			Ok(_) => {
				let (deltas, done) = asm.line(&line);
				for d in deltas {
					on_delta(d);
				}
				if done {
					break;
				}
			}
			Err(e) => return Err(format!("stream interrupted: {e}")),
		}
	}
	let out = asm.finish();
	if let Some(err) = out.finish_reason.as_deref().and_then(|f| f.strip_prefix("error: ")) {
		return Err(format!("model error: {err}"));
	}
	Ok(out)
}

/// One non-streamed completion (vision sweeps, compaction). Returns the text.
pub fn complete(p: &ProviderConfig, messages: &[Value], max_tokens: Option<u32>) -> Result<(String, u64, u64), String> {
	complete_with(p, messages, max_tokens, false)
}

/// `no_think`: ask llama.cpp chat templates (Qwen3, …) to skip the thinking
/// phase — sweeps want a JSON answer, not an essay, and it is much faster.
pub fn complete_with(p: &ProviderConfig, messages: &[Value], max_tokens: Option<u32>, no_think: bool) -> Result<(String, u64, u64), String> {
	let mut body = json!({ "model": p.model, "messages": messages, "stream": false, "temperature": 0.1 });
	if let Some(m) = max_tokens {
		body["max_tokens"] = json!(m);
	}
	if no_think {
		body["chat_template_kwargs"] = json!({ "enable_thinking": false });
	}
	let resp = request(p, &body, Some(Duration::from_secs(600)))?;
	let v: Value = resp.json().map_err(|e| format!("bad response from model server: {e}"))?;
	let text = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("").to_string();
	let pt = v.pointer("/usage/prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
	let ct = v.pointer("/usage/completion_tokens").and_then(Value::as_u64).unwrap_or(0);
	Ok((text, pt, ct))
}

/// Model ids the server offers (`GET /models`).
pub fn models(base_url: &str, api_key: Option<&str>) -> Result<Vec<String>, String> {
	let mut req = client(Some(Duration::from_secs(20)))?.get(endpoint(base_url, "models"));
	if let Some(k) = api_key.filter(|k| !k.is_empty()) {
		req = req.bearer_auth(k);
	}
	let resp = req.send().map_err(|e| format!("could not reach {base_url}: {e}"))?;
	if !resp.status().is_success() {
		return Err(format!("{base_url} returned {}", resp.status()));
	}
	let v: Value = resp.json().map_err(|e| e.to_string())?;
	let list = v.get("data").or_else(|| v.get("models")).and_then(Value::as_array).cloned().unwrap_or_default();
	Ok(list
		.iter()
		.filter_map(|m| m.get("id").or_else(|| m.get("name")).or_else(|| m.get("model")).and_then(Value::as_str).map(str::to_string))
		.collect())
}

/// An image part for a user message (PNG, or JPEG for photos).
pub fn image_part(bytes: &[u8], model: &str) -> Value {
	use base64::Engine;
	let mime = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else { "image/png" };
	let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
	let mut iu = json!({ "url": url });
	if takeoff_core::coords::wants_original_detail(model) {
		iu["detail"] = json!("original");
	}
	json!({ "type": "image_url", "image_url": iu })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn assembles_streamed_tool_calls() {
		let mut a = StreamAssembler::default();
		let lines = [
			r#"data: {"choices":[{"delta":{"role":"assistant","reasoning_content":"Look first."}}]}"#,
			r#"data: {"choices":[{"delta":{"content":"Rendering "}}]}"#,
			r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"view","arguments":""}}]}}]}"#,
			r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"page\":"}}]}}]}"#,
			r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}},{"index":1,"id":"call_b","function":{"name":"summary","arguments":"{}"}}]}}]}"#,
			r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
			r#"data: {"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":30}}"#,
			"data: [DONE]",
		];
		let mut deltas = vec![];
		let mut done = false;
		for l in lines {
			let (d, end) = a.line(l);
			deltas.extend(d);
			done |= end;
		}
		assert!(done);
		assert_eq!(deltas[0], Delta::Reasoning("Look first.".into()));
		let c = a.finish();
		assert_eq!(c.text, "Rendering ");
		assert_eq!(c.tool_calls.len(), 2);
		assert_eq!(c.tool_calls[0], ToolCall { id: "call_a".into(), name: "view".into(), arguments: "{\"page\":1}".into() });
		assert_eq!(c.tool_calls[1].name, "summary");
		assert_eq!(c.finish_reason.as_deref(), Some("tool_calls"));
		assert_eq!((c.prompt_tokens, c.completion_tokens), (120, 30));
	}

	#[test]
	fn ignores_comments_and_keepalives() {
		let mut a = StreamAssembler::default();
		assert_eq!(a.line(": ping").0, vec![]);
		assert_eq!(a.line("").0, vec![]);
		assert_eq!(a.line("event: message").0, vec![]);
		let (d, _) = a.line(r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#);
		assert_eq!(d, vec![Delta::Text("hi".into())]);
		let c = a.finish();
		assert_eq!(c.finish_reason.as_deref(), Some("stop"));
		assert!(c.tool_calls.is_empty());
	}

	#[test]
	fn garbage_tool_call_index_stays_bounded() {
		let mut a = StreamAssembler::default();
		a.line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":18446744073709551615,"id":"x","function":{"name":"view","arguments":"{}"}}]}}]}"#);
		a.line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":4000000000,"function":{"name":"summary"}}]}}]}"#);
		assert_eq!(a.partial.len(), 1);
	}

	#[test]
	fn surfaces_stream_errors() {
		let mut a = StreamAssembler::default();
		a.line(r#"data: {"error":{"message":"context too long"}}"#);
		assert_eq!(a.finish().finish_reason.as_deref(), Some("error: context too long"));
	}
}
