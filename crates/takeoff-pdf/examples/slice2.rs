//! Slice-2 checks on real drawings: page tools, vectors, search, burn-in.
//! `cargo run --release -p takeoff-pdf --example slice2 -- <drawingA.pdf> <layered.pdf> <out_dir>`
use std::path::PathBuf;
use std::time::Instant;

use takeoff_core::*;
use takeoff_pdf::burn::{burn, burn_segments, BurnOptions};
use takeoff_pdf::tools::*;
use takeoff_pdf::{Drawing, FracRect};

fn count(p: &std::path::Path) -> u32 {
	Drawing::open(p).unwrap().page_count()
}

fn p(x: f64, y: f64) -> Point {
	Point { x, y }
}

fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let (src, layered, out) = (PathBuf::from(&a[0]), PathBuf::from(&a[1]), PathBuf::from(&a[2]));
	let n = count(&src);
	let t = Instant::now();
	extract_pages(&src, &[3, 1, 2], &out.join("extract.pdf")).unwrap();
	println!("extract 3 pages -> {} pages ({:?})", count(&out.join("extract.pdf")), t.elapsed());
	let t = Instant::now();
	let parts = split(&src, &SplitMode::EveryN(20), &out.join("split")).unwrap();
	println!("split every 20 -> {:?} ({:?})", parts.iter().map(|(_, a, b)| (a, b)).collect::<Vec<_>>(), t.elapsed());
	let files: Vec<PathBuf> = parts.iter().map(|(f, _, _)| f.clone()).collect();
	let t = Instant::now();
	let starts = merge(&files, &out.join("merged.pdf")).unwrap();
	println!("merge back -> {} pages (was {n}), starts {starts:?} ({:?})", count(&out.join("merged.pdf")), t.elapsed());
	rotate_pages(&out.join("extract.pdf"), &[1], 1, &out.join("rotated.pdf")).unwrap();
	let r = Drawing::open(out.join("rotated.pdf")).unwrap();
	println!("rotated page 1 size {:?} (page 2 {:?})", r.page_size_pt(1).unwrap(), r.page_size_pt(2).unwrap());
	delete_pages(&out.join("extract.pdf"), &[2], &out.join("deleted.pdf")).unwrap();
	insert_blank(&out.join("deleted.pdf"), 2, 1, (612.0, 792.0), &out.join("blank.pdf")).unwrap();
	println!("delete 1 of 3 -> {}, insert blank -> {}", count(&out.join("deleted.pdf")), count(&out.join("blank.pdf")));
	let d = Drawing::open(&src).unwrap();
	println!("outline: {} bookmarks, first {:?}", d.outline().len(), d.outline().first());
	let t = Instant::now();
	let mut hits = 0;
	for pg in 1..=n {
		hits += d.search_page(pg, "PLAN", false, false, 50).unwrap().len();
	}
	let first = d.search_page(3, "SITE PLAN", false, false, 5).unwrap();
	println!("search 'PLAN': {hits} hits over {n} pages ({:?}); p3 'SITE PLAN' {:?}", t.elapsed(), first.first());
	let runs = d.text_runs(3, Some(FracRect { x: 0.85, y: 0.8, w: 0.15, h: 0.2 }), 20).unwrap();
	println!("title-block runs: {:?}", runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>());

	// Vectors on the layered structural plan, overlaid to check alignment.
	let l = Drawing::open(&layered).unwrap();
	let t = Instant::now();
	let segs = l.segments(1).unwrap();
	println!("vectors: {} segments on page 1 ({:?})", segs.len(), t.elapsed());
	let region = FracRect { x: 0.1, y: 0.05, w: 0.2, h: 0.15 };
	let mut r = l.render(1, 200.0, region, None).unwrap();
	burn_segments(&mut r.image, r.region, &segs, (255, 0, 0));
	r.image.save(out.join("vectors.png")).unwrap();
	let t = Instant::now();
	let idx = takeoff_core::snap::SnapIndex::new(segs, 1000.0, 1000.0 * 1728.0 / 2592.0);
	let snap = idx.nearest(p(0.2, 0.1), 10.0);
	println!("snap index build+query {:?}: {snap:?}", t.elapsed());

	// Burn a sample takeoff with every markup kind.
	let mut data = TakeoffData::default();
	let mut cat = Category::new("c1", "Shore post", "#F87171", CategoryKind::Count);
	cat.icon = Some("circle".into());
	data.categories.push(cat);
	let mut area = Category::new("c2", "Slab pour", "#34D399", CategoryKind::Area);
	area.hatch = Some(Hatch::Diagonal);
	data.categories.push(area);
	for (i, (x, y)) in [(0.15, 0.08), (0.18, 0.08), (0.21, 0.081), (0.15, 0.12)].iter().enumerate() {
		data.marks.push(Mark { id: format!("m{i}"), category_id: Some("c1".into()), page: 1, x: *x, y: *y, w: 0.012, h: 0.016, source: Source::Ai, status: if i == 3 { MarkStatus::Pending } else { MarkStatus::Confirmed }, tile: None, note: None, fields: None, meta: Default::default() });
	}
	data.page_scales.push(PageScale { page: 1, p1: p(0.12, 0.17), p2: p(0.22, 0.17), distance: 20.0, unit: "ft".into() });
	let mut slab = Measurement::new("ms1", 1, MeasurementType::Area, vec![p(0.24, 0.07), p(0.29, 0.07), p(0.29, 0.13), p(0.24, 0.13)]);
	slab.category_id = Some("c2".into());
	slab.cutouts = vec![vec![p(0.255, 0.09), p(0.27, 0.09), p(0.27, 0.105), p(0.255, 0.105)]];
	data.measurements.push(slab);
	data.measurements.push(Measurement::new("ms2", 1, MeasurementType::Length, vec![p(0.12, 0.14), p(0.2, 0.15), p(0.22, 0.13)]));
	let ann = |id: &str, kind: AnnotationKind, pts: Vec<Point>, text: Option<&str>| Annotation { id: id.into(), page: 1, kind, points: pts, text: text.map(String::from), style: AnnotationStyle::default(), subject: None, note: None, source: None, meta: Default::default() };
	data.annotations.push(ann("a1", AnnotationKind::Cloud, vec![p(0.11, 0.06), p(0.16, 0.1)], None));
	data.annotations.push(ann("a2", AnnotationKind::Callout, vec![p(0.23, 0.15), p(0.29, 0.18), p(0.2, 0.12)], Some("Verify post spacing with shoring engineer")));
	data.annotations.push(ann("a3", AnnotationKind::Arrow, vec![p(0.12, 0.19), p(0.16, 0.175)], None));
	data.annotations.push(ann("a4", AnnotationKind::Highlight, vec![p(0.17, 0.185), p(0.22, 0.195)], None));
	let doc = Document::new(data, vec![l.page_size_pt(1).map(|(w, h)| (w as f64, h as f64)).unwrap()], "me");
	let mut r = l.render(1, 200.0, region, None).unwrap();
	let t = Instant::now();
	burn(&mut r.image, r.region, (0.0, 0.0), &doc.data, 1, &BurnOptions::default());
	println!("burn: {:?}", t.elapsed());
	r.image.save(out.join("burn.png")).unwrap();
	let t = Instant::now();
	let pages = takeoff_pdf::export::export_flattened(&layered, &doc.data, &out.join("flattened.pdf")).unwrap();
	println!("flattened export: {pages} page(s) in {:?}", t.elapsed());
	let f = Drawing::open(out.join("flattened.pdf")).unwrap();
	f.render(1, 200.0, region, None).unwrap().image.save(out.join("flattened.png")).unwrap();
}
