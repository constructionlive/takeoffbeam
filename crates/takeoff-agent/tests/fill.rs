//! Spaces measured by a point inside them: the room the drawn walls make,
//! its doorway closed, and two rooms filled together as one.

use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::json;
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_core::{Document, MeasurementType, TakeoffData};

const W: f32 = 792.0;
const H: f32 = 612.0;
/// Line weight, pt.
const LW: f64 = 0.8;

/// Two rooms at 1/8" = 1'-0" (9 pt to the foot): x 100..400 and 400..600,
/// y 150..350 (top-down), the wall between them with a 3' (27 pt) doorway,
/// a door swing in it, and a room name.
fn fixture() -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(W), PdfPoints::new(H))).unwrap();
	let black = PdfColor::new(0, 0, 0, 255);
	let mut line = |a: (f64, f64), b: (f64, f64)| {
		page.objects_mut().create_path_object_line(PdfPoints::new(a.0 as f32), PdfPoints::new(H - a.1 as f32), PdfPoints::new(b.0 as f32), PdfPoints::new(H - b.1 as f32), black, PdfPoints::new(LW as f32)).unwrap();
	};
	line((100.0, 150.0), (600.0, 150.0));
	line((100.0, 350.0), (600.0, 350.0));
	line((100.0, 150.0), (100.0, 350.0));
	line((600.0, 150.0), (600.0, 350.0));
	line((400.0, 150.0), (400.0, 236.0));
	line((400.0, 263.0), (400.0, 350.0));
	// The door leaf, open into the right room (hinged at the lower jamb).
	line((400.0, 263.0), (427.0, 263.0));
	// A room name: a block of text-like strokes.
	for k in 0..6 {
		let x = 220.0 + k as f64 * 9.0;
		line((x, 245.0), (x + 6.0, 255.0));
	}
	let path = std::env::temp_dir().join(format!("takeoff-fill-{}.pdf", std::process::id()));
	doc.save_to_file(&path).unwrap();
	path
}

fn sf(pts_w: f64, pts_h: f64) -> f64 {
	pts_w * pts_h / 81.0
}

#[test]
fn a_point_in_a_room_measures_the_room() {
	let pdf = fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(W as f64, H as f64)], "AI")));
	let mut ctx = ToolCtx::new(doc.clone(), takeoff_pdf::Drawing::open(&pdf).unwrap(), "test-model", None);
	tools::call(&mut ctx, "set_scale_preset", json!({"page": 1, "preset": "1/8\" = 1'-0\""})).unwrap();
	let v = tools::call(&mut ctx, "view", json!({"page": 1})).unwrap();
	let img = &v.images[0];
	let (iw, ih) = (img.width as f64, img.height as f64);
	let at = |x: f64, y: f64| json!([(x / W as f64 * iw).round(), (y / H as f64 * ih).round()]);
	let out = tools::call(&mut ctx, "fill_area", json!({"view": img.view_id, "category": "Rooms", "items": [
		{"points": [at(160.0, 200.0)], "label": "Left"},
		{"points": [at(250.0, 250.0), at(500.0, 250.0)], "label": "Both"},
		{"points": [at(160.0, 200.0)], "gap": 0, "label": "Open"}
	]})).unwrap();
	println!("{}", out.text);
	if let (Ok(dir), Some(img)) = (std::env::var("FILL_SNAP_DIR"), out.images.first()) {
		std::fs::write(format!("{dir}/fill-agent.png"), &img.png).unwrap();
	}
	let d = doc.lock();
	let by = |label: &str| d.data.measurements.iter().find(|m| m.label.as_deref() == Some(label)).unwrap_or_else(|| panic!("no {label}"));
	let left = by("Left");
	assert_eq!(left.kind, MeasurementType::Area);
	// The room inside its walls, the doorway closed: within 1%.
	let room = sf(300.0 - LW, 200.0 - LW);
	let got = left.value.unwrap();
	assert!((got - room).abs() < room * 0.01, "left room {got:.1} sf vs {room:.1}");
	// A rectangle, give or take the doorway: few vertices.
	assert!(left.points.len() <= 12, "{} vertices: {:?}", left.points.len(), left.points.iter().map(|p| ((p.x * W as f64 * 10.0).round() / 10.0, (p.y * H as f64 * 10.0).round() / 10.0)).collect::<Vec<_>>());
	// Both rooms and the wall between.
	let both = sf(500.0 - LW, 200.0 - LW);
	let got = by("Both").value.unwrap();
	assert!((got - both).abs() < both * 0.01, "both rooms {got:.1} sf vs {both:.1}");
	// With no closing the fill runs through the doorway into both rooms.
	let open = by("Open").value.unwrap();
	assert!(open > room * 1.5, "open {open:.1}");
	let _ = std::fs::remove_file(&pdf);
}
