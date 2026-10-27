//! Spaces and legends through the real UI: draw a space, fill another, count
//! in and out of them, place a legend, move and resize it, and save.
//! `SPACES_SNAP_DIR=…` writes a screenshot.

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::Settings;
use takeoff_app::tools::Tool;
use takeoff_core::list::rows;
use takeoff_core::Point;

type H = Harness<'static, Option<TakeoffApp>>;

const W: f64 = 792.0;
const H_PT: f64 = 612.0;

/// Two rooms (top-down pt): x 100..400 and 400..600, y 150..350, the wall
/// between with a doorway at y 236..263.
fn make_pdf(path: &std::path::Path) {
	let l = |a: (f64, f64), b: (f64, f64)| format!("{:.1} {:.1} m {:.1} {:.1} l S\n", a.0, H_PT - a.1, b.0, H_PT - b.1);
	let c = ["0 0 0 RG 0.8 w\n".to_string(), l((100.0, 150.0), (600.0, 150.0)), l((100.0, 350.0), (600.0, 350.0)), l((100.0, 150.0), (100.0, 350.0)), l((600.0, 150.0), (600.0, 350.0)), l((400.0, 150.0), (400.0, 236.0)), l((400.0, 263.0), (400.0, 350.0))].concat();
	let objects = [
		String::new(),
		"<< /Type /Catalog /Pages 2 0 R >>".to_string(),
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
		format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H_PT}] /Contents 4 0 R >>"),
		format!("<< /Length {} >>\nstream\n{c}endstream", c.len()),
	];
	let mut out = String::from("%PDF-1.4\n");
	let mut offsets = vec![0usize; objects.len()];
	for (n, o) in objects.iter().enumerate().skip(1) {
		offsets[n] = out.len();
		out.push_str(&format!("{n} 0 obj\n{o}\nendobj\n"));
	}
	let xref = out.len();
	out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len()));
	for off in offsets.iter().skip(1) {
		out.push_str(&format!("{off:010} 00000 n \n"));
	}
	out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len()));
	std::fs::write(path, out).unwrap();
}

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
	pump(h, 25);
}

fn double_click(h: &mut H, pos: Pos2) {
	for _ in 0..2 {
		h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
		h.step();
		h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
		h.step();
	}
	pump(h, 25);
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

/// Screen point of a page position in PDF points (top-down).
fn at(h: &mut H, x: f64, y: f64) -> Pos2 {
	app(h).debug_doc().unwrap().to_screen(Point { x: x / W, y: y / H_PT })
}

#[test]
fn spaces_and_legends_in_the_ui() {
	let dir = std::env::temp_dir().join(format!("takeoff-spaces-ui-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Rooms.pdf");
	make_pdf(&pdf);
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(60) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	pump(&mut h, 20);

	// Space tool: the left room's four corners, double-click to close.
	app(&mut h).debug_canvas().set_tool(Tool::Space);
	for (x, y) in [(100.0, 150.0), (400.0, 150.0), (400.0, 350.0)] {
		let p = at(&mut h, x, y);
		click(&mut h, p);
	}
	let p = at(&mut h, 100.0, 350.0);
	double_click(&mut h, p);
	let left = {
		let d = app(&mut h).debug_doc().unwrap();
		let doc = d.doc.lock();
		assert_eq!(doc.data.spaces.len(), 1, "space drawn");
		assert_eq!(doc.data.spaces[0].name, "Space 1");
		assert_eq!(doc.data.spaces[0].points.len(), 4);
		let id = doc.data.spaces[0].id.clone();
		assert!(d.selection.contains(&id), "the new space is selected");
		id
	};
	app(&mut h).debug_doc().unwrap().doc.lock().edit("t", |d| d.spaces[0].name = "Living".into());

	// Fill Space: one click in the right room outlines it along its walls.
	app(&mut h).debug_canvas().set_tool(Tool::SpaceFill);
	let p = at(&mut h, 500.0, 300.0);
	click(&mut h, p);
	let t = Instant::now();
	while app(&mut h).debug_doc().unwrap().doc.lock().data.spaces.len() < 2 {
		assert!(t.elapsed() < Duration::from_secs(60), "no space filled");
		h.step();
		std::thread::sleep(Duration::from_millis(10));
	}
	{
		let doc = app(&mut h).debug_doc().unwrap().doc.lock();
		let b = takeoff_core::geometry::bounds(&doc.data.spaces[1].points).unwrap();
		assert!((b.0 * W - 400.0).abs() < 3.0 && ((b.0 + b.2) * W - 600.0).abs() < 3.0, "outlined along the walls: {b:?}");
	}

	// Counts: two in the left room, one in the right, one outside both.
	app(&mut h).debug_canvas().set_tool(Tool::Count);
	for (x, y) in [(200.0, 200.0), (300.0, 300.0), (500.0, 200.0), (700.0, 500.0)] {
		let p = at(&mut h, x, y);
		click(&mut h, p);
	}
	{
		let doc = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(doc.data.marks.len(), 4);
		let spaces: Vec<String> = rows(&doc).iter().map(|r| r.space.clone()).collect();
		assert_eq!(spaces, ["Living", "Living", "Space 2", ""]);
		let groups = takeoff_core::list::summary_by_space(&doc);
		assert_eq!(groups.iter().map(|g| (g.name.as_str(), g.lines[0].items)).collect::<Vec<_>>(), [("Living", 2), ("Space 2", 1), ("No space", 1)]);
	}

	// Legend tool: drag a box below the rooms.
	app(&mut h).debug_canvas().set_tool(Tool::Legend);
	let (a, b) = (at(&mut h, 450.0, 420.0), at(&mut h, 700.0, 560.0));
	drag(&mut h, a, b);
	let (lid, l0) = {
		let d = app(&mut h).debug_doc().unwrap();
		let doc = d.doc.lock();
		assert_eq!(doc.data.legends.len(), 1, "legend placed");
		let l = doc.data.legends[0].clone();
		assert!(d.selection.contains(&l.id), "and selected");
		let r = takeoff_core::legend::rows(&doc, &l);
		assert_eq!(r.len(), 1);
		assert_eq!((r[0].name.as_str(), r[0].count), ("Count 1", 4));
		(l.id.clone(), l)
	};

	// Select: drag the legend's body to move it, then its corner to resize it.
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	let (a, b) = (at(&mut h, 600.0, 500.0), at(&mut h, 550.0, 480.0));
	drag(&mut h, a, b);
	let l1 = app(&mut h).debug_doc().unwrap().doc.lock().data.legends[0].clone();
	assert!(((l1.x - l0.x) * W + 50.0).abs() < 2.0 && ((l1.y - l0.y) * H_PT + 20.0).abs() < 2.0, "moved: {:?} → {:?}", (l0.x, l0.y), (l1.x, l1.y));
	assert!((l1.w - l0.w).abs() < 1e-9, "same size");
	let corner = app(&mut h).debug_doc().unwrap().to_screen(Point { x: l1.x + l1.w, y: l1.y + l1.h });
	drag(&mut h, corner, corner + egui::vec2(40.0, 30.0));
	let l2 = app(&mut h).debug_doc().unwrap().doc.lock().data.legends[0].clone();
	assert!(l2.w > l1.w && l2.h > l1.h && (l2.x - l1.x).abs() < 1e-9, "resized: {:?} → {:?}", (l1.w, l1.h), (l2.w, l2.h));

	// A count added later shows in the legend at once.
	app(&mut h).debug_canvas().set_tool(Tool::Count);
	let p = at(&mut h, 250.0, 250.0);
	click(&mut h, p);
	{
		let doc = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(takeoff_core::legend::rows(&doc, &doc.data.legends[0])[0].count, 5);
	}
	if let Ok(d) = std::env::var("SPACES_SNAP_DIR") {
		app(&mut h).debug_canvas().set_tool(Tool::Select);
		pump(&mut h, 4);
		h.render().expect("render").save(format!("{d}/spaces-ui.png")).unwrap();
		app(&mut h).debug_right_tab(0);
		pump(&mut h, 4);
		h.render().expect("render").save(format!("{d}/spaces-takeoff.png")).unwrap();
	}

	// Saved with the takeoff.
	app(&mut h).debug_doc().unwrap().save_now();
	let saved = takeoff_core::sidecar::load(&pdf).unwrap();
	assert_eq!(saved.spaces.len(), 2);
	assert_eq!(saved.spaces[0].id, left);
	assert_eq!(saved.legends[0].id, lid);

	// Clicking the left space's outline selects it; Delete removes it, quantities stay.
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	let p = at(&mut h, 250.0, 150.0);
	click(&mut h, p);
	assert!(app(&mut h).debug_doc().unwrap().selection.contains(&left), "space picked by its outline");
	h.event(Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
	pump(&mut h, 2);
	{
		let doc = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(doc.data.spaces.len(), 1);
		assert_eq!(doc.data.marks.len(), 5);
		assert_eq!(rows(&doc)[0].space, "");
	}
	let _ = std::fs::remove_dir_all(&dir);
}
