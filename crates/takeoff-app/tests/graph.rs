//! Graph view on a small drawing set with a document map next to it:
//! ⌘G switches to it, a click selects a card, a double-click opens that
//! detail on its sheet. `GRAPH_SNAP_DIR=/tmp/snaps` also writes screenshots.
//! `cargo test -p takeoff-app --test graph`

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use takeoff_app::app::TakeoffApp;
use takeoff_app::docstate::ViewMode;
use takeoff_app::settings::{Settings, ThemeChoice};
use takeoff_core::docmap::{self, DocMap, Link, LinkKind, Part, PartKind, Region, Unresolved};

type H = Harness<'static, Option<TakeoffApp>>;

/// A landscape page per sheet: a frame, boxes where the drawings are, and the sheet number.
fn make_pdf(path: &std::path::Path, sheets: &[(&str, &[(f64, f64, f64, f64)])]) {
	let (w, h) = (792.0, 612.0);
	let mut objects: Vec<String> = vec![String::new(); 3];
	let mut kids = Vec::new();
	for (number, boxes) in sheets {
		let mut c = String::from("0 0 0 RG 1.5 w 18 18 756 576 re S 0.5 w\n");
		for (x, y, bw, bh) in boxes.iter() {
			c.push_str(&format!("{:.1} {:.1} {:.1} {:.1} re S\n", x * w, h - (y + bh) * h, bw * w, bh * h));
			// Some line work inside, so thumbnails show a drawing.
			for k in 1..6 {
				let yy = h - (y + bh * k as f64 / 6.0) * h;
				c.push_str(&format!("{:.1} {:.1} m {:.1} {:.1} l S\n", (x + bw * 0.1) * w, yy, (x + bw * (0.4 + 0.1 * k as f64)) * w, yy));
			}
		}
		c.push_str(&format!("BT /F1 22 Tf {:.1} 40 Td ({number}) Tj ET\n", w - 140.0));
		let content = objects.len();
		objects.push(format!("<< /Length {} >>\nstream\n{c}endstream", c.len()));
		kids.push(objects.len());
		objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {content} 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>"));
	}
	objects[1] = "<< /Type /Catalog /Pages 2 0 R >>".into();
	objects[2] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.iter().map(|k| format!("{k} 0 R")).collect::<Vec<_>>().join(" "), kids.len());
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

fn region(x: f64, y: f64, w: f64, h: f64) -> Option<Region> {
	Some(Region { x, y, w, h })
}

fn part(kind: PartKind, label: Option<&str>, title: &str, summary: &str, r: Option<Region>, parent: Option<&str>) -> Part {
	Part { id: String::new(), kind, label: label.map(str::to_string), title: title.into(), summary: summary.into(), region: r, parent: parent.map(str::to_string) }
}

fn link(from: &str, to: &str, kind: LinkKind, text: &str, note: &str) -> Link {
	Link { from: from.into(), to: to.into(), kind, text: text.into(), note: note.into() }
}

const PLAN: [(f64, f64, f64, f64); 2] = [(0.05, 0.06, 0.66, 0.84), (0.78, 0.06, 0.17, 0.84)];
const DETAILS: [(f64, f64, f64, f64); 4] = [(0.05, 0.06, 0.34, 0.4), (0.42, 0.06, 0.34, 0.4), (0.05, 0.52, 0.34, 0.4), (0.78, 0.06, 0.17, 0.84)];
const FRAMING: [(f64, f64, f64, f64); 2] = [(0.05, 0.06, 0.66, 0.84), (0.78, 0.06, 0.17, 0.84)];

/// As the AI records it: sheets, areas (callouts inside the plan), links with notes.
fn map() -> DocMap {
	let mut m = DocMap::new(3);
	m.model = Some("claude-opus-5-5".into());
	m.overview = "A small test set: an architectural floor plan, its details, and the structural framing.".into();
	let sheets = [(1, "A-101", "Floor plan", "Architectural", "Level 1 floor plan with rooms, doors and the stair."), (2, "A-501", "Details", "Architectural", "Stair and wall details."), (3, "S-201", "Framing plan", "Structural", "Level 2 framing.")];
	for (page, number, title, discipline, summary) in sheets {
		let s = m.sheet_mut(page);
		s.number = Some(number.into());
		s.title = title.into();
		s.discipline = Some(discipline.into());
		s.summary = summary.into();
		s.mapped = page != 3;
	}
	m.add_part(1, part(PartKind::Plan, Some("1"), "Level 1", "Rooms, doors and the stair core.", region(0.05, 0.06, 0.66, 0.84), None));
	m.add_part(1, part(PartKind::TitleBlock, None, "", "", region(0.78, 0.06, 0.17, 0.84), None));
	m.add_part(1, part(PartKind::Callout, Some("3/A-501"), "", "", region(0.3, 0.3, 0.02, 0.03), Some("s1.1")));
	m.add_part(1, part(PartKind::Callout, Some("A/A-501"), "", "", region(0.5, 0.5, 0.02, 0.03), Some("s1.1")));
	m.add_part(2, part(PartKind::Detail, Some("1"), "Tread nosing", "Nosing profile.", region(0.05, 0.06, 0.34, 0.4), None));
	m.add_part(2, part(PartKind::Detail, Some("3"), "Handrail bracket", "Wall-mounted rail bracket.", region(0.42, 0.06, 0.34, 0.4), None));
	m.add_part(2, part(PartKind::Section, Some("A"), "Wall section", "Exterior wall assembly.", region(0.05, 0.52, 0.34, 0.4), None));
	m.add_part(3, part(PartKind::Plan, None, "Level 2 framing", "", region(0.05, 0.06, 0.66, 0.84), None));
	m.link(link("s1.3", "s2.2", LinkKind::Detail, "3/A-501", "The stair's handrail bracket."));
	m.link(link("s1.4", "s2.3", LinkKind::Section, "A/A-501", "Cut through the exterior wall."));
	m.link(link("s1.1", "s3", LinkKind::Reference, "SEE S-201", "Framing above this floor."));
	m.link(link("s2.2", "s2.1", LinkKind::Detail, "1/-", "The bracket sits on the nosing."));
	m.unresolved.push(Unresolved { from: "s1.1".into(), text: "M-101, mechanical set".into() });
	m
}

fn harness(theme: ThemeChoice) -> H {
	Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_pixels_per_point(1.0).with_step_dt(1.0 / 60.0).wgpu().build_ui_state(
		move |ui, state: &mut Option<TakeoffApp>| {
			let Some(app) = state.as_mut() else {
				let mut s = Settings::default();
				s.theme = theme;
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

fn press(h: &mut H, pos: Pos2, times: usize) {
	h.event(Event::PointerMoved(pos));
	h.step();
	for _ in 0..times {
		h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
		h.step();
		h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
		h.step();
	}
	pump(h, 25);
}

fn snap(h: &mut H, name: &str) {
	if let Ok(dir) = std::env::var("GRAPH_SNAP_DIR") {
		let img = h.render().expect("render");
		let path = format!("{dir}/{name}.png");
		img.save(&path).unwrap();
		println!("wrote {path}");
	}
}

fn settle(h: &mut H) {
	let t = Instant::now();
	while !app(h).debug_doc().is_some_and(|d| d.graph.debug_settled()) && t.elapsed() < Duration::from_secs(30) {
		h.step();
	}
	// Thumbnails come from the render worker.
	for _ in 0..40 {
		h.step();
		std::thread::sleep(Duration::from_millis(15));
	}
}

#[test]
fn graph_view_browses_the_map_and_opens_a_detail() {
	let dir = std::env::temp_dir().join(format!("takeoff-graph-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Set.pdf");
	make_pdf(&pdf, &[("A-101", &PLAN), ("A-501", &DETAILS), ("S-201", &FRAMING)]);
	let m = map();
	assert_eq!(m.links.len(), 4, "{:?}", m.links);
	docmap::save(&pdf, &m).unwrap();

	let theme = if std::env::var("GRAPH_SNAP_LIGHT").is_ok() { ThemeChoice::Light } else { ThemeChoice::Dark };
	let mut h = harness(theme);
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	assert!(app(&mut h).debug_doc().unwrap().map.is_some(), "the map next to the PDF is loaded with it");

	// ⌘G: the graph.
	h.event(Event::Key { key: Key::G, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
	pump(&mut h, 2);
	assert_eq!(app(&mut h).debug_doc().unwrap().view_mode, ViewMode::Graph);
	settle(&mut h);
	snap(&mut h, "graph-overview");

	// Click the handrail detail: it is selected and its details show.
	let at = app(&mut h).debug_doc().unwrap().graph.debug_node_center("s2.2").expect("detail 3 is on the graph");
	press(&mut h, at, 1);
	assert_eq!(app(&mut h).debug_doc().unwrap().graph.selected.as_deref(), Some("s2.2"));
	settle(&mut h);
	snap(&mut h, "graph-selected");

	// Focus: only what is one step from the detail (its sheet, the callout and detail it links).
	app(&mut h).debug_doc().unwrap().graph.local = 1;
	settle(&mut h);
	snap(&mut h, "graph-focus");
	{
		let g = &app(&mut h).debug_doc().unwrap().graph;
		for id in ["s2.2", "s2", "s2.1", "s1.3"] {
			assert!(g.debug_node_center(id).is_some(), "{id} is near the detail");
		}
		assert!(g.debug_node_center("s3").is_none(), "the framing plan is not");
	}
	app(&mut h).debug_doc().unwrap().graph.local = 0;
	settle(&mut h);

	// The title block is not a card.
	assert!(app(&mut h).debug_doc().unwrap().graph.debug_node_center("s1.2").is_none());

	// Esc clears the selection; ⌘G goes back to the page.
	h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
	pump(&mut h, 2);
	assert!(app(&mut h).debug_doc().unwrap().graph.selected.is_none());

	// Double-click opens the detail on its sheet, zoomed in on it.
	let at = app(&mut h).debug_doc().unwrap().graph.debug_node_center("s2.2").unwrap();
	press(&mut h, at, 2);
	let d = app(&mut h).debug_doc().unwrap();
	assert_eq!(d.view_mode, ViewMode::Page);
	assert_eq!(d.page, 2);
	let pr = d.page_rect();
	let detail_center = egui::pos2(pr.left() + pr.width() * (0.42 + 0.17), pr.top() + pr.height() * (0.06 + 0.2));
	assert!((detail_center - d.canvas_rect.center()).length() < 4.0, "the detail is in the middle of the canvas: {detail_center:?} vs {:?}", d.canvas_rect.center());
	pump(&mut h, 10);
	snap(&mut h, "graph-opened");

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_map_the_graph_offers_to_build_one() {
	let dir = std::env::temp_dir().join(format!("takeoff-graph-empty-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Plain.pdf");
	make_pdf(&pdf, &[("A-101", &PLAN), ("A-501", &DETAILS)]);
	let mut h = harness(ThemeChoice::Dark);
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	h.event(Event::Key { key: Key::G, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
	pump(&mut h, 4);
	assert_eq!(app(&mut h).debug_doc().unwrap().view_mode, ViewMode::Graph);
	use egui_kittest::kittest::Queryable;
	assert!(h.query_by_label_contains("Map this drawing set").is_some(), "the build card shows");
	assert!(h.query_by_label_contains("Runs in the AI chat on").is_some(), "mapping runs in the chat");
	assert!(h.query_by_label_contains("Start mapping").is_some());
	snap(&mut h, "graph-empty");
	let _ = std::fs::remove_dir_all(&dir);
}

/// In Page view the map is links on the sheet: a click on a callout goes to
/// the detail it names, the Map tab follows, Back restores the view left,
/// and a discipline filter narrows the Pages list.
#[test]
fn callouts_on_the_sheet_lead_to_their_details_and_back() {
	let dir = std::env::temp_dir().join(format!("takeoff-graph-links-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Set.pdf");
	make_pdf(&pdf, &[("A-101", &PLAN), ("A-501", &DETAILS), ("S-201", &FRAMING)]);
	docmap::save(&pdf, &map()).unwrap();
	let theme = if std::env::var("GRAPH_SNAP_LIGHT").is_ok() { ThemeChoice::Light } else { ThemeChoice::Dark };
	let mut h = harness(theme);
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	app(&mut h).debug_right_tab(3);
	pump(&mut h, 30);
	for _ in 0..30 {
		h.step();
		std::thread::sleep(Duration::from_millis(15));
	}
	// The Map tab shows the sheet on screen.
	assert_eq!(app(&mut h).debug_doc().unwrap().map_focus().as_deref(), Some("s1"));
	let view_before = {
		let d = app(&mut h).debug_doc().unwrap();
		(d.zoom, d.pan)
	};
	// Hover, then click the 3/A-501 bubble.
	let bubble = {
		let d = app(&mut h).debug_doc().unwrap();
		d.to_screen(takeoff_core::Point { x: 0.31, y: 0.315 })
	};
	h.event(Event::PointerMoved(bubble));
	pump(&mut h, 3);
	snap(&mut h, "page-hover-callout");
	press(&mut h, bubble, 1);
	{
		let d = app(&mut h).debug_doc().unwrap();
		assert_eq!(d.page, 2, "the callout leads to A-501");
		assert_eq!(d.map_focus().as_deref(), Some("s2.2"));
		let pr = d.page_rect();
		let detail = egui::pos2(pr.left() + pr.width() * (0.42 + 0.17), pr.top() + pr.height() * (0.06 + 0.2));
		assert!((detail - d.canvas_rect.center()).length() < 4.0, "the detail is centred");
	}
	pump(&mut h, 8);
	for _ in 0..20 {
		h.step();
		std::thread::sleep(Duration::from_millis(15));
	}
	snap(&mut h, "page-arrived");
	// Back: the plan again, exactly as it was.
	h.event(Event::Key { key: Key::ArrowLeft, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::ALT });
	pump(&mut h, 4);
	{
		let d = app(&mut h).debug_doc().unwrap();
		assert_eq!(d.page, 1);
		assert_eq!((d.zoom, d.pan), view_before);
		assert_eq!(d.map_focus().as_deref(), Some("s1.3"), "back at the callout we left from");
		assert!(d.can_go_forward());
	}
	// Forward again from the Map tab's trail, then the Structural filter.
	app(&mut h).debug_doc().unwrap().map_forward();
	pump(&mut h, 4);
	assert_eq!(app(&mut h).debug_doc().unwrap().page, 2);
	app(&mut h).debug_doc().unwrap().map_filter.disciplines = ["Structural".to_string()].into();
	assert_eq!(app(&mut h).debug_doc().unwrap().filtered_pages(), Some(vec![3]));
	pump(&mut h, 6);
	snap(&mut h, "page-filtered");
	let _ = std::fs::remove_dir_all(&dir);
}

/// A long set with messy discipline names: they group (Civil › Track,
/// Civil › Yard…), the filter is one dropdown, and the Pages list folds by
/// discipline.
#[test]
fn many_disciplines_group_into_one_filter() {
	let dir = std::env::temp_dir().join(format!("takeoff-graph-many-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let pdf = dir.join("Long.pdf");
	let disciplines = ["General", "civil/track", "civil/track", "civil/track", "Civil", "CIVIL", "civil/yard", "civil/yard", "Structural", "Structural", "Electrical/systems", "Electrical - Lighting", "Mechanical", "Plumbing"];
	let numbers: Vec<String> = (0..disciplines.len()).map(|k| format!("X-{:03}", k + 1)).collect();
	let sheets: Vec<(&str, &[(f64, f64, f64, f64)])> = numbers.iter().map(|n| (n.as_str(), &PLAN[..])).collect();
	make_pdf(&pdf, &sheets);
	let mut m = DocMap::new(disciplines.len() as u32);
	for (k, d) in disciplines.iter().enumerate() {
		let s = m.sheet_mut(k as u32 + 1);
		s.number = Some(numbers[k].clone());
		s.title = format!("Sheet {}", k + 1);
		s.discipline = Some(d.to_string());
		s.mapped = true;
	}
	let groups = takeoff_app::graph_view::discipline_groups(&m);
	let names: Vec<(&str, usize, usize)> = groups.iter().map(|g| (g.name.as_str(), g.count, g.subs.len())).collect();
	assert_eq!(names, vec![("General", 1, 1), ("Civil", 7, 3), ("Structural", 2, 1), ("Electrical", 2, 2), ("Mechanical", 1, 1), ("Plumbing", 1, 1)]);
	docmap::save(&pdf, &m).unwrap();

	let theme = if std::env::var("GRAPH_SNAP_LIGHT").is_ok() { ThemeChoice::Light } else { ThemeChoice::Dark };
	let mut h = harness(theme);
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(pdf.clone(), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(20) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	for _ in 0..30 {
		h.step();
		std::thread::sleep(Duration::from_millis(15));
	}
	snap(&mut h, "pages-grouped");
	use egui_kittest::kittest::Queryable;
	h.query_by_label_contains("All disciplines").expect("the discipline filter").click();
	pump(&mut h, 4);
	snap(&mut h, "pages-filter-open");
	// Civil, only: the group's three disciplines.
	app(&mut h).debug_doc().unwrap().map_filter.disciplines = ["Civil".to_string(), "Civil/Track".to_string(), "Civil/Yard".to_string()].into();
	assert_eq!(app(&mut h).debug_doc().unwrap().filtered_pages(), Some(vec![2, 3, 4, 5, 6, 7, 8]));
	h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
	pump(&mut h, 4);
	h.event(Event::Key { key: Key::G, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
	pump(&mut h, 2);
	settle(&mut h);
	snap(&mut h, "graph-filtered");
	let _ = std::fs::remove_dir_all(&dir);
}

/// A real mapped set as a board: GRAPH_REAL_PDF=… GRAPH_SNAP_DIR=… cargo test -p takeoff-app --test graph real_board -- --ignored
#[test]
#[ignore]
fn real_board() {
	let Ok(pdf) = std::env::var("GRAPH_REAL_PDF") else { return };
	let mut h = harness(ThemeChoice::Dark);
	pump(&mut h, 2);
	let ctx = h.ctx.clone();
	app(&mut h).open(std::path::PathBuf::from(pdf), &ctx);
	let t = Instant::now();
	while !app(&mut h).debug_doc().is_some_and(|d| d.loaded) && t.elapsed() < Duration::from_secs(60) {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	h.event(Event::Key { key: Key::G, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
	pump(&mut h, 2);
	settle(&mut h);
	for _ in 0..120 {
		h.step();
		std::thread::sleep(Duration::from_millis(20));
	}
	snap(&mut h, "real-board");
}
