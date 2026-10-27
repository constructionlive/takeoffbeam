//! Slip-sheeting on real PDFs: sheet numbers read off the title block, the
//! new pages swapped in, the takeoff carried onto them (also onto other
//! paper), the old pages kept, and the items on changed parts flagged.
//! `cargo test -p takeoff-pdf --test slip`

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;
use takeoff_core::sheets::{match_sheets, MatchBy, NumberSource};
use takeoff_core::*;
use takeoff_pdf::slip::{slip_sheet, Placement, SlipPage};

/// A sheet: its number, its paper (points), and boxes drawn on it (fractions, x y w h).
struct Sheet<'a> {
	number: &'a str,
	size: (f32, f32),
	boxes: &'a [(f32, f32, f32, f32)],
}

fn pdf(path: &Path, sheets: &[Sheet]) {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let font = doc.fonts_mut().helvetica_bold();
	for s in sheets {
		let (w, h) = s.size;
		let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(w), PdfPoints::new(h))).unwrap();
		let black = Some(PdfColor::new(0, 0, 0, 255));
		// Title block: "SHEET NO." over the number, a job number in big type beside it, a reference to another sheet in the notes.
		page.objects_mut().create_text_object(PdfPoints::new(w * 0.9), PdfPoints::new(h * 0.08), "SHEET NO.", font, PdfPoints::new(6.0)).unwrap();
		page.objects_mut().create_text_object(PdfPoints::new(w * 0.9), PdfPoints::new(h * 0.04), s.number, font, PdfPoints::new(20.0)).unwrap();
		page.objects_mut().create_text_object(PdfPoints::new(w * 0.9), PdfPoints::new(h * 0.14), "5599.2", font, PdfPoints::new(20.0)).unwrap();
		page.objects_mut().create_text_object(PdfPoints::new(w * 0.78), PdfPoints::new(h * 0.3), "SEE A-501 FOR DETAILS", font, PdfPoints::new(5.0)).unwrap();
		for (x, y, bw, bh) in s.boxes {
			let r = PdfRect::new_from_values(h * (1.0 - y - bh), w * x, h * (1.0 - y), w * (x + bw));
			page.objects_mut().create_path_object_rect(r, black, Some(PdfPoints::new(2.0)), black).unwrap();
		}
	}
	doc.save_to_file(path).unwrap();
}

fn dir(name: &str) -> PathBuf {
	let d = std::env::temp_dir().join(format!("takeoff-slip-{name}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&d);
	std::fs::create_dir_all(&d).unwrap();
	d
}

fn mark(page: u32, x: f64, y: f64) -> Mark {
	Mark { id: new_id("m"), category_id: None, page, x, y, w: 0.02, h: 0.02, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() }
}

const ARCH_D: (f32, f32) = (2592.0, 1728.0);

/// pdfium is one library for the whole process: one test at a time.
static PDFIUM: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
	PDFIUM.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn sheet_numbers_from_the_title_block() {
	let _one = serial();
	let d = dir("index");
	let path = d.join("set.pdf");
	pdf(&path, &[Sheet { number: "A-101", size: ARCH_D, boxes: &[] }, Sheet { number: "A2.01", size: ARCH_D, boxes: &[] }, Sheet { number: "S-001", size: (1224.0, 792.0), boxes: &[] }]);
	let idx = takeoff_pdf::sheets::index(&path).unwrap();
	let nums: Vec<_> = idx.iter().map(|k| k.number.clone().unwrap_or_default()).collect();
	assert_eq!(nums, vec!["A-101", "A2.01", "S-001"]);
	assert!(idx.iter().all(|k| k.source == NumberSource::TitleBlock));
	assert_eq!(idx[2].size_pt, (1224.0, 792.0));
	std::fs::remove_dir_all(d).ok();
}

#[test]
fn slip_sheet_swaps_pages_and_keeps_the_takeoff() {
	let _one = serial();
	let d = dir("apply");
	let plan = d.join("plan.pdf");
	let a101_boxes: &[(f32, f32, f32, f32)] = &[(0.1, 0.1, 0.2, 0.2)];
	pdf(&plan, &[Sheet { number: "A-101", size: ARCH_D, boxes: a101_boxes }, Sheet { number: "A-102", size: ARCH_D, boxes: &[(0.5, 0.5, 0.1, 0.1)] }, Sheet { number: "A-103", size: ARCH_D, boxes: &[] }]);
	// Revision B: A-103 first; A-101 with a new box at the bottom right (the
	// rest unchanged); A-102 reissued on wider paper; and a new sheet.
	let rev = d.join("plan rev B.pdf");
	pdf(&rev, &[Sheet { number: "A-103", size: ARCH_D, boxes: &[] }, Sheet { number: "A101", size: ARCH_D, boxes: &[(0.1, 0.1, 0.2, 0.2), (0.6, 0.6, 0.1, 0.1)] }, Sheet { number: "A-102", size: (3024.0, 1728.0), boxes: &[(0.5, 0.5, 0.1, 0.1)] }, Sheet { number: "A-104", size: ARCH_D, boxes: &[] }]);

	let current = takeoff_pdf::sheets::index(&plan).unwrap();
	let incoming = takeoff_pdf::sheets::index(&rev).unwrap();
	let m = match_sheets(&current, &incoming);
	assert_eq!(m.iter().map(|m| m.target).collect::<Vec<_>>(), vec![Some(2), Some(0), Some(1), None]);
	assert!(m[..3].iter().all(|m| m.by == Some(MatchBy::Number)));

	// The takeoff: a mark on A-101's unchanged box and one on its changed corner, a line on A-102, a scale.
	let mut data = TakeoffData::default();
	let (keep, check) = (mark(1, 0.15, 0.15), mark(1, 0.63, 0.63));
	let (keep_id, check_id) = (keep.id.clone(), check.id.clone());
	data.marks = vec![keep, check, mark(3, 0.4, 0.4)];
	data.measurements.push(Measurement::new("line", 2, MeasurementType::Length, vec![Point { x: 0.5, y: 0.5 }, Point { x: 0.6, y: 0.5 }]));
	data.page_scales.push(PageScale { page: 2, p1: Point { x: 0.1, y: 0.9 }, p2: Point { x: 0.2, y: 0.9 }, distance: 10.0, unit: "ft".into() });
	let doc = Document::new(data.clone(), vec![(ARCH_D.0 as f64, ARCH_D.1 as f64); 3], "t");
	let length_before = doc.data.measurements[0].value.unwrap();
	let before = data.clone();

	let pages: Vec<SlipPage> = m.iter().map(|x| SlipPage { file: rev.clone(), page: x.incoming as u32 + 1, to: x.target.map(|t| Placement::Replace(t as u32 + 1)).unwrap_or(Placement::Append), sheet: incoming[x.incoming].number.clone() }).collect();
	let out = slip_sheet(&plan, &mut data, &pages).unwrap();
	assert_eq!((out.replaced, out.added), (3, 1));
	assert_eq!(out.warnings.len(), 1, "A-102 changed paper: {:?}", out.warnings);

	// The drawing now has the new pages where the old ones were, plus A-104.
	let now = takeoff_pdf::sheets::index(&plan).unwrap();
	assert_eq!(now.iter().map(|k| k.number.clone().unwrap()).collect::<Vec<_>>(), vec!["A101", "A-102", "A-103", "A-104"]);
	assert_eq!(now[1].size_pt, (3024.0, 1728.0));

	// Same paper: positions untouched. Other paper: fitted, and lengths stay true.
	assert_eq!(data.marks, before.marks);
	let fit = 2592.0 / 3024.0;
	assert!((data.measurements[0].points[0].x - (0.5 * fit + (1.0 - fit) / 2.0)).abs() < 1e-9);
	let sizes: Vec<(f64, f64)> = now.iter().map(|k| k.size_pt).collect();
	let after = Document::new(data.clone(), sizes, "t");
	assert!((after.data.measurements[0].value.unwrap() - length_before).abs() < 1e-6, "{} vs {length_before}", after.data.measurements[0].value.unwrap());

	// History: the old pages in their own PDF with their takeoff, one entry per replaced sheet.
	let hist = out.history.clone().unwrap();
	assert!(hist.starts_with(d.join("plan.pdf.revisions")));
	let old = takeoff_pdf::sheets::index(&hist).unwrap();
	assert_eq!(old.iter().map(|k| k.number.clone().unwrap()).collect::<Vec<_>>(), vec!["A-101", "A-102", "A-103"]);
	assert_eq!(sidecar::load(&hist).unwrap().marks.len(), 3);
	assert_eq!(data.sheet_revisions.len(), 3);
	let r1 = data.previous_revision(1).unwrap();
	assert_eq!((r1.file_page, r1.sheet.as_deref(), r1.from_page), (1, Some("A101"), Some(2)));
	assert_eq!(takeoff_pdf::slip::history_path(&plan, r1), hist);
	assert!(data.previous_revision(2).unwrap().warning.is_some());
	assert!(data.previous_revision(4).is_none(), "a new sheet has no history");

	// The mark on the changed corner is flagged, the one on the unchanged box isn't.
	assert!(r1.review.contains(&check_id), "{:?}", r1.review);
	assert!(!r1.review.contains(&keep_id));
	assert!(r1.changed.unwrap() > 0.0);
	assert!(data.previous_revision(3).unwrap().review.is_empty(), "A-103 didn't change");

	// The sidecar keeps the history; an older reader (no such field) still loads it.
	sidecar::save(&plan, &data).unwrap();
	let json = std::fs::read_to_string(sidecar::sidecar_path(&plan)).unwrap();
	assert!(json.contains("\"sheetRevisions\"") && json.contains("\"filePage\": 1"));
	assert!(!json.contains("pdfState"));
	assert_eq!(sidecar::load(&plan).unwrap().sheet_revisions, data.sheet_revisions);
	std::fs::remove_dir_all(d).ok();
}

#[test]
fn compare_lines_up_other_paper() {
	let _one = serial();
	let d = dir("compare");
	let (a, b) = (d.join("a.pdf"), d.join("b.pdf"));
	pdf(&a, &[Sheet { number: "A-1", size: (3024.0, 1728.0), boxes: &[] }]);
	pdf(&b, &[Sheet { number: "A-1", size: ARCH_D, boxes: &[] }]);
	let (da, db) = (takeoff_pdf::Drawing::open(&a).unwrap(), takeoff_pdf::Drawing::open(&b).unwrap());
	let diff = takeoff_pdf::compare::compare_pages(&da, 1, &db, 1, 1200).unwrap();
	assert_eq!(diff.image.width(), 1200);
	assert!(diff.grid.cols > diff.grid.rows);
	std::fs::remove_dir_all(d).ok();
}

#[test]
fn sheet_numbers_on_a_real_set() {
	let _one = serial();
	let pdf = Path::new("/Users/office/Downloads/27-10 DRAWING_A.pdf");
	if !pdf.exists() {
		eprintln!("sample drawing missing; skipping");
		return;
	}
	let idx = takeoff_pdf::sheets::index(pdf).unwrap();
	for k in &idx {
		eprintln!("p{} {:?} {:?} ({})", k.page, k.number, k.title, k.source.label());
	}
	let num = |p: usize| idx[p - 1].number.clone().unwrap_or_default();
	assert_eq!((num(1).as_str(), num(6).as_str(), num(20).as_str(), num(33).as_str()), ("T1.01", "A2.01", "A8.06", "M2.11"));
}
