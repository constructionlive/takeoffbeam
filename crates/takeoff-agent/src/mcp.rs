//! MCP (JSON-RPC 2.0, one message per line) over the drawing tools — shared
//! by `takeoff mcp` (stdio, any MCP client) and the in-app tool host that
//! Claude Code / Codex sessions call into (see [`crate::channel`]).

use serde_json::{json, Value};

use crate::tools::{self, ToolOutput};

pub const PROTOCOL: &str = "2025-06-18";

pub const INSTRUCTIONS: &str = "Tools for quantity takeoff on one PDF drawing: view renders a page or region and returns an image with a view id; give positions back in that view's pixel coordinates (the server converts them). Marks you add are 'pending' until a person confirms them.";

/// `tools/list` result: `extra` first, then the drawing tools minus `skip`.
pub fn tool_list(extra: Vec<Value>, skip: &[&str]) -> Value {
	let mut list = extra;
	for t in tools::definitions().into_iter().filter(|t| !skip.contains(&t.name.as_str())) {
		list.push(json!({ "name": t.name, "description": t.description, "inputSchema": t.parameters }));
	}
	json!({ "tools": list })
}

pub fn text_result(text: impl Into<String>, is_error: bool) -> Value {
	json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

/// A tool's output as MCP content: its text, then each render with its caption.
pub fn output_result(out: &ToolOutput) -> Value {
	use base64::Engine;
	let mut content = vec![json!({ "type": "text", "text": out.text })];
	for img in &out.images {
		content.push(json!({ "type": "text", "text": img.caption }));
		content.push(json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&img.png), "mimeType": "image/png" }));
	}
	json!({ "content": content, "isError": false })
}

fn result(id: &Value, r: Value) -> Value {
	json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

pub fn error(id: &Value, code: i64, msg: &str) -> Value {
	json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

/// Answer one request. `list` builds the `tools/list` result, `call` runs a
/// tool (name, arguments) and returns a `tools/call` result. Notifications
/// get no reply.
pub fn reply(msg: &Value, list: impl FnOnce() -> Value, call: impl FnOnce(&str, Value) -> Value) -> Option<Value> {
	let id = msg.get("id")?;
	let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
	let params = msg.get("params").cloned().unwrap_or(Value::Null);
	Some(match method {
		"initialize" => result(id, json!({
			"protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
			"capabilities": { "tools": {} },
			"serverInfo": { "name": "takeoff-desk", "version": env!("CARGO_PKG_VERSION") },
			"instructions": INSTRUCTIONS
		})),
		"ping" => result(id, json!({})),
		"tools/list" => result(id, list()),
		"tools/call" => {
			let name = params.get("name").and_then(Value::as_str).unwrap_or("");
			let args = params.get("arguments").cloned().unwrap_or(json!({}));
			result(id, call(name, args))
		}
		other => error(id, -32601, &format!("method not found: {other}")),
	})
}
