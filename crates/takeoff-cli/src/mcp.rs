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
use takeoff_agent::mcp;
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_core::{sidecar, Document};
use takeoff_pdf::Drawing;

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

fn open_drawing_tool() -> Value {
	json!({
		"name": "open_drawing",
		"description": "Open a PDF drawing to work on (its takeoff is kept in <file>.takeoff.json next to it). Call this first unless the server was started with a drawing.",
		"inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Absolute path to the PDF" } }, "required": ["path"] }
	})
}

fn call(s: &mut Session, name: &str, args: Value) -> Value {
	if name == "open_drawing" {
		let Some(p) = args.get("path").and_then(Value::as_str) else { return mcp::text_result("path is required", true) };
		return match s.open(PathBuf::from(p)) {
			Ok(t) => mcp::text_result(t, false),
			Err(e) => mcp::text_result(e, true),
		};
	}
	let Some(ctx) = s.ctx.as_mut() else { return mcp::text_result("No drawing is open. Call open_drawing with a PDF path first.", true) };
	let out = match tools::call(ctx, name, args) {
		Ok(o) => o,
		Err(e) => return mcp::text_result(e, true),
	};
	s.save();
	mcp::output_result(&out)
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
				let _ = writeln!(stdout, "{}", mcp::error(&Value::Null, -32700, &format!("parse error: {e}")));
				continue;
			}
		};
		let Some(reply) = mcp::reply(&msg, || mcp::tool_list(vec![open_drawing_tool()], takeoff_agent::tools::SHELL_TOOLS), |name, args| call(&mut s, name, args)) else { continue }; // notifications (initialized, cancelled…)
		writeln!(stdout, "{reply}").map_err(|e| e.to_string())?;
		stdout.flush().map_err(|e| e.to_string())?;
	}
	s.save();
	Ok(())
}
