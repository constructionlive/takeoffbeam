//! Curved runs: points on a drawn curve joined by following the drawing's own
//! (dashed) line, or smoothed through, come out the length of the curve;
//! straight ones cut the corner.

use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::json;
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_core::geometry::polyline_length_px;
use takeoff_core::{Document, TakeoffData};

const W: f32 = 792.0;
const H: f32 = 612.0;
/// The track: a quarter circle about (200, 100) pt (top-down page coordinates), radius 400.
const CX: f64 = 200.0;
const CY: f64 = 100.0;
const R: f64 = 400.0;

fn arc_pt(deg: f64, r: f64) -> (f64, f64) {
	let a = deg.to_radians();
	(CX + r * a.cos(), CY + r * a.sin())
}

/// A dashed curved rail, a solid one 5 pt outside it, and a road crossing both.
fn fixture() -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(W), PdfPoints::new(H))).unwrap();
	let black = PdfColor::new(0, 0, 0, 255);
	let mut line = |a: (f64, f64), b: (f64, f64)| {
		page.objects_mut().create_path_object_line(PdfPoints::new(a.0 as f32), PdfPoints::new(H - a.1 as f32), PdfPoints::new(b.0 as f32), PdfPoints::new(H - b.1 as f32), black, PdfPoints::new(0.8)).unwrap();
	};
	for (r, dashed) in [(R, true), (R + 5.0, false)] {
		let n = 270;
		for k in 0..n {
			if dashed && k % 9 >= 6 {
				continue;
			}
			line(arc_pt(k as f64 / n as f64 * 90.0, r), arc_pt((k + 1) as f64 / n as f64 * 90.0, r));
		}
	}
	line(arc_pt(45.0, R - 80.0), arc_pt(45.0, R + 80.0));
	let path = std::env::temp_dir().join(format!("takeoff-curves-{}.pdf", std::process::id()));
	doc.save_to_file(&path).unwrap();
	path
}

#[test]
fn a_curve_is_followed_along_its_dashed_line() {
	let pdf = fixture();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(W as f64, H as f64)], "AI")));
	let mut ctx = ToolCtx::new(doc.clone(), takeoff_pdf::Drawing::open(&pdf).unwrap(), "test-model", None);
	let v = tools::call(&mut ctx, "view", json!({"page": 1})).unwrap();
	let img = &v.images[0];
	let (iw, ih) = (img.width as f64, img.height as f64);
	// Points read off the view, a little off the line as a model would: start, middle, end.
	let at = |deg: f64, r: f64| {
		let (x, y) = arc_pt(deg, r);
		json!([(x / W as f64 * iw).round(), (y / H as f64 * ih).round()])
	};
	let pts = json!([at(1.0, R + 1.0), at(44.0, R - 1.0), at(89.0, R + 1.0)]);
	let mut lengths = Vec::new();
	for path in ["straight", "follow", "smooth"] {
		let out = tools::call(&mut ctx, "add_measurements", json!({"view": img.view_id, "items": [{"type": "length", "points": pts, "path": path}], "category": format!("Track {path}")})).unwrap();
		if path == "follow" {
			assert!(out.text.contains("Followed the drawn line on 2 of 2 span(s)"), "{}", out.text);
			println!("{}", out.text);
			if let (Ok(dir), Some(img)) = (std::env::var("CURVE_SNAP_DIR"), out.images.first()) {
				std::fs::write(format!("{dir}/curve-follow.png"), &img.png).unwrap();
			}
		}
		let d = doc.lock();
		let m = d.data.measurements.last().unwrap();
		lengths.push(polyline_length_px(&m.points, W as f64, H as f64));
		if path == "follow" {
			// It kept to the dashed rail all along, not the solid one beside it.
			for q in &m.points {
				let r = (q.x * W as f64 - CX).hypot(q.y * H as f64 - CY);
				assert!((r - R).abs() < 1.6, "off the rail: r = {r:.2}");
			}
		}
	}
	let arc = R * 88.0_f64.to_radians();
	let (straight, follow, smooth) = (lengths[0], lengths[1], lengths[2]);
	println!("arc {arc:.1} pt · straight {straight:.1} · follow {follow:.1} · smooth {smooth:.1}");
	assert!((follow - arc).abs() < arc * 0.004, "follow {follow:.1} vs {arc:.1}");
	assert!((smooth - arc).abs() < (straight - arc).abs() / 2.0, "smooth {smooth:.1} beats straight {straight:.1}");
	assert!(straight < arc - 5.0, "straight cuts the corner");
	std::fs::remove_file(pdf).ok();
}
