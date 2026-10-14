//! End-to-end turn against a scripted fake OpenAI server: the "model" views
//! the page, marks one of the drawn squares from the view's pixel frame, and
//! finishes. Checks the mark lands where the square is, the image was
//! injected after the tool run, and the event stream is complete.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::{json, Value};
use takeoff_agent::{start_turn, AgentContext, AgentEvent, ChatSession, ProviderConfig, Role};
use takeoff_core::{Document, TakeoffData};

/// 792×612 pt page with black squares; returns path and the squares' fractions.
fn fixture() -> (std::path::PathBuf, Vec<(f64, f64, f64, f64)>) {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
	let squares: [(f32, f32); 3] = [(100.0, 100.0), (300.0, 100.0), (500.0, 300.0)];
	let mut fr = Vec::new();
	for (x, y) in squares {
		// PDF y is bottom-up.
		let rect = PdfRect::new_from_values(612.0 - y - 40.0, x, 612.0 - y, x + 40.0);
		page.objects_mut().create_path_object_rect(rect, None, None, Some(PdfColor::new(0, 0, 0, 255))).unwrap();
		fr.push((x as f64 / 792.0, y as f64 / 612.0, 40.0 / 792.0, 40.0 / 612.0));
	}
	let path = std::env::temp_dir().join(format!("takeoff-agent-fixture-{}.pdf", std::process::id()));
	doc.save_to_file(&path).unwrap();
	(path, fr)
}

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
		json!({"choices": [{"delta": {"content": "Working on it. "}}]}),
		json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]}),
		json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 1000, "completion_tokens": 20}}),
	])
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

#[test]
fn view_then_mark_then_finish() {
	let (pdf, squares) = fixture();
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let port = listener.local_addr().unwrap().port();
	let target = squares[1];
	let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
	let reqs = requests.clone();
	let server = std::thread::spawn(move || {
		for step in 0..3 {
			let (mut s, _) = listener.accept().unwrap();
			let body = read_request(&mut s);
			reqs.lock().push(body.clone());
			let reply = match step {
				0 => tool_call("c1", "view", json!({"page": 1})),
				1 => {
					// Read the view's pixel size off its caption like a model would.
					let text = body.to_string();
					let at = text.find("View v1").expect("caption in the injected image message");
					let tail = &text[at..];
					let px = tail.split(" · ").find_map(|p| p.split_once(" px").map(|x| x.0).filter(|x| x.contains('x') && x.chars().next().unwrap().is_ascii_digit())).unwrap();
					let (w, h) = px.split_once('x').unwrap();
					let (w, h): (f64, f64) = (w.parse().unwrap(), h.parse().unwrap());
					let b = [target.0 * w, target.1 * h, (target.0 + target.2) * w, (target.1 + target.3) * h];
					tool_call("c2", "add_marks", json!({"view": "v1", "boxes": [b], "category": "Squares", "description": "black square"}))
				}
				_ => sse(&[json!({"choices": [{"delta": {"content": "Marked 1 square (badge 1), pending your review."}, "finish_reason": "stop"}]})]),
			};
			respond(&mut s, &reply);
		}
	});

	let page_sizes = vec![(792.0, 612.0)];
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), page_sizes, "tester")));
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let ctx = AgentContext {
		doc: doc.clone(),
		pdf_path: pdf.clone(),
		current_page: 1,
		visible_region: None,
		selected_ids: vec![],
		provider: ProviderConfig { base_url: format!("http://127.0.0.1:{port}/v1"), api_key: Some("k".into()), model: "test-model".into(), context_tokens: 32_768, vision: true },
	};
	let turn = start_turn(session.clone(), ctx, "Mark the middle square".into());
	let mut events = Vec::new();
	while let Ok(e) = turn.events.recv_timeout(std::time::Duration::from_secs(60)) {
		let done = matches!(e, AgentEvent::Done { .. });
		events.push(e);
		if done {
			break;
		}
	}
	server.join().unwrap();

	assert!(matches!(events.last(), Some(AgentEvent::Done { steps: 3 })), "{events:?}");
	assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { name, ok: true, image: Some(_), .. } if name == "view")));
	assert!(events.iter().any(|e| matches!(e, AgentEvent::DocChanged)));
	assert!(events.iter().any(|e| matches!(e, AgentEvent::Viewport { page: 1, .. })));
	assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error(_))), "{events:?}");

	let d = doc.lock();
	assert_eq!(d.data.marks.len(), 1);
	let m = &d.data.marks[0];
	assert!((m.x - target.0).abs() < 0.004 && (m.y - target.1).abs() < 0.004, "mark at {:?} vs {target:?}", (m.x, m.y));
	assert_eq!(m.status, takeoff_core::MarkStatus::Pending);
	assert_eq!(m.meta.author.as_deref(), Some("AI"));
	assert_eq!(d.data.categories[0].name, "Squares");
	assert_eq!(d.undo_label(), Some("AI: add 1 mark(s)"));

	// Request 2 carries the render as a user message right after the tool results.
	let r = requests.lock();
	let msgs = r[1]["messages"].as_array().unwrap();
	let tool_idx = msgs.iter().position(|m| m["role"] == "tool").unwrap();
	let next = &msgs[tool_idx + 1];
	assert_eq!(next["role"], "user");
	assert!(next["content"].to_string().contains("data:image/png;base64"));
	assert!(r[0]["tools"].as_array().unwrap().len() >= 20);

	let t = session.lock().transcript();
	assert_eq!(t[0].role, Role::User);
	assert!(t.iter().any(|e| e.role == Role::Tool && e.tool_name.as_deref() == Some("add_marks") && e.tool_ok == Some(true)));
	assert!(t.last().unwrap().text.contains("Marked 1 square"));
	std::fs::remove_file(pdf).ok();
}

#[test]
fn tools_registry_runs_without_a_model() {
	let (pdf, squares) = fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0)], "t")));
	let drawing = takeoff_pdf::Drawing::open(&pdf).unwrap();
	let mut ctx = takeoff_agent::tools::ToolCtx::new(doc.clone(), drawing, "google/gemini-3.6-flash", None);
	let out = takeoff_agent::tools::call(&mut ctx, "view", json!({"page": 1})).unwrap();
	assert_eq!(out.images.len(), 1);
	assert!(out.text.contains("box_2d [y1,x1,y2,x2]"));
	// Gemini dialect: [ymin, xmin, ymax, xmax] on 0-1000.
	let s = squares[2];
	let b = [s.1 * 1000.0, s.0 * 1000.0, (s.1 + s.3) * 1000.0, (s.0 + s.2) * 1000.0];
	takeoff_agent::tools::call(&mut ctx, "add_marks", json!({"view": "v1", "boxes": [b], "category": "Sq"})).unwrap();
	// Same box again is a duplicate.
	let again = takeoff_agent::tools::call(&mut ctx, "add_marks", json!({"view": "v1", "boxes": [b], "category": "Sq"})).unwrap();
	assert!(again.text.contains("skipped 1 duplicate"));
	let m = doc.lock().data.marks[0].clone();
	assert!((m.x - s.0).abs() < 0.002 && (m.y - s.1).abs() < 0.002);
	// Scale from two points 400 pt apart = 40 ft, then measure back.
	takeoff_agent::tools::call(&mut ctx, "set_scale", json!({"view": "v1", "p1": [500, 100], "p2": [500, 605.05], "distance": 40.0, "unit": "ft", "snap": false})).unwrap();
	let chk = takeoff_agent::tools::call(&mut ctx, "check_scale", json!({"view": "v1", "p1": [500, 100], "p2": [500, 605.05], "expected": 40.0, "snap": false})).unwrap();
	assert!(chk.text.contains("scale verified"), "{}", chk.text);
	let preset = takeoff_agent::tools::call(&mut ctx, "set_scale_preset", json!({"preset": "1/4\" = 1'-0\""})).unwrap();
	assert!(preset.text.contains("1/4"));
	let sum = takeoff_agent::tools::call(&mut ctx, "summary", json!({})).unwrap();
	assert!(sum.text.contains("Sq [count]: 1 ea"), "{}", sum.text);
	let found = takeoff_agent::tools::call(&mut ctx, "find_items_at", json!({"view": "v1", "point": [b[0] + 5.0, b[1] + 5.0]})).unwrap();
	assert!(found.text.contains(&m.id));
	assert!(takeoff_agent::tools::call(&mut ctx, "view", json!({"zoom_view": "v9", "zoom_box": [0, 0, 10, 10]})).err().unwrap().contains("unknown view"));
	assert!(takeoff_agent::tools::definitions().iter().all(|d| d.parameters["type"] == "object"));
	std::fs::remove_file(pdf).ok();
}
