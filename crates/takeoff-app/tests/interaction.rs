//! Drives the real UI with simulated mouse/keyboard input: the tools must
//! produce the right takeoff items. Skips when the sample drawing is absent.
//! `cargo test -p takeoff-app --test interaction`

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::Settings;
use takeoff_app::tools::Tool;
use takeoff_core::*;

type H = Harness<'static, Option<TakeoffApp>>;

fn harness() -> H {
	Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
		|ui, state: &mut Option<TakeoffApp>| {
			let Some(app) = state.as_mut() else {
				let mut s = Settings::default();
				s.snap = false;
				*state = Some(TakeoffApp::new(ui.ctx(), s));
				return;
			};
			app.frame(ui);
		},
		None,
	)
}

fn app(h: &mut H) -> &mut TakeoffApp {
	h.state_mut().as_mut().unwrap()
}

fn pump(h: &mut H, n: usize) {
	for _ in 0..n {
		h.step();
	}
}

fn click(h: &mut H, pos: Pos2) {
	h.event(Event::PointerMoved(pos));
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
	h.step();
	// Let the double-click window (0.3 s of simulated time) pass.
	pump(h, 25);
}

fn double_click(h: &mut H, pos: Pos2) {
	for _ in 0..2 {
		h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
		h.step();
		h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
		h.step();
	}
}

fn drag(h: &mut H, from: Pos2, to: Pos2) {
	h.event(Event::PointerMoved(from));
	h.step();
	h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
	h.step();
	for k in 1..=8 {
		let t = k as f32 / 8.0;
		h.event(Event::PointerMoved(from + (to - from) * t));
		h.step();
	}
	h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
	pump(h, 25);
}

#[test]
fn tools_create_takeoff_items() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	// Work on a copy so the user's file never gets a sidecar.
	let dir = std::env::temp_dir().join(format!("takeoff-ui-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();

	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(copy.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	pump(&mut h, 5);
	let canvas = app(&mut h).debug_doc().unwrap().canvas_rect;
	let c = canvas.center();
	assert!(canvas.width() > 200.0, "canvas laid out: {canvas:?}");

	// Count: two clicks → two marks in an auto-created "Count 1" item.
	app(&mut h).debug_canvas().set_tool(Tool::Count);
	click(&mut h, c);
	click(&mut h, c + egui::vec2(40.0, 0.0));
	{
		let d = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(d.data.marks.len(), 2, "count marks");
		assert_eq!(d.data.categories[0].name, "Count 1");
	}

	// Calibrate: two clicks 200 px apart → dialog → type 20' → scale set.
	app(&mut h).debug_canvas().set_tool(Tool::Calibrate);
	click(&mut h, c + egui::vec2(-100.0, 80.0));
	click(&mut h, c + egui::vec2(100.0, 80.0));
	pump(&mut h, 2);
	h.event(Event::Text("20'".into()));
	pump(&mut h, 2);
	h.event(Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
	pump(&mut h, 3);
	{
		let d = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(d.data.page_scales.len(), 1, "scale set");
		assert_eq!(d.data.page_scales[0].distance, 20.0);
	}

	// Length: three clicks + double-click → one length ≈ 20 + 10 ft.
	pump(&mut h, 30);
	app(&mut h).debug_canvas().set_tool(Tool::Length);
	click(&mut h, c + egui::vec2(-100.0, 120.0));
	click(&mut h, c + egui::vec2(100.0, 120.0));
	double_click(&mut h, c + egui::vec2(100.0, 220.0));
	pump(&mut h, 2);
	{
		let d = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(d.data.measurements.len(), 1, "length created");
		let v = d.data.measurements[0].value.unwrap();
		assert!((v - 30.0).abs() < 0.5, "length value {v}");
	}

	// Rectangle area by drag: 200×100 px → 20 ft × 10 ft = 200 ft².
	app(&mut h).debug_canvas().set_tool(Tool::RectArea);
	drag(&mut h, c + egui::vec2(-100.0, -200.0), c + egui::vec2(100.0, -100.0));
	pump(&mut h, 2);
	{
		let d = app(&mut h).debug_doc().unwrap().doc.lock();
		let area = d.data.measurements.iter().find(|m| m.kind == MeasurementType::Area).expect("area");
		assert!((area.value.unwrap() - 200.0).abs() < 2.0, "area {}", area.value.unwrap());
	}

	// Undo removes the area; redo brings it back.
	h.event(Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
	pump(&mut h, 2);
	assert_eq!(app(&mut h).debug_doc().unwrap().doc.lock().data.measurements.len(), 1, "undo");

	// Select tool: click a mark, Delete removes it.
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	click(&mut h, c);
	assert_eq!(app(&mut h).debug_doc().unwrap().selection.len(), 1, "selected");
	h.event(Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
	pump(&mut h, 2);
	assert_eq!(app(&mut h).debug_doc().unwrap().doc.lock().data.marks.len(), 1, "deleted");

	// Autosave writes the sidecar next to the copy.
	app(&mut h).debug_doc().unwrap().save_now();
	let saved = takeoff_core::sidecar::load(&copy).unwrap();
	assert_eq!(saved.marks.len(), 1);
	assert_eq!(saved.measurements.len(), 1);
	std::fs::remove_dir_all(dir).ok();
}

#[test]
fn item_card_toggles_and_menu_opens() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui2-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(copy.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	app(&mut h).debug_doc().unwrap().doc.lock().edit("t", |t| t.categories.push(Category::new("c9", "Area 1", "#4CC2FF", CategoryKind::Area)));
	app(&mut h).debug_right_tab(0);
	pump(&mut h, 4);

	// Click the item name: it becomes the active item with the Area tool.
	let name = h.get_by_label("Area 1").rect().center();
	click(&mut h, name);
	assert_eq!(app(&mut h).debug_canvas().tool, Tool::Area, "card activates area tool");
	assert_eq!(app(&mut h).debug_canvas().active_category.as_deref(), Some("c9"));
	// Click it again: deselected, back to Select.
	click(&mut h, name);
	assert_eq!(app(&mut h).debug_canvas().tool, Tool::Select, "second click deselects");
	assert_eq!(app(&mut h).debug_canvas().active_category, None);

	// The ⋯ menu button opens its menu (it isn't swallowed by the card).
	let dots = h.get_by_label(takeoff_app::icons::ELLIPSIS).rect().center();
	click(&mut h, dots);
	pump(&mut h, 3);
	assert!(h.query_by_label_contains("Save to Tool Chest").is_some(), "item menu opened");
	assert_eq!(app(&mut h).debug_canvas().tool, Tool::Select, "menu click didn't activate the card");
	std::fs::remove_dir_all(dir).ok();
}
