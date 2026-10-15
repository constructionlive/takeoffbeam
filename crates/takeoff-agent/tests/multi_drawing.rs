//! Several open drawings: the agent switches between them, and a location
//! read off a view always lands in the drawing that view came from.

use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::json;
use takeoff_agent::tools::{self, OtherDrawing, ToolCtx};
use takeoff_core::{Document, TakeoffData};

fn pdf(dir: &std::path::Path, name: &str, pages: usize) -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	for _ in 0..pages {
		doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
	}
	let path = dir.join(name);
	doc.save_to_file(&path).unwrap();
	path
}

fn new_doc(pages: usize) -> Arc<Mutex<Document>> {
	Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0); pages], "t")))
}

#[test]
fn views_carry_their_drawing() {
	let dir = std::env::temp_dir().join(format!("takeoff-multi-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	let (a_path, b_path) = (pdf(&dir, "Plans.pdf", 1), pdf(&dir, "Electrical.pdf", 3));
	let (a, b) = (new_doc(1), new_doc(3));
	let mut ctx = ToolCtx::new(a.clone(), takeoff_pdf::Drawing::open(&a_path).unwrap(), "claude-test", None);
	ctx.set_others(vec![OtherDrawing { path: b_path.clone(), doc: b.clone(), page: 2 }]).unwrap();

	let overview = tools::call(&mut ctx, "get_drawing", json!({})).unwrap().text;
	assert!(overview.contains("Electrical.pdf") && overview.contains("open_drawings"), "{overview}");

	let va = tools::call(&mut ctx, "view", json!({})).unwrap().images[0].view_id.clone();
	let out = tools::call(&mut ctx, "switch_drawing", json!({ "drawing": "electrical" })).unwrap();
	assert!(out.text.contains("\"pages\":3"), "now on the 3-page set: {}", out.text);
	assert!(out.text.contains("\"current_page\":2"), "starts on that tab's page: {}", out.text);
	let vb = tools::call(&mut ctx, "view", json!({})).unwrap();
	assert!(vb.images[0].caption.starts_with("[Electrical.pdf]"), "{}", vb.images[0].caption);
	let vb = vb.images[0].view_id.clone();

	// Active is Electrical, but a view of Plans puts the mark in Plans.
	tools::call(&mut ctx, "add_marks", json!({ "view": va, "boxes": [[100, 100, 140, 140]], "category": "Outlet" })).unwrap();
	tools::call(&mut ctx, "add_marks", json!({ "view": vb, "boxes": [[300, 300, 340, 340]], "category": "Panel" })).unwrap();
	assert_eq!(a.lock().data.marks.len(), 1);
	assert_eq!(b.lock().data.marks.len(), 1);
	assert_eq!(b.lock().data.marks[0].page, 2);
	assert!(a.lock().data.categories.iter().any(|c| c.name == "Outlet"));
	assert!(b.lock().data.categories.iter().any(|c| c.name == "Panel"));

	assert!(tools::call(&mut ctx, "switch_drawing", json!({ "drawing": "nope" })).is_err());

	// Next turn: the tab was closed, so its views no longer resolve and the
	// chat is back on its own drawing.
	ctx.set_others(vec![]).unwrap();
	assert!(tools::call(&mut ctx, "find_items_at", json!({ "view": vb, "point": [320, 320] })).is_err());
	let overview = tools::call(&mut ctx, "get_drawing", json!({})).unwrap().text;
	assert!(overview.contains("Plans.pdf") && !overview.contains("open_drawings"), "{overview}");
}
