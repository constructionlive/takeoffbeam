//! Continuous view and spreads: pages laid out from their sizes, only the
//! pages on screen rendered, the current page following the view, and the
//! tools working on whichever page the pointer is over.
//! `cargo test -p takeoff-app --test page_layout` (`LAYOUT_SNAP_DIR=…` writes screenshots).

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect};
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::layout::PageLayout;
use takeoff_app::settings::Settings;
use takeoff_app::tools::Tool;
use takeoff_core::{MeasurementType, Point};

type H = Harness<'static, Option<TakeoffApp>>;

/// Landscape, landscape, portrait, landscape, landscape, landscape: a box
/// and a diagonal on each.
const SIZES: [(f64, f64); 6] = [(792.0, 612.0), (792.0, 612.0), (612.0, 792.0), (792.0, 612.0), (792.0, 612.0), (792.0, 612.0)];

fn make_pdf(path: &std::path::Path) {
	let n = SIZES.len();
	let mut objects = vec![String::new(), "<< /Type /Catalog /Pages 2 0 R >>".to_string()];
	let kids: Vec<String> = (0..n).map(|k| format!("{} 0 R", 3 + 2 * k)).collect();
	objects.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
	for (k, (w, h)) in SIZES.iter().enumerate() {
		let c = format!("0 0 0 RG 1 w 72 72 {} {} re S 72 72 m {} {} l S\n", w - 144.0, h - 144.0, w - 72.0, h - 72.0);
		objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R >>", 4 + 2 * k));
		objects.push(format!("<< /Length {} >>\nstream\n{c}endstream", c.len()));
	}
	let mut out = String::from("%PDF-1.4\n");
	let mut offsets = vec![0usize; objects.len()];
	for (i, o) in objects.iter().enumerate().skip(1) {
		offsets[i] = out.len();
		out.push_str(&format!("{i} 0 obj\n{o}\nendobj\n"));
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
				s.page_layout = PageLayout::CONTINUOUS;
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

fn snap(h: &mut H, name: &str) {
	if let Ok(dir) = std::env::var("LAYOUT_SNAP_DIR") {
		settle(h, |a| a.debug_doc().is_some_and(|d| d.visible_pages().iter().all(|p| d.base.contains_key(p) || d.thumbs.contains_key(p))));
		pump(h, 30);
		h.render().expect("render").save(format!("{dir}/{name}.png")).unwrap();
	}
}

fn key(h: &mut H, key: egui::Key) {
	h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
	h.step();
	h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
	pump(h, 3);
}

/// Two fingers on a trackpad: the view moves by `dy` points.
fn scroll(h: &mut H, at: Pos2, dy: f32) {
	h.event(Event::PointerMoved(at));
	h.event(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE });
	pump(h, 30);
}

fn open(tag: &str) -> (H, std::path::PathBuf) {
	let dir = std::env::temp_dir().join(format!("takeoff-layout-{tag}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Set.pdf");
	make_pdf(&pdf);
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf, &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	assert!(app(&mut h).debug_doc().unwrap().loaded, "opened");
	pump(&mut h, 10);
	(h, dir)
}

/// Step (with real time passing, for the render worker) until `cond`.
fn settle(h: &mut H, cond: impl Fn(&mut TakeoffApp) -> bool) {
	let t = Instant::now();
	while !cond(app(h)) && t.elapsed() < Duration::from_secs(10) {
		h.step();
		std::thread::sleep(Duration::from_millis(15));
	}
}

#[test]
fn continuous_lays_out_every_page_and_renders_only_the_ones_on_screen() {
	let (mut h, dir) = open("maths");
	let d = app(&mut h).debug_doc().unwrap();
	assert!(d.layout.continuous && !d.layout.spreads);
	let (z, gap) = (d.zoom, d.arr.gap);
	let r: Vec<Rect> = (1..=6).map(|p| d.page_rect_of(p)).collect();
	// Stacked top to bottom with the gap, each page centred on the same axis.
	for k in 0..5 {
		assert!((r[k + 1].top() - r[k].bottom() - gap * z).abs() < 0.01, "gap after page {}: {:?} {:?}", k + 1, r[k], r[k + 1]);
		assert!((r[k + 1].center().x - r[k].center().x).abs() < 0.01, "centred");
	}
	assert!((r[2].width() - 612.0 * z).abs() < 0.01 && (r[2].height() - 792.0 * z).abs() < 0.01, "page 3 portrait: {:?}", r[2]);
	// Opened fitted to page 1, the current page.
	let canvas = d.canvas_rect;
	assert_eq!(d.page, 1);
	assert!(canvas.contains_rect(r[0]), "page 1 fits: {:?} in {canvas:?}", r[0]);
	let on_screen: Vec<u32> = (1..=6).filter(|p| r[*p as usize - 1].intersects(canvas)).collect();
	assert_eq!(d.visible_pages(), on_screen);
	assert!(!on_screen.contains(&4));
	// Only pages on or near the screen are rendered: never page 6.
	settle(&mut h, |a| a.debug_doc().is_some_and(|d| d.base.contains_key(&1)));
	pump(&mut h, 20);
	let d = app(&mut h).debug_doc().unwrap();
	assert!(d.base.contains_key(&1), "page 1 rendered");
	assert!(!d.base.contains_key(&6), "page 6 is far below: not rendered ({:?})", d.base.keys().collect::<Vec<_>>());
	let d = app(&mut h).debug_doc().unwrap();
	let c = d.canvas_rect.center();
	d.zoom_at(c, 0.4);
	snap(&mut h, "continuous");

	// Spreads: pages 1 | 2 side by side, 3 | 4 under them.
	let d = app(&mut h).debug_doc().unwrap();
	d.set_layout(PageLayout::SPREADS_CONTINUOUS);
	let (a, b, c) = (d.page_rect_of(1), d.page_rect_of(2), d.page_rect_of(3));
	assert!((b.left() - a.right() - d.arr.gap * d.zoom).abs() < 0.01 && a.top() == b.top(), "1 | 2: {a:?} {b:?}");
	assert!(c.top() > a.bottom() && c.right() < b.left() + 1.0, "3 starts the next row on the left: {c:?}");
	let d = app(&mut h).debug_doc().unwrap();
	d.set_layout(PageLayout { cover: true, ..PageLayout::SPREADS_CONTINUOUS });
	assert!(d.page_rect_of(2).top() > d.page_rect_of(1).bottom(), "the cover stands alone");
	assert!((d.page_rect_of(2).center().y - d.page_rect_of(3).center().y).abs() < 0.01, "then 2 | 3");
	snap(&mut h, "spreads-cover");
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_current_page_follows_the_view_and_navigation_scrolls() {
	let (mut h, dir) = open("follow");
	let c = app(&mut h).debug_doc().unwrap().canvas_rect.center();
	// Scroll down a page and a bit: page 2 is under the centre and current.
	let step = { let d = app(&mut h).debug_doc().unwrap(); d.page_rect_of(2).center().y - c.y };
	scroll(&mut h, c, -step);
	let d = app(&mut h).debug_doc().unwrap();
	assert!(d.page_rect_of(2).contains(c), "page 2 under the centre: {:?}", d.page_rect_of(2));
	assert_eq!(d.page, 2, "current page follows the view");
	assert!(d.history.is_empty(), "scrolling isn't navigation");
	// PgDn scrolls to page 3 (zoom kept); Page Up back to 2.
	let zoom = d.zoom;
	key(&mut h, egui::Key::PageDown);
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.page, 3);
	assert_eq!(d.zoom, zoom, "paging keeps the zoom when continuous");
	assert!(d.page_rect_of(3).contains(c), "page 3 brought into view: {:?}", d.page_rect_of(3));
	pump(&mut h, 5);
	assert_eq!(app(&mut h).debug_doc().unwrap().page, 3, "and it stays current");
	// Go to page 6, then fit its width: still page 6, centred.
	let d = app(&mut h).debug_doc().unwrap();
	d.go_to_page(6);
	d.fit_width();
	pump(&mut h, 3);
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.page, 6);
	assert!(d.page_rect_of(6).contains(c) && (d.page_rect_of(6).width() - (d.canvas_rect.width() - 48.0)).abs() < 0.5);
	// A search hit / bookmark on page 4 frames that area of page 4.
	let d = app(&mut h).debug_doc().unwrap();
	d.zoom_to(4, (0.4, 0.4, 0.2, 0.2));
	pump(&mut h, 3);
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.page, 4);
	let mid = d.to_screen(Point { x: 0.5, y: 0.5 });
	assert!((mid - c).length() < 1.0, "the area is in the middle: {mid:?} vs {c:?}");
	// Single page again: just page 4, where it was.
	let before = d.page_rect_of(4);
	d.set_layout(PageLayout::SINGLE);
	assert_eq!(d.visible_pages(), vec![4]);
	assert!((d.page_rect().min - before.min).length() < 0.01, "page stays put on screen");
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_measurement_lands_on_the_page_under_the_pointer() {
	let (mut h, dir) = open("measure");
	// Zoom out so several pages show; draw on one that isn't current.
	let d = app(&mut h).debug_doc().unwrap();
	let top = d.canvas_rect.center_top() + egui::vec2(0.0, 30.0);
	d.zoom_at(top, 0.5);
	pump(&mut h, 3);
	let d = app(&mut h).debug_doc().unwrap();
	let current = d.page;
	let target = d.visible_pages().into_iter().find(|p| *p != current && d.canvas_rect.contains_rect(d.page_rect_of(*p))).expect("another page wholly on screen");
	let (rt, rc) = (d.page_rect_of(target), d.page_rect_of(current));
	let at = |x: f32, y: f32| egui::pos2(rt.min.x + x * rt.width(), rt.min.y + y * rt.height());

	app(&mut h).debug_canvas().set_tool(Tool::Length);
	click(&mut h, at(0.25, 0.5));
	assert_eq!(app(&mut h).debug_doc().unwrap().page, target, "drawing on page {target} makes it current");
	// A click on the other page doesn't add to the shape.
	click(&mut h, rc.center());
	assert_eq!(app(&mut h).debug_canvas().points.len(), 1, "page {current}'s click ignored");
	assert_eq!(app(&mut h).debug_doc().unwrap().page, target, "still page {target}");
	click(&mut h, at(0.75, 0.5));
	key(&mut h, egui::Key::Enter);
	{
		let d = app(&mut h).debug_doc().unwrap();
		let doc = d.doc.lock();
		assert_eq!(doc.data.measurements.len(), 1, "one length");
		let m = &doc.data.measurements[0];
		assert_eq!((m.page, m.kind), (target, MeasurementType::Length));
		let want = [(0.25, 0.5), (0.75, 0.5)];
		for (p, w) in m.points.iter().zip(want) {
			assert!((p.x - w.0).abs() < 0.003 && (p.y - w.1).abs() < 0.003, "point {p:?} vs {w:?}");
		}
	}
	// Hit-testing is per page too: select it, then count on the other page.
	app(&mut h).debug_canvas().set_tool(Tool::Select);
	click(&mut h, at(0.5, 0.5));
	assert_eq!(app(&mut h).debug_doc().unwrap().selection.len(), 1, "selected on page {target}");
	app(&mut h).debug_canvas().set_tool(Tool::Count);
	click(&mut h, rc.center());
	app(&mut h).debug_canvas().set_tool(Tool::Length);
	click(&mut h, at(0.3, 0.7));
	h.event(Event::PointerMoved(at(0.6, 0.8)));
	snap(&mut h, "measuring");
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.doc.lock().data.marks.iter().map(|m| m.page).collect::<Vec<_>>(), vec![current]);
	let _ = std::fs::remove_dir_all(&dir);
}

/// A real 52-sheet set, scrolled end to end zoomed out: renders are asked
/// a few at a time and textures stay near the view.
#[test]
fn a_real_set_scrolls_end_to_end() {
	let src = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !src.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-layout-real-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("plan.pdf");
	std::fs::copy(&src, &pdf).unwrap();
	let mut h = harness();
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf, &ctx);
	settle(&mut h, |a| a.debug_doc().is_some_and(|d| d.loaded));
	pump(&mut h, 5);
	let d = app(&mut h).debug_doc().unwrap();
	let n = d.page_count();
	let c = d.canvas_rect.center();
	d.zoom_at(c, 0.3);
	snap(&mut h, "real-zoomed-out");
	let (t, mut frames, mut most) = (Instant::now(), 0, 0);
	while app(&mut h).debug_doc().unwrap().page < n && t.elapsed() < Duration::from_secs(60) {
		scroll(&mut h, c, -400.0);
		frames += 30;
		most = most.max(app(&mut h).debug_doc().unwrap().base.len());
	}
	let d = app(&mut h).debug_doc().unwrap();
	println!("{n} pages scrolled in {:?} ({frames} frames, {:.1} ms each), at most {most} page textures", t.elapsed(), t.elapsed().as_secs_f64() * 1000.0 / frames as f64);
	assert_eq!(d.page, n, "reached the last sheet");
	assert!(most <= 40, "textures stay near the view: {most}");
	snap(&mut h, "real-end");
	let _ = std::fs::remove_dir_all(&dir);
}
