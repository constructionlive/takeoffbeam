//! Renders the real UI headless (wgpu) to PNGs for visual review:
//! `SNAP_DIR=/tmp/snaps SNAP_PDF=plan.pdf cargo test -p takeoff-app --test snapshots -- --ignored --nocapture`

use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::{Settings, ThemeChoice};
use takeoff_core::*;

fn harness(theme: ThemeChoice) -> Harness<'static, Option<TakeoffApp>> {
	Harness::builder()
		.with_size(egui::vec2(1440.0, 900.0))
		.with_pixels_per_point(1.0)
		.wgpu()
		.build_ui_state(
			move |ui, state: &mut Option<TakeoffApp>| {
				// Fonts installed by `new` take effect on the next frame.
				let Some(app) = state.as_mut() else {
					let mut s = Settings::default();
					s.theme = theme;
					s.author = "Alex Rivera".into();
					s.recent = vec!["/Users/office/Downloads/drawing.pdf".into(), "/Users/office/Downloads/27-10 DRAWING_A.pdf".into()];
					*state = Some(TakeoffApp::new(ui.ctx(), s));
					return;
				};
				app.frame(ui);
			},
			None,
		)
}

fn save(h: &mut Harness<'static, Option<TakeoffApp>>, name: &str) {
	let dir = std::env::var("SNAP_DIR").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
	let img = h.render().expect("render");
	let path = format!("{dir}/{name}.png");
	img.save(&path).unwrap();
	println!("wrote {path}");
}

fn settle(h: &mut Harness<'static, Option<TakeoffApp>>, cond: impl Fn(&mut TakeoffApp) -> bool, max: Duration) {
	let t = Instant::now();
	loop {
		h.step();
		if let Some(app) = h.state_mut().as_mut() {
			if cond(app) {
				break;
			}
		}
		if t.elapsed() > max {
			println!("settle timed out");
			break;
		}
		std::thread::sleep(Duration::from_millis(30));
	}
	for _ in 0..3 {
		h.step();
	}
}

fn p(x: f64, y: f64) -> Point {
	Point { x, y }
}

fn sample_takeoff(d: &mut Document) {
	d.edit("sample", |t| {
		let mut c = Category::new("c1", "Shore post 3m", "#F87171", CategoryKind::Count);
		c.unit_cost = Some(42.0);
		t.categories.push(c);
		let mut a = Category::new("c2", "Slab pour 2", "#34D399", CategoryKind::Area);
		a.hatch = Some(Hatch::Diagonal);
		a.unit_cost = Some(6.5);
		t.categories.push(a);
		t.categories.push(Category::new("c3", "Edge form", "#4CC2FF", CategoryKind::Length));
		let xs = [0.137, 0.153, 0.169, 0.185, 0.201, 0.217];
		for (k, x) in xs.iter().enumerate() {
			for (r, y) in [0.069, 0.149].iter().enumerate() {
				t.marks.push(Mark { id: format!("m{k}{r}"), category_id: Some("c1".into()), page: 1, x: *x, y: *y, w: 0.012, h: 0.016, source: if r == 1 { Source::Ai } else { Source::User }, status: if r == 1 && k > 3 { MarkStatus::Pending } else { MarkStatus::Confirmed }, tile: None, note: None, fields: None, meta: Default::default() });
			}
		}
		t.page_scales.push(PageScale { page: 1, p1: p(0.1, 0.5), p2: p(0.2, 0.5), distance: 20.0, unit: "ft".into() });
		let mut slab = Measurement::new("ms1", 1, MeasurementType::Area, vec![p(0.24, 0.07), p(0.33, 0.07), p(0.33, 0.2), p(0.28, 0.2), p(0.28, 0.14), p(0.24, 0.14)]);
		slab.category_id = Some("c2".into());
		slab.depth = Some(0.67);
		slab.cutouts = vec![vec![p(0.295, 0.09), p(0.315, 0.09), p(0.315, 0.11), p(0.295, 0.11)]];
		t.measurements.push(slab);
		let mut edge = Measurement::new("ms2", 1, MeasurementType::Length, vec![p(0.24, 0.215), p(0.33, 0.215), p(0.33, 0.25)]);
		edge.category_id = Some("c3".into());
		t.measurements.push(edge);
		let ann = |id: &str, kind: AnnotationKind, pts: Vec<Point>, text: Option<&str>| Annotation { id: id.into(), page: 1, kind, points: pts, text: text.map(String::from), style: AnnotationStyle { font_size: 9.0, ..Default::default() }, subject: None, note: None, source: None, meta: Default::default() };
		t.annotations.push(ann("a1", AnnotationKind::Cloud, vec![p(0.125, 0.05), p(0.235, 0.05), p(0.235, 0.12), p(0.125, 0.12)], None));
		t.annotations.push(ann("a2", AnnotationKind::Callout, vec![p(0.36, 0.08), p(0.47, 0.12), p(0.33, 0.1)], Some("Verify post spacing with the shoring engineer before pour 2")));
		t.custom_fields.push(FieldDef { id: "f1".into(), name: "Phase".into() });
	});
}

#[test]
#[ignore]
fn ui_snapshots() {
	let pdf = std::env::var("SNAP_PDF").unwrap_or_else(|_| "/Users/office/Downloads/drawing.pdf".into());
	for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
		let tag = if theme == ThemeChoice::Dark { "dark" } else { "light" };
		let mut h = harness(theme);
		h.run_steps(4);
		save(&mut h, &format!("{tag}-start"));
		let ctx = h.ctx.clone();
		h.state_mut().as_mut().unwrap().open(pdf.clone().into(), &ctx);
		settle(&mut h, |a| a.debug_doc().is_some_and(|d| d.loaded && d.base.contains_key(&d.page)), Duration::from_secs(20));
		if let Some(d) = h.state_mut().as_mut().unwrap().debug_doc() {
			let path = d.path.clone();
			let _ = std::fs::remove_file(takeoff_core::sidecar::sidecar_path(&path));
			sample_takeoff(&mut d.doc.lock());
			d.zoom_to(1, (0.1, 0.03, 0.4, 0.26));
			d.selection.insert("ms1".into());
		}
		settle(&mut h, |a| a.debug_doc().is_some_and(|d| d.sharp.is_some()), Duration::from_secs(5));
		h.state_mut().as_mut().unwrap().debug_right_tab(0);
		h.run_steps(3);
		save(&mut h, &format!("{tag}-takeoff"));
		h.state_mut().as_mut().unwrap().debug_right_tab(1);
		h.run_steps(3);
		save(&mut h, &format!("{tag}-properties"));
		h.state_mut().as_mut().unwrap().debug_right_tab(2);
		h.state_mut().as_mut().unwrap().debug_left_tab(2);
		h.run_steps(3);
		save(&mut h, &format!("{tag}-ai"));
		h.state_mut().as_mut().unwrap().debug_open_models(1);
		h.run_steps(4);
		save(&mut h, &format!("{tag}-models"));
		// Don't leave a sidecar behind next to the user's PDF.
		if let Some(d) = h.state_mut().as_mut().unwrap().debug_doc() {
			let _ = std::fs::remove_file(takeoff_core::sidecar::sidecar_path(&d.path));
			d.doc.lock().mark_saved();
		}
	}
}

#[test]
#[ignore]
fn compare_snapshot() {
	let a = std::path::PathBuf::from("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !a.exists() {
		return;
	}
	let mut h = harness(ThemeChoice::Light);
	h.run_steps(3);
	let ctx = h.ctx.clone();
	h.state_mut().as_mut().unwrap().open(a.clone(), &ctx);
	settle(&mut h, |app| app.debug_doc().is_some_and(|d| d.loaded && d.base.contains_key(&d.page)), Duration::from_secs(20));
	if let Some(d) = h.state_mut().as_mut().unwrap().debug_doc() {
		d.go_to_page(4);
	}
	settle(&mut h, |app| app.debug_doc().is_some_and(|d| d.base.contains_key(&4)), Duration::from_secs(10));
	if let Some(d) = h.state_mut().as_mut().unwrap().debug_doc() {
		d.start_compare(a.clone(), 5, &ctx);
	}
	settle(&mut h, |app| app.debug_doc().is_some_and(|d| d.compare.as_ref().is_some_and(|c| c.tex.is_some())), Duration::from_secs(20));
	save(&mut h, "light-compare");
}

#[test]
#[ignore]
fn update_banner_snapshot() {
	// A tiny local feed announcing a newer version.
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let port = listener.local_addr().unwrap().port();
	std::thread::spawn(move || {
		use std::io::{Read, Write};
		for mut s in listener.incoming().flatten() {
			let mut buf = [0u8; 2048];
			let _ = s.read(&mut buf);
			let body = r#"{"version":"0.0.2","date":"2026-11-02","notes":"- Auto area: click inside a room to fill it\n- 2x faster thumbnails\n- Fixes for rotated sheets","downloads":{"macos-arm64":"https://example.invalid/a.dmg"}}"#;
			let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
		}
	});
	std::env::set_var("TAKEOFF_UPDATE_URL", format!("http://127.0.0.1:{port}/latest.json"));
	let mut h = harness(ThemeChoice::Dark);
	h.run_steps(3);
	settle(&mut h, |_| false, Duration::from_secs(2));
	save(&mut h, "dark-update");
}
