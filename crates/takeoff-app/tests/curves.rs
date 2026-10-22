//! Curves by hand: the Trace Line tool follows a dashed curve between the
//! points clicked; the Curve tools (measure and markup) curve through them. `CURVE_SNAP_DIR=… ` writes screenshots.

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::Settings;
use takeoff_app::tools::Tool;
use takeoff_core::geometry::polyline_length_px;
use takeoff_core::Point;

type H = Harness<'static, Option<TakeoffApp>>;

const W: f64 = 792.0;
const H_PT: f64 = 612.0;
/// A quarter circle (top-down page pt): centre (200, 100), radius 400.
const R: f64 = 400.0;

fn arc(deg: f64, r: f64) -> (f64, f64) {
	let a = deg.to_radians();
	(200.0 + r * a.cos(), 100.0 + r * a.sin())
}

/// The track as one dashed Bézier quarter circle, a solid rail 5 pt outside
/// it, and a road crossing both.
fn make_pdf(path: &std::path::Path) {
	let k = 0.552_284_75;
	let curve = |r: f64| {
		let f = |x: f64, y: f64| format!("{:.2} {:.2}", x, H_PT - y);
		format!("{} m {} {} {} c S\n", f(200.0 + r, 100.0), f(200.0 + r, 100.0 + k * r), f(200.0 + k * r, 100.0 + r), f(200.0, 100.0 + r))
	};
	let (a, b) = (arc(45.0, R - 80.0), arc(45.0, R + 80.0));
	let c = format!("0 0 0 RG 0.8 w [6 3] 0 d\n{}[] 0 d\n{}{:.1} {:.1} m {:.1} {:.1} l S\n", curve(R), curve(R + 5.0), a.0, H_PT - a.1, b.0, H_PT - b.1);
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
				s.snap = false;
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

fn pump(h: &mut H, n: usize) {
	for _ in 0..n {
		h.step();
	}
}

fn press(h: &mut H, pos: Pos2) {
	h.event(Event::PointerMoved(pos));
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
	h.step();
	h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
	pump(h, 6);
}

fn snap(h: &mut H, name: &str) {
	if let Ok(dir) = std::env::var("CURVE_SNAP_DIR") {
		h.render().expect("render").save(format!("{dir}/{name}.png")).unwrap();
	}
}

#[test]
fn length_tool_follows_and_smooths_curves() {
	let dir = std::env::temp_dir().join(format!("takeoff-curves-ui-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Track.pdf");
	make_pdf(&pdf);
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	pump(&mut h, 20);
	let screen = |h: &mut H, deg: f64, r: f64| {
		let (x, y) = arc(deg, r);
		app(h).debug_doc().unwrap().to_screen(Point { x: x / W, y: y / H_PT })
	};
	let expected = R * 86.0_f64.to_radians();
	let mut got = Vec::new();
	for tool in [Tool::Length, Tool::Trace, Tool::Curve, Tool::CurveMarkup] {
		app(&mut h).debug_canvas().set_tool(tool);
		// Three clicks along the curve, a little off it, as a hand would.
		for (deg, r) in [(2.0, R + 1.0), (45.0, R - 1.0)] {
			let p = screen(&mut h, deg, r);
			press(&mut h, p);
		}
		let end = screen(&mut h, 88.0, R + 1.0);
		press(&mut h, end);
		if tool == Tool::Trace {
			h.event(Event::PointerMoved(end + egui::vec2(30.0, 10.0)));
			pump(&mut h, 3);
			snap(&mut h, "curve-drawing");
		}
		// Double-click ends the run.
		press(&mut h, end);
		h.event(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
		h.step();
		h.event(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
		pump(&mut h, 6);
		let d = app(&mut h).debug_doc().unwrap();
		let doc = d.doc.lock();
		let points = if tool == Tool::CurveMarkup {
			let a = doc.data.annotations.last().expect("a curve markup");
			assert_eq!(a.kind, takeoff_core::AnnotationKind::Polyline);
			a.points.clone()
		} else {
			doc.data.measurements.last().expect("a measurement").points.clone()
		};
		let m = takeoff_core::Measurement::new("x", 1, takeoff_core::MeasurementType::Length, points);
		got.push(polyline_length_px(&m.points, W, H_PT));
		let radii: Vec<f64> = m.points.iter().map(|q| (q.x * W - 200.0).hypot(q.y * H_PT - 100.0)).collect();
		let ang = |q: &Point| (q.y * H_PT - 100.0).atan2(q.x * W - 200.0).to_degrees();
		println!("{tool:?}: {} pts, r {:.1}..{:.1}, from {:.1}° to {:.1}°", m.points.len(), radii.iter().cloned().fold(f64::MAX, f64::min), radii.iter().cloned().fold(0.0, f64::max), ang(&m.points[0]), ang(m.points.last().unwrap()));
	}
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	pump(&mut h, 4);
	snap(&mut h, "curve-drawn");
	let (straight, follow, smooth, markup) = (got[0], got[1], got[2], got[3]);
	println!("curve {expected:.1} pt · straight {straight:.1} · trace {follow:.1} · curve {smooth:.1} · curve markup {markup:.1}");
	assert!((markup - smooth).abs() < 0.5, "the curve markup is the same curve");
	assert!((follow - expected).abs() < 2.0, "along the line: {follow:.1} vs {expected:.1}");
	assert!((smooth - expected).abs() < (straight - expected).abs() / 2.0, "smooth {smooth:.1}, straight {straight:.1}");
	let _ = std::fs::remove_dir_all(&dir);
}
