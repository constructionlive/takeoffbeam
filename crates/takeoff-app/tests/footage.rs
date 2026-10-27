//! Records real UI footage for the launch video: drives the app with
//! simulated input and saves every frame (plus the cursor position) as PNG.
//! `FOOTAGE_DIR=marketing/footage cargo test --release -p takeoff-app --test footage -- --ignored --nocapture`

use std::io::Write;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::{Settings, ThemeChoice};
use takeoff_app::tools::Tool;
use takeoff_core::*;

const PPP: f32 = 1.2; // 1600×900 points → 1920×1080 px
type H = Harness<'static, Option<TakeoffApp>>;

struct Rec {
	h: H,
	dir: std::path::PathBuf,
	clip: String,
	n: usize,
	cursor: Pos2,
	log: Vec<String>,
	cursor_kind: &'static str,
}

impl Rec {
	fn app(&mut self) -> &mut TakeoffApp {
		self.h.state_mut().as_mut().unwrap()
	}
	fn start(&mut self, clip: &str) {
		self.flush();
		self.clip = clip.into();
		self.n = 0;
		std::fs::create_dir_all(self.dir.join(clip)).unwrap();
	}
	fn flush(&mut self) {
		if !self.clip.is_empty() {
			let mut f = std::fs::File::create(self.dir.join(&self.clip).join("cursor.txt")).unwrap();
			for l in &self.log {
				writeln!(f, "{l}").unwrap();
			}
		}
		self.log.clear();
	}
	fn frame(&mut self) {
		self.h.step();
		let img = self.h.render().expect("render");
		img.save(self.dir.join(&self.clip).join(format!("{:04}.png", self.n))).unwrap();
		self.log.push(format!("{} {} {} {}", self.n, self.cursor.x * PPP, self.cursor.y * PPP, self.cursor_kind));
		self.n += 1;
	}
	fn hold(&mut self, frames: usize) {
		for _ in 0..frames {
			self.frame();
		}
	}
	/// Eased cursor move over `frames` frames.
	fn move_to(&mut self, to: Pos2, frames: usize) {
		let from = self.cursor;
		for k in 1..=frames {
			let t = k as f32 / frames as f32;
			let e = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
			self.cursor = from + (to - from) * e;
			self.h.event(Event::PointerMoved(self.cursor));
			self.frame();
		}
	}
	fn click(&mut self) {
		let pos = self.cursor;
		self.h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
		self.frame();
		self.h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
		self.log.push(format!("click {}", self.n));
		self.frame();
	}
	fn double_click(&mut self) {
		let pos = self.cursor;
		for _ in 0..2 {
			self.h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
			self.h.step();
			self.h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
			self.h.step();
		}
		self.log.push(format!("click {}", self.n));
		self.frame();
	}
	fn settle(&mut self, cond: impl Fn(&mut TakeoffApp) -> bool, max: Duration) {
		let t = Instant::now();
		while !cond(self.app()) && t.elapsed() < max {
			self.h.step();
			std::thread::sleep(Duration::from_millis(25));
		}
		for _ in 0..4 {
			self.h.step();
		}
	}
	fn screen(&mut self, p: Point) -> Pos2 {
		self.app().debug_doc().unwrap().to_screen(p)
	}
}

fn p(x: f64, y: f64) -> Point {
	Point { x, y }
}

#[test]
#[ignore]
fn launch_footage() {
	let pdf_src = std::path::PathBuf::from("/Users/office/Downloads/drawing.pdf");
	let set_src = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf_src.exists() || !set_src.exists() {
		return;
	}
	let dir = std::path::PathBuf::from(std::env::var("FOOTAGE_DIR").unwrap_or_else(|_| "/tmp/footage".into()));
	let work = std::env::temp_dir().join(format!("takeoff-footage-{}", std::process::id()));
	std::fs::create_dir_all(&work).unwrap();
	let pdf = work.join("S-201 Level 02 Shoring Plan.pdf");
	std::fs::copy(&pdf_src, &pdf).unwrap();
	let set = work.join("Cooper Middle School — Architectural Set.pdf");
	std::fs::copy(&set_src, &set).unwrap();

	let h = Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_pixels_per_point(PPP).with_step_dt(1.0 / 30.0).wgpu().build_ui_state(
		|ui, state: &mut Option<TakeoffApp>| {
			let Some(app) = state.as_mut() else {
				let mut s = Settings::default();
				s.theme = ThemeChoice::Dark;
				s.author = "Alex Rivera".into();
				s.snap = true;
				s.model = "local:Qwen3.8-27B-Q4_K_M".into();
				*state = Some(TakeoffApp::new(ui.ctx(), s));
				return;
			};
			app.frame(ui);
		},
		None,
	);
	let mut r = Rec { h, dir: dir.clone(), clip: String::new(), n: 0, cursor: Pos2::new(800.0, 450.0), log: vec![], cursor_kind: "arrow" };
	r.h.run_steps(3);
	let ctx = r.h.ctx.clone();
	r.app().open(pdf.clone(), &ctx);
	r.settle(|a| a.debug_doc().is_some_and(|d| d.loaded && d.base.contains_key(&1)), Duration::from_secs(20));
	{
		let d = r.app().debug_doc().unwrap();
		let (w, _) = d.page_size(1);
		// The sheet is drawn at 1:75.
		let scale = units::ScalePreset { label: "1:75", paper_in: 1000.0 / 25.4, real: 75.0, unit: "m" }.to_page_scale(1, w);
		d.doc.lock().edit("setup", |t| {
			t.page_scales.push(scale);
			let mut c = Category::new("c1", "Shore post 3 m", "#F87171", CategoryKind::Count);
			c.unit_cost = Some(42.0);
			t.categories.push(c);
			let mut a = Category::new("c2", "Slab pour 2", "#34D399", CategoryKind::Area);
			a.hatch = Some(Hatch::Diagonal);
			a.unit_cost = Some(65.0);
			t.categories.push(a);
		});
		d.zoom_to(1, (0.115, 0.06, 0.13, 0.08));
	}
	r.settle(|a| a.debug_doc().is_some_and(|d| d.sharp.is_some()), Duration::from_secs(8));
	r.app().debug_right_tab(0);
	r.app().debug_doc().unwrap().ensure_snap();
	r.settle(|a| a.debug_doc().is_some_and(|d| !d.snap.is_empty()), Duration::from_secs(10));

	// ── Clip 1: count by clicking ────────────────────────────────────
	r.start("count");
	r.app().debug_canvas().active_category = Some("c1".into());
	r.app().debug_canvas().set_tool(Tool::Count);
	r.cursor_kind = "cross";
	r.hold(8);
	for x in [0.1297, 0.1458, 0.1618, 0.1778, 0.1939, 0.2099] {
		let to = r.screen(p(x, 0.0792));
		r.move_to(to, 9);
		r.click();
		r.hold(2);
	}
	for x in [0.1297, 0.1458, 0.1618, 0.1778] {
		let to = r.screen(p(x, 0.1218));
		r.move_to(to, 9);
		r.click();
		r.hold(2);
	}
	r.hold(20);

	// ── Clip 2: area with snapping and a live quantity ───────────────
	r.start("area");
	r.app().debug_canvas().active_category = Some("c2".into());
	r.app().debug_canvas().set_tool(Tool::Area);
	r.hold(6);
	let corners = [p(0.1297, 0.0792), p(0.1778, 0.0792), p(0.1778, 0.1218), p(0.1458, 0.1218), p(0.1458, 0.1005), p(0.1297, 0.1005)];
	for (k, c) in corners.iter().enumerate() {
		// Approach a little off the corner: the snap marker pulls it in.
		let target = r.screen(*c) + egui::vec2(5.0, -4.0);
		r.move_to(target, if k == 0 { 14 } else { 18 });
		r.hold(4);
		if k + 1 == corners.len() {
			r.double_click();
		} else {
			r.click();
		}
	}
	r.app().debug_canvas().set_tool(Tool::Select);
	r.cursor_kind = "arrow";
	r.hold(36);

	// ── Clip 3: select + the markups list ────────────────────────────
	r.start("select");
	let to = r.screen(p(0.16, 0.09));
	r.move_to(to, 18);
	r.click();
	r.app().debug_right_tab(1);
	r.hold(40);

	// ── Clip 4: layers on / off ──────────────────────────────────────
	r.start("layers");
	r.app().debug_right_tab(0);
	{
		let d = r.app().debug_doc().unwrap();
		d.selection.clear();
		d.zoom_to(1, (0.02, 0.0, 0.42, 0.3));
	}
	r.app().debug_left_tab(2);
	r.settle(|a| a.debug_doc().is_some_and(|d| d.base.contains_key(&1) && d.sharp.is_some()), Duration::from_secs(8));
	r.hold(15);
	let hide: std::collections::HashSet<String> = r.app().debug_doc().unwrap().layers.iter().filter(|l| l.contains("ANNO") || l.contains("GRID")).cloned().collect();
	for hidden in [hide, Default::default()] {
		r.app().debug_doc().unwrap().set_hidden_layers(hidden);
		// The textures drop when the layers apply; wait for them to come back.
		r.settle(|a| a.debug_doc().is_some_and(|d| !d.base.contains_key(&1)), Duration::from_secs(3));
		r.settle(|a| a.debug_doc().is_some_and(|d| d.base.contains_key(&1) && d.sharp.is_some()), Duration::from_secs(8));
		r.hold(30);
	}

	// ── Clip 5: AI conversation (scripted transcript, real UI) ───────
	r.start("ai");
	{
		let path = r.app().debug_doc().unwrap().path.clone();
		let session = r.app().debug_session(&path);
		let json = serde_json::json!({ "messages": [
			{ "role": "user", "text": "Set the scale from a dimension and count the shore posts in the top-left bay." },
			{ "role": "assistant", "text": "", "tool_calls": [{ "id": "1", "name": "view", "arguments": "{}" }] },
			{ "role": "tool", "call_id": "1", "name": "view", "text": "v3 · page 1 · top-left bay rendered", "ok": true },
			{ "role": "assistant", "text": "", "tool_calls": [{ "id": "2", "name": "set_scale", "arguments": "{}" }] },
			{ "role": "tool", "call_id": "2", "name": "set_scale", "text": "Scale set from grid D–E = 6400 mm", "ok": true },
			{ "role": "assistant", "text": "", "tool_calls": [{ "id": "3", "name": "check_scale", "arguments": "{}" }] },
			{ "role": "tool", "call_id": "3", "name": "check_scale", "text": "3525 mm dimension measures 3610 mm · 2.4% · verified", "ok": true },
			{ "role": "assistant", "text": "", "tool_calls": [{ "id": "4", "name": "detect", "arguments": "{}" }] },
			{ "role": "tool", "call_id": "4", "name": "detect", "text": "18 shore posts found · saved as pending", "ok": true },
			{ "role": "assistant", "text": "**Scale set** from grid D–E (6400 mm) and checked against the 3525 mm dimension: **2.4% off — verified.**\n\nI found **18 shore posts** in the top-left bay. They're marked *to review* — badges 1–18. Two near the stair opening are partly hidden by the hatch; please check those." }
		]});
		*session.lock() = serde_json::from_value(json).unwrap();
	}
	r.app().debug_right_tab(2);
	{
		let d = r.app().debug_doc().unwrap();
		d.zoom_to(1, (0.115, 0.06, 0.13, 0.08));
	}
	r.app().debug_canvas().ghost = Some((1, (0.122, 0.068, 0.095, 0.062), "Counting shore posts".into(), Instant::now()));
	r.settle(|a| a.debug_doc().is_some_and(|d| d.sharp.is_some()), Duration::from_secs(8));
	r.hold(75);
	r.app().debug_canvas().ghost = None;

	// ── Clip 7: models window (memory check) ─────────────────────────
	r.start("models");
	r.app().debug_open_models(0);
	r.hold(45);
	r.app().debug_close_models();
	// ── Clip 6: revision compare on a real set ───────────────────────
	r.start("compare");
	r.app().open(set.clone(), &ctx);
	r.settle(|a| a.debug_doc().is_some_and(|d| d.loaded && d.base.contains_key(&1)), Duration::from_secs(20));
	r.app().debug_doc().unwrap().go_to_page(4);
	r.app().debug_left_tab(0);
	r.settle(|a| a.debug_doc().is_some_and(|d| d.base.contains_key(&4)), Duration::from_secs(10));
	r.hold(20);
	r.app().debug_doc().unwrap().start_compare(set.clone(), 5, &ctx);
	r.settle(|a| a.debug_doc().is_some_and(|d| d.compare.as_ref().is_some_and(|c| c.tex.is_some())), Duration::from_secs(20));
	r.hold(45);

	r.flush();
	println!("footage → {}", dir.display());
	for d in [&pdf, &set] {
		let _ = std::fs::remove_file(takeoff_core::sidecar::sidecar_path(d));
	}
}
