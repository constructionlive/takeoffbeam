//! The Fill Area tool: a click inside a room measures the room, its doorway
//! closed; Shift-click on the room next door adds it. `FILL_SNAP_DIR=…`
//! writes a screenshot.

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::Settings;
use takeoff_app::tools::Tool;
use takeoff_core::geometry::polygon_area_px;
use takeoff_core::{MeasurementType, Point};

type H = Harness<'static, Option<TakeoffApp>>;

const W: f64 = 792.0;
const H_PT: f64 = 612.0;

/// Two rooms (top-down pt): x 100..400 and 400..600, y 150..350, the wall
/// between with a doorway at y 236..263 and a door swing.
fn make_pdf(path: &std::path::Path) {
	let l = |a: (f64, f64), b: (f64, f64)| format!("{:.1} {:.1} m {:.1} {:.1} l S\n", a.0, H_PT - a.1, b.0, H_PT - b.1);
	let c = ["0 0 0 RG 0.8 w\n".to_string(), l((100.0, 150.0), (600.0, 150.0)), l((100.0, 350.0), (600.0, 350.0)), l((100.0, 150.0), (100.0, 350.0)), l((600.0, 150.0), (600.0, 350.0)), l((400.0, 150.0), (400.0, 236.0)), l((400.0, 263.0), (400.0, 350.0)), l((400.0, 263.0), (427.0, 263.0))].concat();
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
	Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_pixels_per_point(1.0).with_step_dt(1.0 / 60.0).wgpu().build_ui_state(
		|ui, state: &mut Option<TakeoffApp>| {
			let Some(app) = state.as_mut() else {
				let mut s = Settings::default();
				s.markups_list = false;
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

fn click(h: &mut H, pos: Pos2, modifiers: Modifiers) {
	h.input_mut().modifiers = modifiers;
	h.event(Event::PointerMoved(pos));
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers });
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers });
	h.step();
	h.input_mut().modifiers = Modifiers::NONE;
}

/// Step until the fill is in (the worker makes it).
fn wait_for(h: &mut H, n: usize, area: &dyn Fn(&mut H) -> Option<f64>, last: Option<f64>) -> f64 {
	let t = Instant::now();
	loop {
		h.step();
		if let Some(a) = area(h).filter(|a| Some(*a) != last) {
			if app(h).debug_doc().unwrap().doc.lock().data.measurements.len() == n {
				println!("filled in {:?}", t.elapsed());
				return a;
			}
		}
		assert!(t.elapsed() < Duration::from_secs(20), "no fill");
		std::thread::sleep(Duration::from_millis(10));
	}
}

#[test]
fn a_click_fills_the_room() {
	let dir = std::env::temp_dir().join(format!("takeoff-fill-ui-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Rooms.pdf");
	make_pdf(&pdf);
	let mut h = harness();
	for _ in 0..2 {
		h.step();
	}
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	for _ in 0..20 {
		h.step();
	}
	let screen = |h: &mut H, x: f64, y: f64| app(h).debug_doc().unwrap().to_screen(Point { x: x / W, y: y / H_PT });
	let area = |h: &mut H| {
		let d = app(h).debug_doc().unwrap();
		let doc = d.doc.lock();
		doc.data.measurements.last().map(|m| {
			assert_eq!(m.kind, MeasurementType::Area);
			polygon_area_px(&m.points, W, H_PT)
		})
	};
	app(&mut h).debug_canvas().set_tool(Tool::Fill);
	let p = screen(&mut h, 200.0, 200.0);
	click(&mut h, p, Modifiers::NONE);
	let room = wait_for(&mut h, 1, &area, None);
	// The left room inside its walls; no scale here, so the doorway (27 pt)
	// closes at one of the paper widths.
	let want = (300.0 - 0.8) * (200.0 - 0.8);
	println!("room {room:.0} pt² vs {want:.0}");
	assert!((room - want).abs() < want * 0.01, "room {room:.0} vs {want:.0}");
	// Shift-click the room next door: one measurement, both rooms.
	let p = screen(&mut h, 500.0, 300.0);
	click(&mut h, p, Modifiers::SHIFT);
	let both = wait_for(&mut h, 1, &area, Some(room));
	let want = (500.0 - 0.8) * (200.0 - 0.8);
	println!("both {both:.0} pt² vs {want:.0}");
	assert!((both - want).abs() < want * 0.01, "both {both:.0} vs {want:.0}");
	if let Ok(d) = std::env::var("FILL_SNAP_DIR") {
		for _ in 0..4 {
			h.step();
		}
		h.render().expect("render").save(format!("{d}/fill-ui.png")).unwrap();
	}
	let _ = std::fs::remove_dir_all(&dir);
}
