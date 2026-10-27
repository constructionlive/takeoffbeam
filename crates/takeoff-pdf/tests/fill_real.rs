//! Fill on a real sheet, drawn over a render to look at:
//! FILL_PDF=plan.pdf FILL_PAGE=1 FILL_AT="0.42,0.37 0.5,0.5+0.52,0.5" FILL_OUT=dir cargo test -p takeoff-pdf --release --test fill_real -- --ignored --nocapture
//! (points joined by + fill together). Without FILL_AT it writes the page alone (page.png) to pick points from.

use takeoff_core::Point;
use takeoff_pdf::{Drawing, FracRect};

fn parse(group: &str) -> Vec<Point> {
	group.split('+').map(|at| {
		let v: Vec<f64> = at.split(',').map(|s| s.parse().unwrap()).collect();
		Point { x: v[0], y: v[1] }
	}).collect()
}

#[test]
#[ignore]
fn fills_on_a_real_sheet() {
	let Ok(pdf) = std::env::var("FILL_PDF") else { return };
	let page: u32 = std::env::var("FILL_PAGE").ok().and_then(|p| p.parse().ok()).unwrap_or(1);
	let out = std::path::PathBuf::from(std::env::var("FILL_OUT").unwrap_or_else(|_| ".".into()));
	let d = Drawing::open(&pdf).unwrap();
	let mut r = d.render(page, 150.0, FracRect::FULL, Some(4000)).unwrap();
	let (w, h) = (r.image.width() as f64, r.image.height() as f64);
	let ats = std::env::var("FILL_AT").unwrap_or_default();
	for (k, at) in ats.split_whitespace().enumerate() {
		let pts = parse(at);
		let at = pts[0];
		let t = std::time::Instant::now();
		let f = d.fill_area(page, &pts, &[]).unwrap();
		match f {
			Ok(f) => {
				println!("{k}: at {:.3},{:.3}: {} vertices, {:.0} pt² ({:.1} in² of paper), gap {} pt, {:?}", at.x, at.y, f.points.len(), f.area_pt2, f.area_pt2 / 5184.0, f.gap_pt, t.elapsed());
				let n = f.points.len();
				for i in 0..n {
					let (a, b) = (f.points[i], f.points[(i + 1) % n]);
					let steps = (((b.x - a.x) * w).hypot((b.y - a.y) * h) * 2.0) as usize + 1;
					for s in 0..=steps {
						let t = s as f64 / steps as f64;
						let (x, y) = ((a.x + (b.x - a.x) * t) * w, (a.y + (b.y - a.y) * t) * h);
						for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
							let (x, y) = (x as i64 + dx, y as i64 + dy);
							if x >= 0 && y >= 0 && x < w as i64 && y < h as i64 {
								r.image.put_pixel(x as u32, y as u32, image::Rgba([230, 30, 120, 255]));
							}
						}
					}
				}
			}
			Err(e) => println!("{k}: at {:.3},{:.3}: no fill ({e:?}), {:?}", at.x, at.y, t.elapsed()),
		}
		let (x, y) = ((at.x * w) as i64, (at.y * h) as i64);
		for dy in -6..=6 {
			for dx in -6..=6 {
				if dx * dx + dy * dy <= 36 && x + dx >= 0 && y + dy >= 0 && x + dx < w as i64 && y + dy < h as i64 {
					r.image.put_pixel((x + dx) as u32, (y + dy) as u32, image::Rgba([20, 120, 255, 255]));
				}
			}
		}
	}
	r.image.save(out.join(if ats.is_empty() { "page.png" } else { "fill.png" })).unwrap();
	// A close-up of each.
	for (k, at) in ats.split_whitespace().enumerate() {
		let pts = parse(at);
		let v = [pts[0].x, pts[0].y];
		let region = FracRect { x: (v[0] - 0.06).max(0.0), y: (v[1] - 0.06).max(0.0), w: 0.12, h: 0.12 };
		let mut c = d.render(page, 200.0, region, Some(1400)).unwrap();
		let (pw, ph) = (c.page_px.0 as f64, c.page_px.1 as f64);
		let to = |p: Point| ((p.x - c.region.x) * pw, (p.y - c.region.y) * ph);
		let (cw, chh) = (c.image.width() as i64, c.image.height() as i64);
		let dot = |img: &mut image::RgbaImage, x: f64, y: f64, col: [u8; 4], r: i64| {
			for dy in -r..=r {
				for dx in -r..=r {
					let (x, y) = (x as i64 + dx, y as i64 + dy);
					if x >= 0 && y >= 0 && x < cw && y < chh {
						img.put_pixel(x as u32, y as u32, image::Rgba(col));
					}
				}
			}
		};
		if let Ok(Ok(f)) = d.fill_area(page, &pts, &[]) {
			let n = f.points.len();
			for i in 0..n {
				let (a, b) = (to(f.points[i]), to(f.points[(i + 1) % n]));
				let steps = ((b.0 - a.0).hypot(b.1 - a.1) * 2.0) as usize + 1;
				for s in 0..=steps {
					let t = s as f64 / steps as f64;
					dot(&mut c.image, a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, [230, 30, 120, 255], 1);
				}
			}
		}
		for p in &pts {
			let s = to(*p);
			dot(&mut c.image, s.0, s.1, [20, 120, 255, 255], 5);
		}
		c.image.save(out.join(format!("fill{k}.png"))).unwrap();
	}
}
