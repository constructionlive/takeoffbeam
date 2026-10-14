//! MCP server over stdio: the same drawing tools the built-in agent uses
//! (view, count, mark, measure, read, layers…), so Claude Desktop, Cursor or
//! any MCP client can work a drawing. JSON-RPC 2.0, one message per line.
//!
//! Register in Claude Desktop's config:
//!   { "mcpServers": { "takeoff": { "command": "takeoff", "args": ["mcp", "/path/plan.pdf"] } } }

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_core::{sidecar, Document};
use takeoff_pdf::Drawing;

const PROTOCOL: &str = "2025-06-18";

struct Session {
	pdf: Option<PathBuf>,
	ctx: Option<ToolCtx>,
	doc: Option<Arc<Mutex<Document>>>,
}

impl Session {
	fn open(&mut self, path: PathBuf) -> Result<String, String> {
		let drawing = Drawing::open(&path).map_err(|e| e.to_string())?;
		let n = drawing.page_count();
		let sizes = (1..=n).map(|p| drawing.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect();
		let data = sidecar::load(&path).map_err(|e| e.to_string())?;
		let doc = Arc::new(Mutex::new(Document::new(data, sizes, "AI")));
		// MCP clients are usually Claude: pixel coordinates, Claude-sized renders.
		let model = std::env::var("TAKEOFF_MCP_MODEL").unwrap_or_else(|_| "claude".into());
		self.ctx = Some(ToolCtx::new(doc.clone(), drawing, &model, None));
		self.doc = Some(doc);
		self.pdf = Some(path.clone());
		Ok(format!("Opened {} ({n} pages). Takeoff saves to {}", path.display(), sidecar::sidecar_path(&path).display()))
	}

	fn save(&self) {
		if let (Some(pdf), Some(doc)) = (&self.pdf, &self.doc) {
			let mut d = doc.lock();
			if d.is_dirty() && sidecar::save(pdf, &d.data).is_ok() {
				d.mark_saved();
			}
		}
	}
}

fn tool_list() -> Value {
	let mut list: Vec<Value> = vec![json!({
		"name": "open_drawing",
		"description": "Open a PDF drawing to work on (its takeoff is kept in <file>.takeoff.json next to it). Call this first unless the server was started with a drawing.",
		"inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Absolute path to the PDF" } }, "required": ["path"] }
	})];
	for t in tools::definitions() {
		list.push(json!({ "name": t.name, "description": t.description, "inputSchema": t.parameters }));
	}
	json!({ "tools": list })
}

fn result(id: &Value, r: Value) -> Value {
	json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

fn error(id: &Value, code: i64, msg: &str) -> Value {
	json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

fn call(s: &mut Session, name: &str, args: Value) -> Value {
	let text_result = |t: String, is_error: bool| json!({ "content": [{ "type": "text", "text": t }], "isError": is_error });
	if name == "open_drawing" {
		let Some(p) = args.get("path").and_then(Value::as_str) else { return text_result("path is required".into(), true) };
		return match s.open(PathBuf::from(p)) {
			Ok(t) => text_result(t, false),
			Err(e) => text_result(e, true),
		};
	}
	let Some(ctx) = s.ctx.as_mut() else { return text_result("No drawing is open. Call open_drawing with a PDF path first.".into(), true) };
	let out = match tools::call(ctx, name, args) {
		Ok(o) => o,
		Err(e) => return text_result(e, true),
	};
	s.save();
	let mut content = vec![json!({ "type": "text", "text": out.text })];
	for img in out.images {
		use base64::Engine;
		content.push(json!({ "type": "text", "text": img.caption }));
		content.push(json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&img.png), "mimeType": "image/png" }));
	}
	json!({ "content": content, "isError": false })
}

pub fn serve(pdf: Option<PathBuf>) -> Result<(), String> {
	let mut s = Session { pdf: None, ctx: None, doc: None };
	if let Some(p) = pdf {
		s.open(p)?;
	}
	let stdin = std::io::stdin();
	let mut stdout = std::io::stdout().lock();
	for line in stdin.lock().lines() {
		let line = line.map_err(|e| e.to_string())?;
		if line.trim().is_empty() {
			continue;
		}
		let msg: Value = match serde_json::from_str(&line) {
			Ok(v) => v,
			Err(e) => {
				let _ = writeln!(stdout, "{}", error(&Value::Null, -32700, &format!("parse error: {e}")));
				continue;
			}
		};
		let id = msg.get("id").cloned();
		let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
		let params = msg.get("params").cloned().unwrap_or(Value::Null);
		let Some(id) = id else { continue }; // notifications (initialized, cancelled…)
		let reply = match method {
			"initialize" => result(&id, json!({
				"protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
				"capabilities": { "tools": {} },
				"serverInfo": { "name": "takeoff-desk", "version": env!("CARGO_PKG_VERSION") },
				"instructions": "Tools for quantity takeoff on one PDF drawing: view renders a page or region and returns an image with a view id; give positions back in that view's pixel coordinates (the server converts them). Marks you add are 'pending' until a person confirms them."
			})),
			"ping" => result(&id, json!({})),
			"tools/list" => result(&id, tool_list()),
			"tools/call" => {
				let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
				let args = params.get("arguments").cloned().unwrap_or(json!({}));
				result(&id, call(&mut s, &name, args))
			}
			other => error(&id, -32601, &format!("method not found: {other}")),
		};
		writeln!(stdout, "{reply}").map_err(|e| e.to_string())?;
		stdout.flush().map_err(|e| e.to_string())?;
	}
	s.save();
	Ok(())
}
