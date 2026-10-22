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
		images: vec![],
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
		images: vec![],
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

/// A sheet: its title-block text, texts (text, x, y, size) and rectangles, in pt.
type SheetSpec<'a> = (&'a str, &'a [(&'a str, f32, f32, f32)], &'a [(f32, f32, f32, f32)]);

/// A small set with real sheet text: plan, details, framing, notes.
fn set_fixture() -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let font = doc.fonts_mut().helvetica_bold();
	let sheets: [SheetSpec; 4] = [
		(
			"A-101  FIRST FLOOR PLAN",
			&[("1  FIRST FLOOR PLAN  1/8\" = 1'-0\"", 60.0, 560.0, 12.0), ("3/A-501", 330.0, 300.0, 10.0), ("A/A-501", 470.0, 200.0, 10.0), ("SEE S-201 FOR FRAMING", 80.0, 120.0, 9.0), ("STAIR 1", 300.0, 340.0, 9.0), ("NOTE 4: ALL GWB 5/8\" TYPE X, SEE A-601", 80.0, 100.0, 9.0)],
			&[(60.0, 90.0, 520.0, 450.0), (320.0, 290.0, 40.0, 25.0), (460.0, 190.0, 40.0, 25.0)],
		),
		("A-501  DETAILS", &[("1  TREAD NOSING", 60.0, 330.0, 11.0), ("3  HANDRAIL BRACKET", 330.0, 330.0, 11.0), ("A  WALL SECTION", 60.0, 60.0, 11.0)], &[(60.0, 350.0, 240.0, 200.0), (330.0, 350.0, 240.0, 200.0), (60.0, 80.0, 240.0, 220.0)]),
		("S-201  SECOND FLOOR FRAMING PLAN", &[("1  FRAMING PLAN", 60.0, 560.0, 12.0), ("W12x26 TYP.", 200.0, 300.0, 9.0)], &[(60.0, 90.0, 520.0, 450.0)]),
		("A-601  GENERAL NOTES AND DOOR SCHEDULE", &[("GENERAL NOTES", 60.0, 560.0, 12.0), ("4. ALL GYPSUM BOARD 5/8\" TYPE X UNLESS NOTED.", 60.0, 530.0, 9.0), ("DOOR SCHEDULE", 330.0, 560.0, 12.0), ("101  3'-0\" x 7'-0\"  HM", 330.0, 530.0, 9.0)], &[(320.0, 400.0, 260.0, 180.0)]),
	];
	for (title, texts, rects) in sheets {
		let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
		// Title block on the right edge.
		page.objects_mut().create_path_object_rect(PdfRect::new_from_values(20.0, 620.0, 592.0, 772.0), Some(PdfColor::new(0, 0, 0, 255)), Some(PdfPoints::new(1.0)), None).unwrap();
		page.objects_mut().create_text_object(PdfPoints::new(630.0), PdfPoints::new(60.0), title, font, PdfPoints::new(9.0)).unwrap();
		for (x, y, w, h) in rects {
			page.objects_mut().create_path_object_rect(PdfRect::new_from_values(*y, *x, y + h, x + w), Some(PdfColor::new(0, 0, 0, 255)), Some(PdfPoints::new(1.0)), None).unwrap();
		}
		for (t, x, y, size) in texts {
			page.objects_mut().create_text_object(PdfPoints::new(*x), PdfPoints::new(*y), t, font, PdfPoints::new(*size)).unwrap();
		}
	}
	let path = std::env::temp_dir().join(format!("takeoff-live-set-{}.pdf", std::process::id()));
	doc.save_to_file(&path).unwrap();
	let _ = takeoff_core::docmap::delete(&path);
	path
}

/// The whole mapping path: the CLI surveys, hands sheets to its own
/// subagents (which use the drawing tools over MCP), and the map is written.
#[test]
#[ignore]
fn maps_a_small_set_through_the_cli() {
	let ch = Channel::ClaudeCode;
	assert!(std::env::var_os("TAKEOFF_BRIDGE_EXE").is_some(), "set TAKEOFF_BRIDGE_EXE to a built takeoff-desk");
	let pdf = set_fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0); 4], "AI")));
	let host = ToolHost::start(doc.clone(), pdf.clone()).unwrap();
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let model = std::env::var("TAKEOFF_LIVE_MODEL").unwrap_or_else(|_| "sonnet".into());
	let turn = ChannelTurn { exe: channel::find_exe(ch).expect("CLI installed"), model, ..live_turn(&ch) };
	let h = channel::start_turn(&host, session, turn, "Map this drawing set: build the document map.\n\nTo test parallel mapping, hand the sheets to two subagents after the survey (pages 1-2 and 3-4).".into());
	let mut tools = vec![];
	let mut errors = vec![];
	let started = std::time::Instant::now();
	loop {
		match h.events.recv_timeout(Duration::from_secs(1200)).expect("turn timed out") {
			AgentEvent::TextDelta(t) => print!("{t}"),
			AgentEvent::ToolStarted { name, .. } => println!("\n→ {name}"),
			AgentEvent::ToolFinished { name, ok, summary, .. } => {
				println!("← {name} ok={ok} {}", summary.chars().take(160).collect::<String>());
				tools.push(name);
			}
			AgentEvent::Error(e) => errors.push(e),
			AgentEvent::Done { steps } => {
				println!("\n[done: {steps} steps in {:.0}s]", started.elapsed().as_secs_f32());
				break;
			}
			_ => {}
		}
	}
	let map = takeoff_core::docmap::load(&pdf).expect("a map was written");
	println!("\n{}", serde_json::to_string_pretty(&map).unwrap());
	assert!(errors.is_empty(), "errors: {errors:?}");
	assert_eq!(map.sheets.len(), 4, "every page indexed");
	assert_eq!(map.pages_mapped().len(), 4, "every page mapped");
	assert_eq!(map.find_sheet("A-501").map(|s| s.page), Some(2));
	assert!(map.links.iter().any(|l| takeoff_core::docmap::page_of(&l.from) == Some(1) && takeoff_core::docmap::page_of(&l.to) == Some(2)), "the plan's callouts lead to the details: {:?}", map.links);
	std::fs::remove_file(pdf).ok();
}
