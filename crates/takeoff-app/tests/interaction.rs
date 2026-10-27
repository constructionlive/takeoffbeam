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
	harness_sized(900.0)
}

fn harness_sized(height: f32) -> H {
	Harness::builder().with_size(egui::vec2(1440.0, height)).with_step_dt(1.0 / 60.0).build_ui_state(
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

/// Select tool: dragging empty sheet box-selects (Bluebeam), a trackpad pans
/// while a mouse wheel zooms.
#[test]
fn select_drag_box_selects_and_trackpad_pans() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui-nav-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(copy, &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	pump(&mut h, 5);
	let c = app(&mut h).debug_doc().unwrap().canvas_rect.center();

	// One mark to select.
	app(&mut h).debug_canvas().set_tool(Tool::Count);
	click(&mut h, c);
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	click(&mut h, c + egui::vec2(200.0, 150.0)); // empty: clears selection
	assert!(app(&mut h).debug_doc().unwrap().selection.is_empty());

	// A plain drag around the mark box-selects it, without moving the view.
	let pan0 = app(&mut h).debug_doc().unwrap().pan;
	drag(&mut h, c - egui::vec2(60.0, 60.0), c + egui::vec2(60.0, 60.0));
	pump(&mut h, 2);
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.selection.len(), 1, "box-selected the mark");
	assert_eq!(d.pan, pan0, "a drag in Select doesn't pan");

	// A drag over empty sheet selects nothing.
	drag(&mut h, c + egui::vec2(150.0, 100.0), c + egui::vec2(250.0, 160.0));
	pump(&mut h, 2);
	assert!(app(&mut h).debug_doc().unwrap().selection.is_empty());

	// Trackpad (point deltas) pans; a mouse wheel (line deltas) zooms.
	let (pan1, zoom1) = { let d = app(&mut h).debug_doc().unwrap(); (d.pan, d.zoom) };
	h.event(Event::PointerMoved(c));
	h.event(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -80.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE });
	pump(&mut h, 30);
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.zoom, zoom1, "trackpad scroll does not zoom");
	assert!((d.pan.y - pan1.y).abs() > 20.0, "trackpad scroll pans: {:?} → {:?}", pan1, d.pan);
	h.event(Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, 3.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE });
	pump(&mut h, 30);
	assert!(app(&mut h).debug_doc().unwrap().zoom > zoom1 * 1.05, "mouse wheel zooms");
}

/// Follow the AI: the canvas goes to the tab, page and area it looks at;
/// with Follow off, it stays put.
#[test]
fn follow_the_ai_switches_tab_and_zooms() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui-follow-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let (a, b) = (dir.join("A.pdf"), dir.join("B.pdf"));
	std::fs::copy(&pdf, &a).unwrap();
	std::fs::copy(&pdf, &b).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(a.clone(), &ctx);
	app(&mut h).open(b.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	pump(&mut h, 5);
	assert_eq!(app(&mut h).debug_active_path(), Some(b.clone()));

	// Follow off: nothing moves.
	app(&mut h).settings.follow_agent = false;
	app(&mut h).debug_agent_looked(a.clone(), 1, (0.4, 0.4, 0.1, 0.1));
	pump(&mut h, 2);
	assert_eq!(app(&mut h).debug_active_path(), Some(b.clone()));

	// Follow on: switches to A and zooms in on that area.
	app(&mut h).settings.follow_agent = true;
	let zoom_before = app(&mut h).debug_doc().unwrap().zoom;
	app(&mut h).debug_agent_looked(a.clone(), 1, (0.4, 0.4, 0.1, 0.1));
	pump(&mut h, 2);
	assert_eq!(app(&mut h).debug_active_path(), Some(a.clone()), "switched to the drawing the AI looked at");
	let d = app(&mut h).debug_doc().unwrap();
	assert!(d.zoom > zoom_before * 2.0, "zoomed in on the area: {zoom_before} → {}", d.zoom);
	let centre = d.to_page(d.canvas_rect.center());
	assert!((centre.x - 0.45).abs() < 0.02 && (centre.y - 0.45).abs() < 0.02, "centred on it: {centre:?}");
	app(&mut h).settings.follow_agent = false;
}

fn click_with(h: &mut H, pos: Pos2, mods: Modifiers) {
	h.input_mut().modifiers = mods;
	h.event(Event::PointerMoved(pos));
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: mods });
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: mods });
	h.step();
	h.input_mut().modifiers = Modifiers::NONE;
	pump(h, 25);
}

/// Markups list: Shift-click selects a range, ⌘-click toggles, Select all
/// takes every shown row, Delete removes the selection.
#[test]
fn markups_list_multi_select() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui-list-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(copy, &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	pump(&mut h, 5);
	let c = app(&mut h).debug_doc().unwrap().canvas_rect.center();
	app(&mut h).debug_canvas().set_tool(Tool::Count);
	for k in 0..4 {
		click(&mut h, c + egui::vec2(k as f32 * 40.0, 0.0));
	}
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	pump(&mut h, 3);
	// The list's rows (the subject column), top to bottom — read again before
	// each click, since the selection bar above the table changes with the selection.
	let list_top = app(&mut h).debug_doc().unwrap().canvas_rect.bottom();
	let rows = |h: &mut H| -> Vec<Pos2> {
		let mut r: Vec<Pos2> = h.query_all_by_label("Count 1").map(|n| n.rect().center()).filter(|p| p.y > list_top).collect();
		r.sort_by(|a, b| a.y.total_cmp(&b.y));
		r
	};
	assert_eq!(rows(&mut h).len(), 4, "four rows in the list");
	let sel = |h: &mut H| app(h).debug_doc().unwrap().selection.len();

	let r = rows(&mut h)[0];
	click(&mut h, r);
	assert_eq!(sel(&mut h), 1);
	let r = rows(&mut h)[1];
	click_with(&mut h, r, Modifiers::SHIFT);
	assert_eq!(sel(&mut h), 2, "Shift-click selects the range from the first row");
	let r = rows(&mut h)[0];
	click_with(&mut h, r, Modifiers::COMMAND);
	assert_eq!(sel(&mut h), 1, "⌘-click takes a row out");
	let r = rows(&mut h)[0];
	click_with(&mut h, r, Modifiers::COMMAND);
	assert_eq!(sel(&mut h), 2, "⌘-click puts it back");
	let r = rows(&mut h)[1];
	click(&mut h, r);
	assert_eq!(sel(&mut h), 1, "a plain click selects just that row");

	// Buttons in the list's selection bar (below the canvas).
	let in_list = |h: &mut H, label: &str| -> Pos2 {
		h.query_all_by_label_contains(label).map(|n| n.rect().center()).find(|p| p.y > list_top).unwrap_or_else(|| panic!("no '{label}' button in the list"))
	};
	let b = in_list(&mut h, "Select all (4)");
	click(&mut h, b);
	assert_eq!(sel(&mut h), 4);
	in_list(&mut h, "4 selected");
	let b = in_list(&mut h, "Delete 4");
	click(&mut h, b);
	assert_eq!(app(&mut h).debug_doc().unwrap().doc.lock().data.marks.len(), 0, "deleted all four");
	assert_eq!(sel(&mut h), 0);
}

/// The AI prompt grows as you type and stays on screen.
#[test]
fn ai_prompt_grows_with_its_text() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui-prompt-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(copy, &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	app(&mut h).debug_right_tab(2);
	pump(&mut h, 5);
	let input = |h: &mut H| h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect();
	let before = input(&mut h);
	click(&mut h, before.center());
	h.event(Event::Text("Count every shoring tower on this sheet, then the post shores under the slab edge, and tell me which bays have no shoring at all so I can check them against the formwork plan before we price it. ".repeat(3)));
	pump(&mut h, 6);
	let after = input(&mut h);
	assert!(after.height() > before.height() * 2.0, "grew: {before:?} → {after:?}");
	assert!(after.bottom() < 900.0 - 30.0, "still above the status bar: {after:?}");
}

/// The menu bar opens its menus, with and without a drawing open.
#[test]
fn menu_bar_opens() {
	let mut h = harness();
	pump(&mut h, 5);
	let file = h.get_by_label("File").rect().center();
	click(&mut h, file);
	assert!(h.query_by_label_contains("Open recent").is_some(), "File menu opens with no drawing");
	h.key_press(egui::Key::Escape);
	pump(&mut h, 5);

	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui-menu-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();
	let ctx = h.ctx.clone();
	app(&mut h).open(copy, &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	for tab in [0, 1, 2] {
		app(&mut h).debug_right_tab(tab);
		pump(&mut h, 5);
		for menu in ["File", "Edit", "View"] {
			let r = h.get_by_label(menu).rect().center();
			click(&mut h, r);
			let item = match menu {
				"File" => "Open recent",
				"Edit" => "Undo",
				_ => "Fit width",
			};
			let open = h.query_by_label_contains(item).is_some();
			assert!(open, "{menu} menu opens with the right panel on tab {tab}");
			h.key_press(egui::Key::Escape);
			pump(&mut h, 5);
		}
	}
}

/// The AI tab's header: the model picker and its buttons don't overlap.
#[test]
fn ai_header_buttons_do_not_overlap_the_picker() {
	let pdf = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-ui-header-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let copy = dir.join("plan.pdf");
	std::fs::copy(&pdf, &copy).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(copy, &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	app(&mut h).debug_right_tab(2);
	pump(&mut h, 5);
	let picker = h.get_by_label_contains("Model:").rect();
	for tip in ["New conversation", "Past conversations", "Follow the AI"] {
		let b = h.get_by_label_contains(tip).rect();
		assert!(!b.intersects(picker), "'{tip}' button overlaps the picker: {b:?} vs {picker:?}");
		assert!((b.center().y - picker.center().y).abs() < 4.0, "'{tip}' sits in the picker's row");
	}
	assert!(picker.top() < 200.0, "the header stays at the top of the panel: {picker:?}");
	assert!(picker.height() <= 44.0, "one row: {picker:?}");
}

/// Last session's crash report: a corner notice that Dismiss archives; Help has the log items.
#[test]
fn crash_notice_dismisses_and_help_has_logs() {
	let dir = std::env::temp_dir().join(format!("takeoff-ui-crash-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	takeoff_app::crash::write(&dir, &takeoff_app::crash::report("boom 42", "a.rs:1:1", "main", "0: x")).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let report = takeoff_app::crash::pending(&dir).unwrap();
	app(&mut h).offer_crash_report(report, dir.clone());
	pump(&mut h, 3);
	assert!(h.query_by_label_contains("closed unexpectedly").is_some());
	assert!(h.query_by_label_contains("boom 42").is_some());
	let r = h.get_by_label("Dismiss").rect().center();
	click(&mut h, r);
	assert!(h.query_by_label_contains("closed unexpectedly").is_none());
	assert!(takeoff_app::crash::pending(&dir).is_none(), "dismissed report is archived");
	assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
	let _ = std::fs::remove_dir_all(&dir);

	let help = h.get_by_label("Help").rect().center();
	click(&mut h, help);
	assert!(h.query_by_label_contains("Copy diagnostics").is_some());
	assert!(h.query_by_label_contains("Show logs").is_some());
}
