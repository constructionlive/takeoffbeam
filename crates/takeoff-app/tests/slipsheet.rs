//! Slip-sheeting and sets through the real UI: review the matches, apply,
//! compare with the previous revision, one undo puts the old PDF and
//! takeoff back; a set lists every drawing's sheets and opens one on click.
//! The drawings are made here, so this always runs.
//! `cargo test -p takeoff-app --test slipsheet`

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use pdfium_render::prelude::*;
use takeoff_app::app::TakeoffApp;
use takeoff_app::settings::Settings;
use takeoff_app::slipsheet::Choice;
use takeoff_core::sheets::MatchBy;
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

/// Step until `cond`, in real time (the work happens on other threads).
fn wait(h: &mut H, what: &str, cond: impl Fn(&mut TakeoffApp) -> bool) {
	let t = Instant::now();
	loop {
		h.step();
		if cond(app(h)) {
			return;
		}
		assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
		std::thread::sleep(Duration::from_millis(15));
	}
}

/// Sheets: a number in the title block, and boxes drawn on it (fractions).
fn pdf(path: &Path, sheets: &[(&str, &[(f32, f32, f32, f32)])]) {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let font = doc.fonts_mut().helvetica_bold();
	let (w, h) = (1224.0_f32, 792.0_f32);
	for (number, boxes) in sheets {
		let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(w), PdfPoints::new(h))).unwrap();
		page.objects_mut().create_text_object(PdfPoints::new(w * 0.9), PdfPoints::new(h * 0.08), "SHEET NO.", font, PdfPoints::new(5.0)).unwrap();
		page.objects_mut().create_text_object(PdfPoints::new(w * 0.9), PdfPoints::new(h * 0.04), number, font, PdfPoints::new(16.0)).unwrap();
		let black = Some(PdfColor::new(0, 0, 0, 255));
		for (x, y, bw, bh) in boxes.iter() {
			let r = PdfRect::new_from_values(h * (1.0 - y - bh), w * x, h * (1.0 - y), w * (x + bw));
			page.objects_mut().create_path_object_rect(r, black, Some(PdfPoints::new(2.0)), black).unwrap();
		}
	}
	doc.save_to_file(path).unwrap();
}

fn numbers(pdf: &Path) -> Vec<String> {
	takeoff_pdf::sheets::index(pdf).unwrap().into_iter().map(|k| k.number.unwrap_or_default()).collect()
}

fn mark(page: u32, x: f64, y: f64) -> Mark {
	Mark { id: new_id("m"), category_id: None, page, x, y, w: 0.02, h: 0.02, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() }
}

fn key(h: &mut H, k: egui::Key, modifiers: Modifiers) {
	h.event(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
	h.step();
	h.event(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
	h.step();
}

#[test]
fn slip_sheet_review_apply_compare_undo_and_sets() {
	let dir = std::env::temp_dir().join(format!("takeoff-ui-slip-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	let plan = dir.join("Arch.pdf");
	pdf(&plan, &[("A-101", &[(0.1, 0.1, 0.2, 0.2)]), ("A-102", &[(0.4, 0.4, 0.1, 0.1)]), ("A-103", &[])]);
	// Revision B, in another order: A-102 gains a box at the bottom right; A-101 is reissued unchanged.
	let rev = dir.join("Arch rev B.pdf");
	pdf(&rev, &[("A102", &[(0.4, 0.4, 0.1, 0.1), (0.7, 0.7, 0.1, 0.1)]), ("A101", &[(0.1, 0.1, 0.2, 0.2)])]);
	let other = dir.join("Structural.pdf");
	pdf(&other, &[("S-101", &[]), ("S-102", &[])]);
	// The takeoff: one mark on what changes, one on what doesn't.
	let mut t = TakeoffData::default();
	t.categories.push(Category::new("c1", "Door", "#4CC2FF", CategoryKind::Count));
	let (on_change, steady) = (mark(2, 0.73, 0.73), mark(2, 0.42, 0.42));
	let (on_change_id, steady_id) = (on_change.id.clone(), steady.id.clone());
	t.marks = vec![Mark { category_id: Some("c1".into()), ..on_change }, steady, mark(1, 0.15, 0.15)];
	sidecar::save(&plan, &t).unwrap();
	let mut s = t.clone();
	s.marks = vec![Mark { category_id: Some("c1".into()), ..mark(1, 0.5, 0.5) }];
	sidecar::save(&other, &s).unwrap();

	let mut h = harness();
	h.step();
	let ctx = h.ctx.clone();
	app(&mut h).open(plan.clone(), &ctx);
	wait(&mut h, "the drawing", |a| a.debug_doc().is_some_and(|d| d.loaded));

	// Review: matched by sheet number, across the different order.
	app(&mut h).review_slip_sheet(false, vec![rev.clone()], &ctx);
	wait(&mut h, "the sheet numbers", |a| a.debug_slip().is_some_and(|r| !r.reading()));
	{
		let r = app(&mut h).debug_slip().unwrap();
		assert_eq!(r.rows.len(), 2);
		let target_page = |c: Choice| match c {
			Choice::Replace(t) => r.targets[t].key.page,
			_ => 0,
		};
		assert_eq!((target_page(r.rows[0].choice), r.rows[0].by), (2, Some(MatchBy::Number)));
		assert_eq!(target_page(r.rows[1].choice), 1);
	}
	h.step();
	assert!(h.query_by_label_contains("Slip-sheet a new revision").is_some(), "the review shows");
	h.get_by_label("Apply slip-sheet").click();
	wait(&mut h, "the swap", |a| a.debug_slip().is_none());
	wait(&mut h, "the reload", |a| a.debug_doc().is_some_and(|d| d.loaded && d.page_count() == 3));
	assert_eq!(numbers(&plan), vec!["A101", "A102", "A-103"], "new pages in place, A-103 kept");
	{
		let d = app(&mut h).debug_doc().unwrap().doc.lock();
		assert_eq!(d.data.marks.len(), 3, "the takeoff stays");
		assert_eq!(d.data.marks.iter().find(|m| m.id == steady_id).unwrap().x, 0.42, "same paper, same place");
		assert_eq!(d.data.sheet_revisions.len(), 2);
		let r2 = d.data.previous_revision(2).unwrap();
		assert!(r2.review.contains(&on_change_id) && !r2.review.contains(&steady_id), "{:?}", r2.review);
		assert!(d.data.previous_revision(1).unwrap().review.is_empty());
		assert_eq!(d.undo_label(), Some("Slip-sheet"));
	}
	assert_eq!(sidecar::load(&plan).unwrap().sheet_revisions.len(), 2, "saved");
	assert!(dir.join("Arch.pdf.revisions").is_dir());

	// Compare page 2 with the revision it replaced: the changed mark is flagged.
	app(&mut h).debug_doc().unwrap().go_to_page(2);
	assert!(app(&mut h).debug_doc().unwrap().compare_previous(&ctx));
	wait(&mut h, "the compare", |a| a.debug_doc().is_some_and(|d| d.compare.as_ref().is_some_and(|c| c.tex.is_some())));
	{
		let c = app(&mut h).debug_doc().unwrap().compare.as_ref().unwrap();
		assert!(c.revision && c.changed > 0.0);
		assert_eq!(c.flagged, vec![on_change_id.clone()]);
	}
	h.step();
	h.get_by_label_contains("1 item on changes").click();
	h.step();
	assert!(app(&mut h).debug_doc().unwrap().selection.contains(&on_change_id), "flagged items selected");

	// One undo: the old pages and the takeoff without history come back.
	key(&mut h, egui::Key::Z, Modifiers::COMMAND);
	wait(&mut h, "the undo", |a| a.debug_doc().is_some_and(|d| d.loaded && d.doc.lock().data.sheet_revisions.is_empty()));
	wait(&mut h, "the old PDF", |a| a.debug_doc().is_some_and(|d| d.loaded) && numbers(&a.debug_active_path().unwrap()) == ["A-101", "A-102", "A-103"]);
	assert!(app(&mut h).debug_doc().unwrap().compare.is_none());
	// And redo swaps the new ones back in.
	key(&mut h, egui::Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
	wait(&mut h, "the redo", |a| a.debug_doc().is_some_and(|d| d.loaded) && numbers(&a.debug_active_path().unwrap()) == ["A101", "A102", "A-103"]);
	assert_eq!(app(&mut h).debug_doc().unwrap().doc.lock().data.sheet_revisions.len(), 2);

	// A set of both drawings: the Set tab lists every sheet; a click opens the other drawing there.
	let set_path = dir.join("Tower.takeoffset.json");
	let mut set = set::SetFile::new("Tower");
	set.add(&set_path, &plan);
	set.add(&set_path, &other);
	set::save(&set_path, &set).unwrap();
	app(&mut h).open_set(set_path.clone(), &ctx);
	wait(&mut h, "the set's sheets", |a| a.debug_set().is_some_and(|s| s.docs.iter().all(|d| d.sheets.is_some())));
	h.step();
	assert!(app(&mut h).debug_set().unwrap().docs[0].revised.contains(&2), "the set knows what was revised");
	h.get_by_label("Structural S-102").click();
	h.step();
	assert_eq!(app(&mut h).debug_active_path(), Some(other.clone()));
	assert_eq!(app(&mut h).debug_doc().unwrap().page, 2);
	h.get_by_label("Arch A-103").click();
	h.step();
	assert_eq!((app(&mut h).debug_active_path(), app(&mut h).debug_doc().unwrap().page), (Some(plan.clone()), 3));

	// The project takeoff adds both drawings up.
	h.get_by_label_contains("Project takeoff:").click();
	h.step();
	h.step();
	assert!(h.query_by_label_contains("Project takeoff · Tower").is_some(), "the window opens");
	assert!(h.query_all_by_label("2 ea").count() >= 1, "1 door in each drawing, 2 in the project");

	// Set-level slip-sheet: a new S-102 goes to the Structural drawing.
	let srev = dir.join("S rev 2.pdf");
	pdf(&srev, &[("S-102", &[(0.2, 0.2, 0.3, 0.3)])]);
	app(&mut h).review_slip_sheet(true, vec![srev.clone()], &ctx);
	wait(&mut h, "the set's sheet numbers", |a| a.debug_slip().is_some_and(|r| !r.reading()));
	{
		let r = app(&mut h).debug_slip().unwrap();
		let Choice::Replace(t) = r.rows[0].choice else { panic!("matched: {:?}", r.rows[0].choice) };
		assert_eq!((r.targets[t].doc, r.targets[t].key.page), (1, 2));
	}
	h.step();
	h.get_by_label("Apply slip-sheet").click();
	wait(&mut h, "the set swap", |a| a.debug_slip().is_none());
	assert_eq!(app(&mut h).debug_active_path(), Some(other.clone()));
	wait(&mut h, "Structural reloaded", |a| a.debug_doc().is_some_and(|d| d.loaded));
	assert_eq!(sidecar::load(&other).unwrap().sheet_revisions.len(), 1);
	assert_eq!(sidecar::load(&plan).unwrap().sheet_revisions.len(), 2, "Arch untouched");
	drop(h);
	std::fs::remove_dir_all(dir).ok();
}
