//! The document map tools as the agent uses them (view, record areas and
//! links off the view, read back, remove), the contact sheet, and `delegate`
//! running parallel subagents that write the same map, against a scripted
//! fake OpenAI server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::{json, Value};
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_agent::{start_turn, AgentContext, AgentEvent, ChatSession, ProviderConfig};
use takeoff_core::docmap::{self, LinkKind, PartKind};
use takeoff_core::{Document, TakeoffData};

/// Two 792×612 pt pages, each with one black square at (100, 100) pt.
fn fixture(name: &str) -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	for _ in 0..2 {
		let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
		let rect = PdfRect::new_from_values(612.0 - 100.0 - 80.0, 100.0, 612.0 - 100.0, 180.0);
		page.objects_mut().create_path_object_rect(rect, None, None, Some(PdfColor::new(0, 0, 0, 255))).unwrap();
	}
	let path = std::env::temp_dir().join(format!("takeoff-map-{name}-{}.pdf", std::process::id()));
	doc.save_to_file(&path).unwrap();
	let _ = docmap::delete(&path);
	path
}

fn doc() -> Arc<Mutex<Document>> {
	Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0); 2], "tester")))
}

#[test]
fn the_agent_records_areas_and_links_off_its_views() {
	let pdf = fixture("tools");
	let mut ctx = ToolCtx::new(doc(), takeoff_pdf::Drawing::open(&pdf).unwrap(), "test-model", None);
	assert!(tools::call(&mut ctx, "document_map", json!({})).unwrap().text.contains("no document map yet"));

	let contact = tools::call(&mut ctx, "contact_sheet", json!({})).unwrap();
	assert_eq!(contact.images.len(), 1);
	let snap = |name: &str, png: &[u8]| {
		if let Ok(dir) = std::env::var("MAP_SNAP_DIR") {
			std::fs::write(std::path::Path::new(&dir).join(name), png).unwrap();
		}
	};
	snap("contact.png", &contact.images[0].png);
	assert!(contact.text.contains("p1") && contact.text.contains("p2"), "{}", contact.text);

	let out = tools::call(&mut ctx, "map_sheets", json!({"overview": "Two sheets.", "sheets": [{"page": 1, "number": "A-101", "title": "Plan", "discipline": "Architectural"}, {"page": 2, "number": "A-501", "title": "Details"}]})).unwrap();
	assert!(out.text.contains("2 of 2 pages indexed, 0 mapped"), "{}", out.text);

	// Box the square on each page off a whole-page view, as a model would.
	let mut boxes = Vec::new();
	for page in 1..=2 {
		let v = tools::call(&mut ctx, "view", json!({"page": page})).unwrap();
		let img = &v.images[0];
		let (w, h) = (img.width as f64, img.height as f64);
		boxes.push((img.view_id.clone(), [100.0 / 792.0 * w, 100.0 / 612.0 * h, 180.0 / 792.0 * w, 180.0 / 612.0 * h]));
	}
	let (v1, b1) = &boxes[0];
	let (v2, b2) = &boxes[1];
	let out = tools::call(
		&mut ctx,
		"map_add",
		json!({
			"areas": [
				{"key": "plan", "view": v1, "box": [0, 0, b1[2] * 3.0, b1[3] * 3.0], "kind": "plan", "title": "Level 1"},
				{"key": "bubble", "view": v1, "box": b1, "kind": "callout", "label": "3/A-501", "parent": "plan"},
				{"key": "det", "view": v2, "box": b2, "kind": "detail", "label": "3", "title": "Stair rail", "note": "Steel rail, 42in high."}
			],
			"links": [
				{"from": "bubble", "to": "det", "kind": "detail", "text": "3/A-501", "note": "The north stair's rail is drawn in detail 3."},
				{"from": "plan", "outside": "S-201, structural set"}
			],
			"mapped_pages": [1]
		}),
	)
	.unwrap();
	assert!(out.text.contains("Added s1.1 (plan), Added s1.2 (bubble), Added s2.1 (det)"), "{}", out.text);
	// The check render shows the boxes on the page with most of them, and is a view.
	assert_eq!(out.images.len(), 1);
	assert!(out.images[0].caption.contains("page 1"));
	snap("check.png", &out.images[0].png);

	let map = docmap::load(&pdf).unwrap();
	let det = map.node("s2.1").unwrap();
	let r = det.region().unwrap();
	assert!((r.x - 100.0 / 792.0).abs() < 0.01 && (r.y - 100.0 / 612.0).abs() < 0.01 && (r.w - 80.0 / 792.0).abs() < 0.01, "{r:?}");
	let bubble = &map.sheet(1).unwrap().parts[1];
	assert_eq!((bubble.kind, bubble.parent.as_deref()), (PartKind::Callout, Some("s1.1")));
	assert_eq!(map.links.len(), 1);
	assert_eq!((map.links[0].from.as_str(), map.links[0].to.as_str(), map.links[0].kind), ("s1.2", "s2.1", LinkKind::Detail));
	assert_eq!(map.unresolved[0].text, "S-201, structural set");
	assert_eq!(map.pages_mapped(), vec![1]);
	assert_eq!(map.model.as_deref(), Some("test-model"));

	// Reading it back: the index, a sheet, an area with its links both ways, a search.
	let index = tools::call(&mut ctx, "document_map", json!({})).unwrap().text;
	assert!(index.contains("2 of 2 pages indexed, 1 mapped closely, 3 areas, 1 links") && index.contains("Indexed but not mapped closely: pages 2"), "{index}");
	let node = tools::call(&mut ctx, "document_map", json!({"node": "s2.1"})).unwrap().text;
	assert!(node.contains("Referenced from:") && node.contains("s1.2 → s2.1 [detail]") && node.contains("north stair"), "{node}");
	let sheet = tools::call(&mut ctx, "document_map", json!({"page": 1})).unwrap().text;
	assert!(sheet.contains("s1.2 [callout] Callout 3/A-501 (A-101) (in s1.1)") && sheet.contains("Points outside this PDF"), "{sheet}");
	assert!(tools::call(&mut ctx, "document_map", json!({"query": "north stair"})).unwrap().text.contains("link s1.2 → s2.1"));

	// Links can name a sheet by its number; a bad reference changes nothing.
	tools::call(&mut ctx, "map_add", json!({"links": [{"from": "s2.1", "to": "A-101", "kind": "same", "note": "Same stair"}]})).unwrap();
	assert!(tools::call(&mut ctx, "map_add", json!({"links": [{"from": "s2.1", "to": "Z-999"}]})).is_err());
	assert!(tools::call(&mut ctx, "map_add", json!({"areas": [{"kind": "detail"}]})).is_err());
	assert_eq!(docmap::load(&pdf).unwrap().links.len(), 2);

	// Changing an area keeps its id; removing the plan takes its callout and their links.
	tools::call(&mut ctx, "map_add", json!({"areas": [{"id": "s2.1", "title": "Stair handrail"}]})).unwrap();
	assert_eq!(docmap::load(&pdf).unwrap().node("s2.1").unwrap().title(), "Detail 3 · Stair handrail (A-501)");
	let out = tools::call(&mut ctx, "map_remove", json!({"ids": ["s1.1", "s9.9"]})).unwrap();
	assert!(out.text.contains("Removed 1 node") && out.text.contains("Not in the map: s9.9"), "{}", out.text);
	let map = docmap::load(&pdf).unwrap();
	assert!(map.sheet(1).unwrap().parts.is_empty());
	assert_eq!(map.links.len(), 1, "{:?}", map.links);
	assert!(map.unresolved.is_empty());
	let _ = docmap::delete(&pdf);
}

// ── delegate ────────────────────────────────────────────────────────────

fn sse(chunks: &[Value]) -> String {
	let mut s = String::new();
	for c in chunks {
		s.push_str(&format!("data: {c}\n\n"));
	}
	s.push_str("data: [DONE]\n\n");
	s
}

fn tool_call(id: &str, name: &str, args: Value) -> String {
	sse(&[
		json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]}),
		json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 1000, "completion_tokens": 20}}),
	])
}

fn text(t: &str) -> String {
	sse(&[json!({"choices": [{"delta": {"content": t}, "finish_reason": "stop"}]})])
}

fn read_request(stream: &mut std::net::TcpStream) -> Value {
	let mut reader = BufReader::new(stream.try_clone().unwrap());
	let mut len = 0;
	loop {
		let mut line = String::new();
		reader.read_line(&mut line).unwrap();
		if line == "\r\n" || line.is_empty() {
			break;
		}
		if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
			len = v.trim().parse().unwrap();
		}
	}
	let mut body = vec![0u8; len];
	reader.read_exact(&mut body).unwrap();
	serde_json::from_slice(&body).unwrap()
}

fn respond(stream: &mut std::net::TcpStream, body: &str) {
	let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
	stream.write_all(head.as_bytes()).unwrap();
	stream.write_all(body.as_bytes()).unwrap();
}

/// The fake model: the main agent delegates one page to each of two
/// subagents; each records its sheet and reports; the main agent sums up.
fn reply_to(body: &Value) -> String {
	let msgs = body["messages"].as_array().unwrap();
	let system = msgs[0]["content"].as_str().unwrap_or("");
	let last = msgs.last().unwrap();
	let tools: Vec<&str> = body["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str()).collect();
	if system.contains("# You are a subagent") {
		assert!(!tools.contains(&"delegate"), "subagents can't delegate");
		let brief = msgs.iter().find(|m| m["role"] == "user").unwrap().to_string();
		let page: u32 = if brief.contains("page 2") { 2 } else { 1 };
		return match last["role"].as_str() {
			Some("user") => tool_call(&format!("s{page}"), "map_sheets", json!({"sheets": [{"page": page, "number": format!("A-10{page}"), "mapped": true}]})),
			_ => text(&format!("Mapped page {page}.")),
		};
	}
	assert!(tools.contains(&"delegate"));
	match last["role"].as_str() {
		Some("user") => tool_call("d1", "delegate", json!({"tasks": [{"title": "Page 1", "brief": "Map page 1 of the set."}, {"title": "Page 2", "brief": "Map page 2 of the set."}]})),
		_ => {
			let result = last["content"].to_string();
			assert!(result.contains("Subagent 1 · Page 1") && result.contains("Mapped page 1.") && result.contains("Mapped page 2."), "{result}");
			text("Both sheets are mapped.")
		}
	}
}

#[test]
fn delegate_runs_subagents_in_parallel_on_the_same_map() {
	let pdf = fixture("delegate");
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let port = listener.local_addr().unwrap().port();
	let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
	let count = served.clone();
	std::thread::spawn(move || {
		for stream in listener.incoming() {
			let mut s = stream.unwrap();
			let count = count.clone();
			std::thread::spawn(move || {
				let body = read_request(&mut s);
				count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
				respond(&mut s, &reply_to(&body));
			});
		}
	});
	let ctx = AgentContext {
		doc: doc(),
		pdf_path: pdf.clone(),
		current_page: 1,
		visible_region: None,
		selected_ids: vec![],
		provider: ProviderConfig { base_url: format!("http://127.0.0.1:{port}/v1"), api_key: Some("k".into()), model: "test-model".into(), context_tokens: 32_768, vision: true },
		workspace: None,
		others: vec![],
		attachments: vec![],
		images: vec![],
	};
	let turn = start_turn(Arc::new(Mutex::new(ChatSession::new())), ctx, "Map this drawing set.".into());
	let mut events = Vec::new();
	while let Ok(e) = turn.events.recv_timeout(std::time::Duration::from_secs(60)) {
		let done = matches!(e, AgentEvent::Done { .. });
		events.push(e);
		if done {
			break;
		}
	}
	assert!(matches!(events.last(), Some(AgentEvent::Done { steps: 2 })), "{events:?}");
	assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error(_))), "{events:?}");
	assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { name, ok: true, .. } if name == "delegate")));
	// The subagents' own steps stay out of the chat.
	assert!(!events.iter().any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "map_sheets")), "{events:?}");
	// 2 main requests + 2 per subagent.
	assert_eq!(served.load(std::sync::atomic::Ordering::Relaxed), 6);
	let map = docmap::load(&pdf).unwrap();
	assert_eq!(map.pages_mapped(), vec![1, 2]);
	assert_eq!(map.sheet(2).unwrap().number.as_deref(), Some("A-102"));
	let _ = docmap::delete(&pdf);
}
