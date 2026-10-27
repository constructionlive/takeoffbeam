//! Spaces in the agent's item tools: `read_items` says which space each item
//! is in and filters by space; `summary` totals per space.

use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::{json, Value};
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_core::*;

fn blank_pdf(dir: &std::path::Path) -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
	let path = dir.join("drawing.pdf");
	doc.save_to_file(&path).unwrap();
	path
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
	vec![Point { x: x0, y: y0 }, Point { x: x1, y: y0 }, Point { x: x1, y: y1 }, Point { x: x0, y: y1 }]
}

/// The JSON list after read_items' first line.
fn listed(text: &str) -> Vec<Value> {
	serde_json::from_str(text.lines().nth(1).unwrap()).unwrap()
}

#[test]
fn items_know_their_space() {
	let root = std::env::temp_dir().join(format!("takeoff-agent-spaces-{}", std::process::id()));
	std::fs::create_dir_all(&root).unwrap();
	let pdf = blank_pdf(&root);
	let mut d = TakeoffData::default();
	d.categories.push(Category::new("c1", "Duplex", "#4CC2FF", CategoryKind::Count));
	d.spaces.push(Space { id: "u".into(), page: 1, name: "Unit 204".into(), color: "#34D399".into(), points: rect(0.0, 0.0, 0.5, 0.5), hidden: false, meta: Default::default() });
	d.spaces.push(Space { id: "k".into(), page: 1, name: "Kitchen".into(), color: "#FBBF24".into(), points: rect(0.0, 0.0, 0.2, 0.2), hidden: false, meta: Default::default() });
	for (i, x) in [0.1, 0.3, 0.8].iter().enumerate() {
		d.marks.push(Mark { id: format!("m{i}"), category_id: Some("c1".into()), page: 1, x: *x, y: 0.1, w: 0.01, h: 0.01, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() });
	}
	let doc = Arc::new(Mutex::new(Document::new(d, vec![(792.0, 612.0)], "t")));
	let mut ctx = ToolCtx::new(doc, takeoff_pdf::Drawing::open(&pdf).unwrap(), "claude-test", None);

	let all = listed(&tools::call(&mut ctx, "read_items", json!({})).unwrap().text);
	let spaces: Vec<Value> = all.iter().map(|r| r["space"].clone()).collect();
	assert_eq!(spaces, [json!("Unit 204 › Kitchen"), json!("Unit 204"), Value::Null]);
	let unit = listed(&tools::call(&mut ctx, "read_items", json!({"space": "unit 204"})).unwrap().text);
	assert_eq!(unit.len(), 2, "the unit and the kitchen inside it");
	let kitchen = listed(&tools::call(&mut ctx, "read_items", json!({"space": "k"})).unwrap().text);
	assert_eq!(kitchen.len(), 1);
	assert!(tools::call(&mut ctx, "read_items", json!({"space": "Garage"})).err().unwrap().contains("Existing: Unit 204"));

	let sum = tools::call(&mut ctx, "summary", json!({})).unwrap().text;
	assert!(sum.contains("By space"), "{sum}");
	assert!(sum.contains("Unit 204 › Kitchen (p1): Duplex 1 ea"), "{sum}");
	assert!(sum.contains("No space: Duplex 1 ea"), "{sum}");
	let sum = tools::call(&mut ctx, "summary", json!({"space": "Unit 204"})).unwrap().text;
	assert!(sum.contains("in Unit 204") && sum.contains("Duplex [count]: 2 ea"), "{sum}");
	std::fs::remove_dir_all(root).ok();
}
