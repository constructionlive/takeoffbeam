//! The render worker: one thread owns the open `Drawing` (pdfium is not
//! re-entrant), renders pages/regions/thumbnails, reads vectors, searches.
//! The UI sends requests and drains results every frame; stale view renders
//! are dropped so panning never queues up work.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use takeoff_core::snap::Segment;
use takeoff_pdf::outline::Bookmark;
use takeoff_pdf::search::Hit;
use takeoff_pdf::{Drawing, FracRect};

pub enum Req {
	/// Base texture for a page (whole page, long edge `max_px`).
	Base { page: u32, max_px: u32 },
	/// Sharp render of a region at an exact pixel size.
	View { page: u32, region: FracRect, px: (u32, u32), generation: u64 },
	Thumb { page: u32, max_px: u32 },
	/// A small render of part of a page (graph view cards), named by `key`.
	Crop { key: String, page: u32, region: FracRect, max_px: u32 },
	Segments { page: u32 },
	/// Follow the line drawn from near `a` to near `b` in the page's picture.
	Trace { page: u32, a: takeoff_core::Point, b: takeoff_core::Point },
	/// Fill the space around these points (openings to try, points of paper).
	Fill { token: u64, page: u32, at: Vec<takeoff_core::Point>, gaps: Vec<f64> },
	Search { query: String, match_case: bool, whole_word: bool, generation: u64, from: u32 },
	SetHidden(HashSet<String>),
	Reload,
	/// Overlay this page with a page of another PDF (revision compare).
	Compare { page: u32, other: PathBuf, other_page: u32, max_px: u32 },
}

pub struct Opened {
	pub page_sizes: Vec<(f64, f64)>,
	pub labels: Vec<Option<String>>,
	pub layers: Vec<String>,
	pub default_off: Vec<String>,
	pub outline: Vec<Bookmark>,
}

pub enum Res {
	Opened(Box<Opened>),
	Base { page: u32, image: egui::ColorImage },
	View { page: u32, region: FracRect, generation: u64, image: egui::ColorImage },
	Thumb { page: u32, image: egui::ColorImage },
	Crop { key: String, image: egui::ColorImage },
	Segments { page: u32, segments: Vec<Segment> },
	Trace { page: u32, a: takeoff_core::Point, b: takeoff_core::Point, run: Option<Vec<takeoff_core::Point>> },
	/// None: the page could not be read.
	Fill { token: u64, fill: Option<Result<takeoff_pdf::fill::Fill, takeoff_pdf::fill::NoFill>> },
	Search { generation: u64, hits: Vec<Hit>, done: bool },
	LayersApplied,
	Compare { page: u32, image: egui::ColorImage, changed: f32 },
	Error(String),
}

pub struct Worker {
	tx: Sender<Req>,
	pub rx: Receiver<Res>,
	view_gen: Arc<AtomicU64>,
	search_gen: Arc<AtomicU64>,
}

fn to_color_image(img: image::RgbaImage) -> egui::ColorImage {
	let size = [img.width() as usize, img.height() as usize];
	egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
}

impl Worker {
	pub fn spawn(path: PathBuf, hidden: HashSet<String>, ctx: egui::Context) -> Self {
		let (tx, req_rx) = unbounded::<Req>();
		let (res_tx, rx) = unbounded::<Res>();
		let view_gen = Arc::new(AtomicU64::new(0));
		let search_gen = Arc::new(AtomicU64::new(0));
		let (vg, sg) = (view_gen.clone(), search_gen.clone());
		std::thread::Builder::new()
			.name("takeoff-render".into())
			.spawn(move || run(path, hidden, req_rx, res_tx, vg, sg, ctx))
			.expect("spawn render worker");
		Self { tx, rx, view_gen, search_gen }
	}

	pub fn send(&self, r: Req) {
		let _ = self.tx.send(r);
	}

	/// Request a sharp view render; supersedes any pending one.
	pub fn view(&self, page: u32, region: FracRect, px: (u32, u32)) -> u64 {
		let generation = self.view_gen.fetch_add(1, Ordering::SeqCst) + 1;
		self.send(Req::View { page, region, px, generation });
		generation
	}

	/// Sharp renders of several pages at once (continuous, spreads);
	/// together they supersede any pending.
	pub fn views(&self, views: Vec<(u32, FracRect, (u32, u32))>) {
		let generation = self.view_gen.fetch_add(1, Ordering::SeqCst) + 1;
		for (page, region, px) in views {
			self.send(Req::View { page, region, px, generation });
		}
	}

	pub fn search(&self, query: String, match_case: bool, whole_word: bool) -> u64 {
		let generation = self.search_gen.fetch_add(1, Ordering::SeqCst) + 1;
		self.send(Req::Search { query, match_case, whole_word, generation, from: 1 });
		generation
	}
}

fn run(path: PathBuf, hidden: HashSet<String>, rx: Receiver<Req>, tx: Sender<Res>, view_gen: Arc<AtomicU64>, search_gen: Arc<AtomicU64>, ctx: egui::Context) {
	let open = |hidden: &HashSet<String>| -> Result<Drawing, String> {
		let mut d = Drawing::open(&path).map_err(|e| e.to_string())?;
		d.set_hidden_layers(hidden).map_err(|e| e.to_string())?;
		Ok(d)
	};
	let mut d = match open(&hidden) {
		Ok(d) => d,
		Err(e) => {
			let _ = tx.send(Res::Error(format!("Could not open the PDF: {e}")));
			ctx.request_repaint();
			return;
		}
	};
	let send = |r: Res| {
		let _ = tx.send(r);
		ctx.request_repaint();
	};
	let describe = |d: &Drawing| {
		let n = d.page_count();
		Opened {
			page_sizes: (1..=n).map(|p| d.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect(),
			labels: (1..=n).map(|p| d.page_label(p).ok().flatten()).collect(),
			layers: d.layer_names(),
			default_off: d.default_off_layers(),
			outline: d.outline(),
		}
	};
	send(Res::Opened(Box::new(describe(&d))));

	let mut queue: Vec<Req> = Vec::new();
	loop {
		if queue.is_empty() {
			match rx.recv() {
				Ok(r) => queue.push(r),
				Err(_) => return,
			}
		}
		queue.extend(rx.try_iter());
		// Priority: layers > view > base > segments > search > thumbs.
		let pick = queue
			.iter()
			.enumerate()
			.min_by_key(|(_, r)| match r {
				Req::SetHidden(_) | Req::Reload | Req::Compare { .. } => 0,
				Req::View { .. } => 1,
				Req::Base { .. } => 2,
				Req::Segments { .. } | Req::Trace { .. } | Req::Fill { .. } => 3,
				Req::Search { .. } => 4,
				Req::Thumb { .. } | Req::Crop { .. } => 5,
			})
			.map(|(i, _)| i)
			.unwrap();
		let req = queue.remove(pick);
		match req {
			Req::SetHidden(h) => {
				if let Err(e) = d.set_hidden_layers(&h) {
					send(Res::Error(e.to_string()));
				}
				send(Res::LayersApplied);
			}
			Req::Reload => match open(d.hidden_layers()) {
				Ok(nd) => {
					d = nd;
					send(Res::Opened(Box::new(describe(&d))));
				}
				Err(e) => send(Res::Error(e)),
			},
			Req::View { page, region, px, generation } => {
				if generation != view_gen.load(Ordering::SeqCst) {
					continue;
				}
				let Ok((w_pt, _)) = d.page_size_pt(page) else { continue };
				// DPI that makes the region exactly px.0 wide.
				let dpi = px.0 as f32 / (w_pt * region.w as f32) * 72.0;
				match d.render(page, dpi, region, None) {
					Ok(r) => send(Res::View { page, region: r.region, generation, image: to_color_image(r.image) }),
					Err(e) => send(Res::Error(e.to_string())),
				}
			}
			Req::Compare { page, other, other_page, max_px } => match compare(&d, page, &other, other_page, max_px) {
				Ok((image, changed)) => send(Res::Compare { page, image, changed }),
				Err(e) => send(Res::Error(e)),
			},
			Req::Base { page, max_px } => {
				if let Ok(r) = d.render(page, 300.0, FracRect::FULL, Some(max_px)) {
					send(Res::Base { page, image: to_color_image(r.image) });
				}
			}
			Req::Thumb { page, max_px } => {
				if let Ok(r) = d.render(page, 30.0, FracRect::FULL, Some(max_px)) {
					send(Res::Thumb { page, image: to_color_image(r.image) });
				}
			}
			Req::Crop { key, page, region, max_px } => {
				if let Ok(r) = d.render(page, 150.0, region, Some(max_px)) {
					send(Res::Crop { key, image: to_color_image(r.image) });
				}
			}
			Req::Segments { page } => {
				let segments = d.segments(page).unwrap_or_default();
				send(Res::Segments { page, segments });
			}
			Req::Trace { page, a, b } => {
				let run = d.track_raster(page, a, b).ok().flatten();
				send(Res::Trace { page, a, b, run });
			}
			Req::Fill { token, page, at, gaps } => {
				let fill = d.fill_area(page, &at, &gaps).ok();
				send(Res::Fill { token, fill });
			}
			Req::Search { query, match_case, whole_word, generation, from } => {
				let n = d.page_count();
				let mut batch = Vec::new();
				if from > n {
					send(Res::Search { generation, hits: vec![], done: true });
				}
				for p in from..=n {
					if generation != search_gen.load(Ordering::SeqCst) {
						break;
					}
					batch.extend(d.search_page(p, &query, match_case, whole_word, 200).unwrap_or_default());
					if batch.len() >= 40 || p == n {
						send(Res::Search { generation, hits: std::mem::take(&mut batch), done: p == n });
					}
					if !rx.is_empty() {
						// Let views/thumbs through between pages; resume the search after.
						queue.extend(rx.try_iter());
						if queue.iter().any(|r| matches!(r, Req::View { .. } | Req::Base { .. } | Req::SetHidden(_))) {
							if !batch.is_empty() {
								send(Res::Search { generation, hits: std::mem::take(&mut batch), done: false });
							}
							queue.push(Req::Search { query: query.clone(), match_case, whole_word, generation, from: p + 1 });
							break;
						}
					}
				}
			}
		}
	}
}

/// Revision overlay: this page in red, the other in blue, shared ink black.
/// Both are rendered to the same pixel size so the sheets line up when they
/// share a paper size (the normal case for revisions).
fn compare(d: &Drawing, page: u32, other: &std::path::Path, other_page: u32, max_px: u32) -> Result<(egui::ColorImage, f32), String> {
	let a = d.render(page, 300.0, FracRect::FULL, Some(max_px)).map_err(|e| e.to_string())?;
	let o = Drawing::open(other).map_err(|e| format!("Could not open the other drawing: {e}"))?;
	let (ow, _) = o.page_size_pt(other_page).map_err(|e| e.to_string())?;
	let dpi = a.image.width() as f32 / ow * 72.0;
	let b = o.render(other_page, dpi, FracRect::FULL, None).map_err(|e| e.to_string())?;
	let b = if b.image.dimensions() == a.image.dimensions() {
		b.image
	} else {
		image::imageops::resize(&b.image, a.image.width(), a.image.height(), image::imageops::FilterType::Triangle)
	};
	let (w, h) = a.image.dimensions();
	let mut out = Vec::with_capacity((w * h * 4) as usize);
	let mut changed = 0u64;
	let lum = |p: &image::Rgba<u8>| (p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10;
	for (pa, pb) in a.image.pixels().zip(b.pixels()) {
		let (la, lb) = (lum(pa).min(255) as u8, lum(pb).min(255) as u8);
		if la.abs_diff(lb) > 60 {
			changed += 1;
		}
		// Only-this = red, only-other = blue, both = dark, neither = white.
		out.extend_from_slice(&[lb, la.min(lb), la, 255]);
	}
	let frac = changed as f32 / (w as f32 * h as f32);
	Ok((egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &out), frac))
}
