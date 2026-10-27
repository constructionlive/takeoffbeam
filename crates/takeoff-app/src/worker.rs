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
	Compare { page: u32, image: egui::ColorImage, changed: f32, grid: takeoff_core::slip::ChangeGrid },
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
		// What the UI waits on, should the request panic (a pdfium or geometry bug on an odd file):
		// the worker reports it and carries on, so one bad page doesn't stop all rendering.
		let fallback = match &req {
			Req::Segments { page } => Some(Res::Segments { page: *page, segments: vec![] }),
			Req::Trace { page, a, b } => Some(Res::Trace { page: *page, a: *a, b: *b, run: None }),
			Req::Fill { token, .. } => Some(Res::Fill { token: *token, fill: None }),
			Req::Search { generation, .. } => Some(Res::Search { generation: *generation, hits: vec![], done: true }),
			Req::SetHidden(_) => Some(Res::LayersApplied),
			_ => None,
		};
		let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
						return;
					}
					let Ok((w_pt, _)) = d.page_size_pt(page) else { return };
					// DPI that makes the region exactly px.0 wide.
					let dpi = px.0 as f32 / (w_pt * region.w as f32) * 72.0;
					match d.render(page, dpi, region, None) {
						Ok(r) => send(Res::View { page, region: r.region, generation, image: to_color_image(r.image) }),
						Err(e) => send(Res::Error(e.to_string())),
					}
				}
				Req::Compare { page, other, other_page, max_px } => match compare(&d, page, &other, other_page, max_px) {
					Ok((image, changed, grid)) => send(Res::Compare { page, image, changed, grid }),
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
		}));
		if let Err(why) = handled {
			let why = why.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| why.downcast_ref::<String>().cloned()).unwrap_or_default();
			send(Res::Error(format!("The drawing could not be read there (internal error: {why})")));
			if let Some(r) = fallback {
				send(r);
			}
		}
	}
}

/// Revision overlay: this page in red, the other in blue, shared ink black,
/// and where the sheet changed (see `takeoff_pdf::compare`).
fn compare(d: &Drawing, page: u32, other: &std::path::Path, other_page: u32, max_px: u32) -> Result<(egui::ColorImage, f32, takeoff_core::slip::ChangeGrid), String> {
	let o = Drawing::open(other).map_err(|e| format!("Could not open the other drawing: {e}"))?;
	let diff = takeoff_pdf::compare::compare_pages(d, page, &o, other_page, max_px).map_err(|e| e.to_string())?;
	Ok((to_color_image(diff.image), diff.changed, diff.grid))
}
