//! On a real drawing: re-trace measurements along the drawn line and draw both.
//! TAKEOFF_REAL_PDF=… TAKEOFF_REAL_PAGE=20 TAKEOFF_REAL_OUT=dir cargo test -p takeoff-agent --test real_curves -- --ignored --nocapture

use takeoff_core::geometry::{polyline_length_px, scale_units_per_px};
use takeoff_core::snap::SnapIndex;
use takeoff_pdf::burn::burn_segments;
use takeoff_pdf::{Drawing, FracRect};

#[test]
#[ignore]
fn retrace_page_measurements() {
	let Ok(pdf) = std::env::var("TAKEOFF_REAL_PDF") else { return };
	let page: u32 = std::env::var("TAKEOFF_REAL_PAGE").ok().and_then(|p| p.parse().ok()).unwrap_or(1);
	let data = takeoff_core::sidecar::load(std::path::Path::new(&pdf)).unwrap();
	let d = Drawing::open(&pdf).unwrap();
	let (w, h) = d.page_size_pt(page).unwrap();
	let (w, h) = (w as f64, h as f64);
	let idx = SnapIndex::new(d.segments(page).unwrap(), w, h);
	println!("{} drawn segments on page {page}", idx.segments.len());
	let mut drawn = Vec::new();
	for m in data.measurements.iter().filter(|m| m.page == page && m.kind == takeoff_core::MeasurementType::Length) {
		let mut out: Vec<takeoff_core::Point> = Vec::new();
		let mut missed = 0;
		for p in m.points.windows(2) {
			let run = idx.track(p[0], p[1]).or_else(|| d.track_raster(page, p[0], p[1]).ok().flatten()).unwrap_or_else(|| {
				missed += 1;
				vec![p[0], p[1]]
			});
			let skip = usize::from(!out.is_empty());
			out.extend(run.into_iter().skip(skip));
		}
		let upp = data.scale_at(page, m.points[0]).and_then(|s| scale_units_per_px(s, w, h)).unwrap_or(f64::NAN);
		let (before, after) = (polyline_length_px(&m.points, w, h) * upp, polyline_length_px(&out, w, h) * upp);
		println!("{}: straight {before:.1} → along the line {after:.1} ({} pts, {missed} spans missed) · {}", m.id, out.len(), m.label.as_deref().unwrap_or(""));
		drawn.push((m.points.clone(), out));
	}
	if let Ok(dir) = std::env::var("TAKEOFF_REAL_OUT") {
		for (k, region) in [FracRect { x: 0.0, y: 0.1, w: 1.0, h: 0.2 }, FracRect { x: 0.05, y: 0.15, w: 0.3, h: 0.1 }].into_iter().enumerate() {
			let r = d.render(page, 150.0, region, Some(3000)).unwrap();
			let mut img = r.image;
			for (before, after) in &drawn {
				let segs = |pts: &[takeoff_core::Point]| pts.windows(2).map(|p| takeoff_core::snap::Segment { a: p[0], b: p[1] }).collect::<Vec<_>>();
				burn_segments(&mut img, r.region, &segs(before), (0x4C, 0xC2, 0xFF));
				burn_segments(&mut img, r.region, &segs(after), (0xF9, 0x73, 0x16));
			}
			img.save(format!("{dir}/real-{k}.png")).unwrap();
		}
	}
}

/// Points placed ON a coloured line in the picture (found by colour), then
/// followed: the run should hug that line.
#[test]
#[ignore]
fn follow_a_coloured_line_in_the_picture() {
	let Ok(pdf) = std::env::var("TAKEOFF_REAL_PDF") else { return };
	let page: u32 = std::env::var("TAKEOFF_REAL_PAGE").ok().and_then(|p| p.parse().ok()).unwrap_or(1);
	let d = Drawing::open(&pdf).unwrap();
	let (w, h) = d.page_size_pt(page).unwrap();
	let (w, h) = (w as f64, h as f64);
	// Blue ink in a column of the page: (y fractions of each blue run's middle).
	let blue_runs = |x: f64| -> Vec<f64> {
		let r = d.render(page, 288.0, FracRect { x, y: 0.1, w: 0.0015, h: 0.2 }, None).unwrap();
		let img = &r.image;
		let mut runs = Vec::new();
		let mut start: Option<u32> = None;
		for y in 0..img.height() {
			let p = img.get_pixel(img.width() / 2, y).0;
			let blue = p[2] as i32 - p[0] as i32 > 60 && p[2] > 120;
			match (blue, start) {
				(true, None) => start = Some(y),
				(false, Some(s)) => {
					runs.push(0.1 + (s + y) as f64 / 2.0 / r.page_px.1 as f64);
					start = None;
				}
				_ => {}
			}
		}
		runs
	};
	let (xa, xb) = (0.10, 0.20);
	let (ra, rb) = (blue_runs(xa), blue_runs(xb));
	println!("blue lines at x={xa}: {ra:.4?}\nblue lines at x={xb}: {rb:.4?}");
	let (Some(&ya), Some(&yb)) = (ra.first(), rb.first()) else { return };
	let (a, b) = (takeoff_core::Point { x: xa, y: ya }, takeoff_core::Point { x: xb, y: yb });
	let run = d.track_raster(page, a, b).unwrap();
	let chord = polyline_length_px(&[a, b], w, h);
	match &run {
		Some(r) => println!("followed: {} pts, {:.1} pt (straight {:.1} pt), max off the straight line {:.2} pt", r.len(), polyline_length_px(r, w, h), chord, r.iter().map(|p| takeoff_core::geometry::point_segment_distance_px(*p, a, b, w, h)).fold(0.0, f64::max)),
		None => println!("not followed"),
	}
	if let (Ok(dir), Some(r)) = (std::env::var("TAKEOFF_REAL_OUT"), &run) {
		let region = FracRect { x: xa - 0.01, y: ya.min(yb) - 0.02, w: xb - xa + 0.02, h: (ya - yb).abs() + 0.04 };
		let rr = d.render(page, 288.0, region, Some(4000)).unwrap();
		let mut img = rr.image;
		let segs: Vec<_> = r.windows(2).map(|p| takeoff_core::snap::Segment { a: p[0], b: p[1] }).collect();
		burn_segments(&mut img, rr.region, &segs, (0xF9, 0x73, 0x16));
		img.save(format!("{dir}/real-follow.png")).unwrap();
	}
	assert!(run.is_some());
}
