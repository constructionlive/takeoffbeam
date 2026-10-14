//! Slice-0 spike: how fast is pdfium on a real drawing set?
//! `cargo run --release -p takeoff-pdf --example spike -- plans.pdf [page] [out_dir]`
use std::time::Instant;

use takeoff_pdf::{Drawing, FracRect};

fn main() {
	let mut args = std::env::args().skip(1);
	let path = args.next().expect("usage: spike <file.pdf> [page] [out_dir]");
	let page: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(1);
	let out = args.next().unwrap_or_else(|| ".".into());

	let t = Instant::now();
	let d = Drawing::open(&path).expect("open");
	let n = d.page_count();
	println!("open: {n} pages in {:?}", t.elapsed());

	let t = Instant::now();
	let sizes: Vec<(f32, f32)> = (1..=n).map(|p| d.page_size_pt(p).unwrap()).collect();
	println!("all page sizes: {:?}", t.elapsed());
	// The cheap size must match what the renderer uses (rotation included).
	for p in [1, n / 2, n] {
		let r = d.render(p, 72.0, FracRect::FULL, None).unwrap();
		let (w, h) = sizes[(p - 1) as usize];
		assert_eq!((r.page_px.0, r.page_px.1), (w.round() as u32, h.round() as u32), "page {p}");
	}
	let (w, h) = d.page_size_pt(page).unwrap();
	println!("page {page}: {w:.0}×{h:.0} pt ({:.1}×{:.1} in), label {:?}", w / 72.0, h / 72.0, d.page_label(page).unwrap());

	for dpi in [72.0, 150.0, 200.0] {
		let t = Instant::now();
		let r = d.render(page, dpi, FracRect::FULL, None).unwrap();
		let el = t.elapsed();
		println!("render p{page} @{dpi} dpi: {}×{} in {el:?}", r.image.width(), r.image.height());
		if dpi == 200.0 {
			r.image.save(format!("{out}/p{page}_200.png")).unwrap();
		}
	}
	let t = Instant::now();
	let r = d.render(page, 200.0, FracRect { x: 0.6, y: 0.35, w: 0.3, h: 0.4 }, None).unwrap();
	println!("region render @200: {}×{} in {:?}", r.image.width(), r.image.height(), t.elapsed());
	r.image.save(format!("{out}/p{page}_region.png")).unwrap();
	let t = Instant::now();
	let r = d.render(page, 200.0, FracRect::FULL, Some(1568)).unwrap();
	println!("model-sized render (max 1568): {}×{} @{:.0} dpi in {:?}", r.image.width(), r.image.height(), r.dpi, t.elapsed());

	let t = Instant::now();
	let text = d.text(page).unwrap();
	println!("text: {} chars in {:?}", text.len(), t.elapsed());
	println!("  sample: {:?}", text.chars().take(300).collect::<String>());
	let tb = d.text_in(page, FracRect { x: 0.7, y: 0.85, w: 0.3, h: 0.15 }).unwrap();
	println!("title block text: {:?}", tb.chars().take(300).collect::<String>());

	// Thumbnails for a spread of pages.
	let t = Instant::now();
	let picks: Vec<u32> = (0..20).map(|i| 1 + i * (n.saturating_sub(1)) / 19).collect();
	for p in &picks {
		d.render(*p, 20.0, FracRect::FULL, Some(160)).unwrap();
	}
	println!("20 thumbnails (160px): {:?}", t.elapsed());
}
