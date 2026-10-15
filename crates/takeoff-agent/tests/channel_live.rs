//! A real turn through the estimator's Claude Code / Codex CLI, with the
//! drawing tools served over the MCP bridge. Needs the CLI installed and
//! signed in, and a built `takeoff-desk` for the bridge, so it is opt-in:
//!
//!   cargo build -p takeoff-app
//!   TAKEOFF_BRIDGE_EXE=target/debug/takeoff-desk cargo test -p takeoff-agent --test channel_live -- --ignored --nocapture
//!
//! TAKEOFF_LIVE_CHANNEL=codex runs it on Codex; TAKEOFF_LIVE_MODEL picks the model.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use takeoff_agent::channel::{self, Channel, ChannelTurn, ToolHost};
use takeoff_agent::{AgentEvent, ChatSession};
use takeoff_core::{Document, TakeoffData};

/// 792×612 pt page with three black squares.
fn fixture() -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
	for (x, y) in [(100.0f32, 100.0f32), (300.0, 100.0), (500.0, 300.0)] {
		let rect = PdfRect::new_from_values(612.0 - y - 40.0, x, 612.0 - y, x + 40.0);
		page.objects_mut().create_path_object_rect(rect, None, None, Some(PdfColor::new(0, 0, 0, 255))).unwrap();
	}
	let path = std::env::temp_dir().join(format!("takeoff-channel-live-{}.pdf", std::process::id()));
	doc.save_to_file(&path).unwrap();
	path
}

fn live_turn(ch: &Channel) -> ChannelTurn {
	ChannelTurn {
		channel: *ch,
		exe: channel::find_exe(*ch).expect("CLI not installed"),
		model: std::env::var("TAKEOFF_LIVE_MODEL").unwrap_or_else(|_| if *ch == Channel::ClaudeCode { "haiku".into() } else { String::new() }),
		workdir: std::env::temp_dir().join("takeoff-channel-live"),
		workspace: std::env::var_os("TAKEOFF_LIVE_WORKSPACE").map(std::path::PathBuf::from),
		others: vec![],
		attachments: vec![],
		sweeps: None,
		current_page: 1,
		visible_region: None,
		selected_ids: vec![],
	}
}

#[test]
#[ignore]
fn marks_squares_through_the_cli() {
	let ch = match std::env::var("TAKEOFF_LIVE_CHANNEL").as_deref() {
		Ok("codex") => Channel::Codex,
		Ok("opencode") => Channel::OpenCode,
		_ => Channel::ClaudeCode,
	};
	let exe = channel::find_exe(ch).expect("CLI installed");
	assert!(std::env::var_os("TAKEOFF_BRIDGE_EXE").is_some(), "set TAKEOFF_BRIDGE_EXE to a built takeoff-desk");
	let pdf = fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0)], "AI")));
	let host = ToolHost::start(doc.clone(), pdf.clone()).unwrap();
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let turn = ChannelTurn { exe, ..live_turn(&ch) };
	let h = channel::start_turn(&host, session.clone(), turn, "There are black squares on page 1. View the page, then mark every black square as a count in a new item called \"Squares\". Reply with how many you marked.".into());
	let mut text = String::new();
	let mut tools = vec![];
	let mut errors = vec![];
	loop {
		match h.events.recv_timeout(Duration::from_secs(600)).expect("turn timed out") {
			AgentEvent::TextDelta(t) => {
				print!("{t}");
				text.push_str(&t);
			}
			AgentEvent::ToolStarted { name, args, .. } => println!("\n→ {name} {args}"),
			AgentEvent::ToolFinished { name, ok, summary, .. } => {
				println!("← {name} ok={ok} {summary}");
				tools.push(name);
			}
			AgentEvent::Error(e) => errors.push(e),
			AgentEvent::Done { steps } => {
				println!("\n[done: {steps} steps]");
				break;
			}
			_ => {}
		}
	}
	assert!(errors.is_empty(), "errors: {errors:?}");
	assert!(tools.iter().any(|t| t == "view"), "never looked: {tools:?}");
	let marks = doc.lock().data.marks.iter().filter(|m| m.page == 1).count();
	println!("marks on page 1: {marks}");
	assert_eq!(marks, 3, "expected the 3 squares marked");
	let thread = session.lock().external.clone().expect("no resumable thread recorded");
	assert_eq!(thread.channel, ch.key());
	// Second turn resumes the same conversation: it remembers without looking again.
	let turn = ChannelTurn { exe: channel::find_exe(ch).unwrap(), ..live_turn(&ch) };
	let h = channel::start_turn(&host, session.clone(), turn, "Without using any tools: what did you call the item you just created? Answer with the name only.".into());
	let mut answer = String::new();
	loop {
		match h.events.recv_timeout(Duration::from_secs(600)).expect("turn timed out") {
			AgentEvent::TextDelta(t) => answer.push_str(&t),
			AgentEvent::Error(e) => panic!("{e}"),
			AgentEvent::Done { .. } => break,
			_ => {}
		}
	}
	println!("second turn: {answer}");
	assert!(answer.contains("Squares"), "did not resume: {answer}");
	assert_eq!(session.lock().external.as_ref(), Some(&thread));
	std::fs::remove_file(pdf).ok();
}

/// With the workspace on, the CLI's own shell writes a file there and the
/// drawing tools lay an overlay on the sheet.
#[test]
#[ignore]
fn shell_and_overlay_through_the_cli() {
	let ch = match std::env::var("TAKEOFF_LIVE_CHANNEL").as_deref() {
		Ok("codex") => Channel::Codex,
		Ok("opencode") => Channel::OpenCode,
		_ => Channel::ClaudeCode,
	};
	assert!(std::env::var_os("TAKEOFF_BRIDGE_EXE").is_some(), "set TAKEOFF_BRIDGE_EXE to a built takeoff-desk");
	let ws = std::env::temp_dir().join(format!("takeoff-live-ws-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&ws);
	std::fs::create_dir_all(&ws).unwrap();
	let pdf = fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0)], "AI")));
	let host = ToolHost::start(doc.clone(), pdf.clone()).unwrap();
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let turn = ChannelTurn { exe: channel::find_exe(ch).expect("CLI installed"), workspace: Some(ws.clone()), ..live_turn(&ch) };
	let h = channel::start_turn(
		&host,
		session,
		turn,
		"Two steps. 1) Use your shell to write the text 42 into a file named answer.txt in your working directory. 2) Add an overlay over the whole of page 1: an SVG with viewBox 0 0 4 3 containing one red circle, label it \"Test\". Then reply done.".into(),
	);
	let mut errors = vec![];
	loop {
		match h.events.recv_timeout(Duration::from_secs(600)).expect("turn timed out") {
			AgentEvent::TextDelta(t) => print!("{t}"),
			AgentEvent::ToolStarted { name, args, .. } => println!("\n→ {name} {args}"),
			AgentEvent::ToolFinished { name, ok, summary, .. } => println!("← {name} ok={ok} {summary}"),
			AgentEvent::Error(e) => errors.push(e),
			AgentEvent::Done { .. } => break,
			_ => {}
		}
	}
	assert!(errors.is_empty(), "errors: {errors:?}");
	let answer = std::fs::read_to_string(ws.join("answer.txt")).expect("shell wrote answer.txt in the workspace");
	assert_eq!(answer.trim(), "42");
	let d = doc.lock();
	assert_eq!(d.data.overlays.len(), 1, "one overlay added");
	assert_eq!(d.data.overlays[0].label.as_deref(), Some("Test"));
	std::fs::remove_file(pdf).ok();
}

/// An attached view: the model's first look is at that area.
#[test]
#[ignore]
fn looks_at_the_attached_view_first() {
	assert!(std::env::var_os("TAKEOFF_BRIDGE_EXE").is_some(), "set TAKEOFF_BRIDGE_EXE to a built takeoff-desk");
	let ch = Channel::ClaudeCode;
	let pdf = fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0)], "AI")));
	let host = ToolHost::start(doc.clone(), pdf.clone()).unwrap();
	let region = (0.3, 0.1, 0.25, 0.3);
	let turn = ChannelTurn {
		exe: channel::find_exe(ch).expect("CLI installed"),
		attachments: vec![takeoff_agent::ViewAttachment { drawing: pdf.clone(), page: 1, region }],
		..live_turn(&ch)
	};
	let h = channel::start_turn(&host, Arc::new(Mutex::new(ChatSession::new())), turn, "How many black squares are in the area I attached? Answer with the number only.".into());
	let mut first_view = None;
	let mut text = String::new();
	loop {
		match h.events.recv_timeout(Duration::from_secs(600)).expect("turn timed out") {
			AgentEvent::TextDelta(t) => text.push_str(&t),
			AgentEvent::ToolStarted { name, args, .. } => {
				println!("→ {name} {args}");
				if name == "view" && first_view.is_none() {
					first_view = Some(args);
				}
			}
			AgentEvent::Error(e) => panic!("{e}"),
			AgentEvent::Done { .. } => break,
			_ => {}
		}
	}
	println!("answer: {text}");
	let v = first_view.expect("looked at it");
	let x = v["region"]["x"].as_f64().expect("viewed a region, not the whole page");
	assert!((x - region.0).abs() < 0.02, "first view at the attached area: {v}");
	assert!(text.contains('1'), "one square (at 300,100) is in that area: {text}");
	std::fs::remove_file(pdf).ok();
}

/// Install / Sign in open a visible terminal running the command.
#[test]
#[ignore]
fn opens_a_terminal_that_runs_the_command() {
	let marker = std::env::temp_dir().join(format!("takeoff-terminal-{}", std::process::id()));
	let _ = std::fs::remove_file(&marker);
	channel::open_in_terminal(&format!("touch '{}'", marker.display())).unwrap();
	let t = std::time::Instant::now();
	while !marker.exists() && t.elapsed() < Duration::from_secs(20) {
		std::thread::sleep(Duration::from_millis(200));
	}
	assert!(marker.exists(), "the terminal ran the command");
	std::fs::remove_file(marker).ok();
}
