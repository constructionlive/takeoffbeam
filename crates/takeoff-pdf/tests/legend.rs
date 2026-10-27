//! Legends in renders and in the flattened PDF: the box, its swatches and
//! its text are drawn from the live takeoff. `LEGEND_PNG=out.png` writes the render.

use image::{Rgba, RgbaImage};
use takeoff_core::*;
use takeoff_pdf::burn::{burn, BurnOptions};
use takeoff_pdf::FracRect;

fn takeoff() -> TakeoffData {
	let mut d = TakeoffData::default();
	d.pages.push(PageInfo { page: 1, width_px: 2200.0, height_px: 1700.0 });
	d.categories.push(Category::new("c1", "Duplex", "#FF0000", CategoryKind::Count));
	for i in 0..3 {
		d.marks.push(Mark { id: format!("m{i}"), category_id: Some("c1".into()), page: 1, x: 0.1 + 0.05 * i as f64, y: 0.1, w: 0.01, h: 0.01, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() });
	}
	let mut l = Legend::new("lg1", 1, 0.5, 0.5, 0.4, 0.3);
	l.title = Some("Electrical legend".into());
	d.legends.push(l);
	d
}

#[test]
fn legends_burn_into_renders() {
	let d = takeoff();
	let blank = RgbaImage::from_pixel(800, 618, Rgba([255, 255, 255, 255]));
	let mut img = blank.clone();
	burn(&mut img, FracRect::FULL, (792.0, 612.0), &d, 1, &BurnOptions::default());
	if let Ok(out) = std::env::var("LEGEND_PNG") {
		img.save(out).unwrap();
	}
	let ink_in = |img: &RgbaImage, x0: f64, y0: f64, x1: f64, y1: f64, f: &dyn Fn(&Rgba<u8>) -> bool| {
		let (w, h) = (img.width() as f64, img.height() as f64);
		(((x0 * w) as u32)..((x1 * w) as u32)).flat_map(|x| (((y0 * h) as u32)..((y1 * h) as u32)).map(move |y| (x, y))).filter(|(x, y)| f(img.get_pixel(*x, *y))).count()
	};
	let dark = |p: &Rgba<u8>| p[0] < 120 && p[1] < 120 && p[2] < 120;
	let red = |p: &Rgba<u8>| p[0] > 200 && p[1] < 80 && p[2] < 80;
	assert!(ink_in(&img, 0.5, 0.5, 0.9, 0.8, &dark) > 200, "frame and text");
	assert!(ink_in(&img, 0.5, 0.5, 0.65, 0.8, &red) > 10, "the item's swatch");
	// Without markups in the options, no legend.
	let mut plain = blank.clone();
	burn(&mut plain, FracRect::FULL, (792.0, 612.0), &d, 1, &BurnOptions { annotations: false, ..Default::default() });
	assert_eq!(ink_in(&plain, 0.5, 0.5, 0.9, 0.8, &dark), 0);
}

/// One empty landscape page.
fn blank_pdf(path: &std::path::Path) {
	let objects = ["", "<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] >>"];
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

#[test]
fn legends_export_into_the_flattened_pdf() {
	if takeoff_pdf::pdfium().is_err() {
		eprintln!("pdfium missing; skipping");
		return;
	}
	let dir = std::env::temp_dir().join(format!("takeoff-legend-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let (src, dst) = (dir.join("a.pdf"), dir.join("b.pdf"));
	blank_pdf(&src);
	let mut d = takeoff();
	d.marks.clear();
	assert_eq!(takeoff_pdf::export::export_flattened(&src, &d, &dst).unwrap(), 1, "a page with only a legend is exported");
	let text = takeoff_pdf::Drawing::open(&dst).unwrap().text(1).unwrap();
	assert!(text.contains("Electrical legend") && text.contains("No items"), "{text}");
	d = takeoff();
	takeoff_pdf::export::export_flattened(&src, &d, &dst).unwrap();
	let text = takeoff_pdf::Drawing::open(&dst).unwrap().text(1).unwrap();
	assert!(text.contains("Duplex") && text.contains("3 ea"), "{text}");
	std::fs::remove_dir_all(dir).ok();
}
