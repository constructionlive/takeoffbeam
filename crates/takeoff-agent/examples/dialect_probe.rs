//! Which box convention does a local model really answer in? Renders a crop,
//! asks for boxes of obvious symbols, and scores each interpretation by how
//! much of each box lands on the symbol colour. Saves the burned crops.
//! `cargo run --release -p takeoff-agent --example dialect_probe -- <pdf> <page> <base_url> <model> <out_dir>`
use std::io::Cursor;

use serde_json::json;
use takeoff_agent::{llm, ProviderConfig};
use takeoff_core::coords::{dialect_for, image_fit, Dialect, RawBox};
use takeoff_pdf::{Drawing, FracRect};

fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let (pdf, page, base, model, out) = (&a[0], a[1].parse::<u32>().unwrap(), &a[2], &a[3], &a[4]);
	let region = FracRect { x: 0.10, y: 0.05, w: 0.20, h: 0.15 };
	let d = Drawing::open(pdf).unwrap();
	let r = d.render(page, 400.0, region, Some(1260)).unwrap();
	let (fw, fh) = image_fit(model, r.image.width(), r.image.height());
	let img = image::imageops::resize(&r.image, fw, fh, image::imageops::FilterType::Triangle);
	let mut png = Vec::new();
	img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
	let dialect = dialect_for(model);
	println!("model {model} → dialect {dialect:?}, image {fw}x{fh}");
	let p = ProviderConfig { base_url: base.clone(), api_key: None, model: model.clone(), context_tokens: 32768, vision: true };
	let prompt = format!(
		"The image is a section of a construction drawing. Find EVERY green dashed square symbol with an X inside it (shore post / wood block). Respond with ONLY a JSON array where each entry is a location plus a \"label\". {}",
		dialect.box_request(fw, fh)
	);
	let t = std::time::Instant::now();
	let msgs = vec![json!({"role": "user", "content": [{"type": "text", "text": prompt}, llm::image_part(&png, model)]})];
	let (reply, pt, ct) = llm::complete_with(&p, &msgs, Some(4096), true).unwrap();
	println!("reply in {:?} ({pt} prompt / {ct} completion tokens): {}", t.elapsed(), reply.chars().take(600).collect::<String>());
	let green = |px: &image::Rgba<u8>| px[1] > 100 && px[1] as i32 - px[0] as i32 > 40 && px[1] as i32 - px[2] as i32 > 40;
	for (name, interp) in [("as-dialect", dialect), ("yx-order", Dialect::Grid1000Yx), ("xy-order", Dialect::Grid1000Xy), ("pixel", Dialect::Pixel)] {
		let boxes: Vec<RawBox> = interp.parse_boxes(&reply);
		let mut canvas = img.clone();
		let mut hits = 0;
		for b in &boxes {
			let (x, y, w, h) = interp.box_to_fractions(b.clone(), fw as f64, fh as f64);
			let (x0, y0) = ((x * fw as f64).max(0.0) as u32, (y * fh as f64).max(0.0) as u32);
			let (x1, y1) = (((x + w) * fw as f64).min(fw as f64 - 1.0) as u32, ((y + h) * fh as f64).min(fh as f64 - 1.0) as u32);
			let mut g = 0;
			let mut n = 0;
			for yy in y0..=y1.max(y0) {
				for xx in x0..=x1.max(x0) {
					if xx < fw && yy < fh {
						n += 1;
						if green(img.get_pixel(xx, yy)) {
							g += 1;
						}
						if xx == x0 || xx == x1 || yy == y0 || yy == y1 {
							canvas.put_pixel(xx, yy, image::Rgba([255, 0, 255, 255]));
						}
					}
				}
			}
			if n > 0 && g as f64 / n as f64 > 0.04 {
				hits += 1;
			}
		}
		println!("  {name:>10}: {} boxes, {hits} land on green", boxes.len());
		canvas.save(format!("{out}/probe_{name}.png")).unwrap();
	}
}
