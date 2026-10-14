//! One open drawing: the takeoff document (shared with the agent), the
//! render worker, page textures, the view (page, zoom, pan), selection and
//! autosave.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use parking_lot::Mutex;
use takeoff_core::snap::SnapIndex;
use takeoff_core::{Document, Point, TakeoffData};
use takeoff_pdf::outline::Bookmark;
use takeoff_pdf::search::Hit;
use takeoff_pdf::FracRect;

use crate::worker::{Opened, Req, Res, Worker};

pub struct SharpView {
	pub page: u32,
	pub region: FracRect,
	pub tex: TextureHandle,
	pub zoom: f32,
}

pub struct SearchState {
	pub query: String,
	pub match_case: bool,
	pub whole_word: bool,
	pub generation: u64,
	pub hits: Vec<Hit>,
	pub done: bool,
	pub active: Option<usize>,
}

pub struct OpenDoc {
	pub path: PathBuf,
	pub name: String,
	pub doc: Arc<Mutex<Document>>,
	pub worker: Worker,
	pub loaded: bool,
	pub page_sizes: Vec<(f64, f64)>,
	pub labels: Vec<Option<String>>,
	pub layers: Vec<String>,
	pub outline: Vec<Bookmark>,
	pub error: Option<String>,

	pub page: u32,
	/// Screen points per PDF point.
	pub zoom: f32,
	/// Page top-left relative to the canvas top-left, in screen points.
	pub pan: Vec2,
	pub fit_pending: bool,
	pub canvas_rect: Rect,

	pub base: HashMap<u32, TextureHandle>,
	base_requested: HashSet<u32>,
	pub sharp: Option<SharpView>,
	sharp_key: Option<(u32, [i64; 4], i64)>,
	view_changed_at: Instant,
	pub thumbs: HashMap<u32, TextureHandle>,
	thumbs_requested: HashSet<u32>,
	pub snap: Option<(u32, Arc<SnapIndex>)>,
	snap_requested: Option<u32>,

	pub selection: HashSet<String>,
	pub search: Option<SearchState>,
	last_saved_rev: u64,
	last_change: Option<(u64, Instant)>,
	pub save_error: Option<String>,
	/// Page history for Back/Forward.
	pub history: Vec<u32>,
}

impl OpenDoc {
	pub fn open(path: PathBuf, author: &str, ctx: &egui::Context) -> Result<Self, String> {
		let data: TakeoffData = takeoff_core::sidecar::load(&path).map_err(|e| e.to_string())?;
		let hidden: HashSet<String> = data.hidden_layers.iter().cloned().collect();
		let worker = Worker::spawn(path.clone(), hidden, ctx.clone());
		let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
		// Page sizes arrive with `Opened`; until then the document has none.
		let doc = Document::new(data, vec![], author);
		Ok(Self {
			path,
			name,
			doc: Arc::new(Mutex::new(doc)),
			worker,
			loaded: false,
			page_sizes: vec![],
			labels: vec![],
			layers: vec![],
			outline: vec![],
			error: None,
			page: 1,
			zoom: 1.0,
			pan: Vec2::ZERO,
			fit_pending: true,
			canvas_rect: Rect::NOTHING,
			base: HashMap::new(),
			base_requested: HashSet::new(),
			sharp: None,
			sharp_key: None,
			view_changed_at: Instant::now(),
			thumbs: HashMap::new(),
			thumbs_requested: HashSet::new(),
			snap: None,
			snap_requested: None,
			selection: HashSet::new(),
			search: None,
			last_saved_rev: 0,
			last_change: None,
			save_error: None,
			history: vec![],
		})
	}

	fn on_opened(&mut self, o: Opened) {
		let page_count_changed = o.page_sizes.len() != self.page_sizes.len();
		self.page_sizes = o.page_sizes.clone();
		self.labels = o.labels;
		self.layers = o.layers;
		self.outline = o.outline;
		{
			let mut d = self.doc.lock();
			let data = std::mem::take(&mut d.data);
			let author = d.author.clone();
			let rev_dirty = d.is_dirty();
			*d = Document::new(data, o.page_sizes, author);
			// Keep the layer registry (names) in the takeoff for platform consumers.
			for name in &self.layers {
				if !d.data.layers.iter().any(|l| &l.name == name) {
					d.data.layers.push(takeoff_core::LayerInfo { name: name.clone(), description: None });
				}
			}
			if !rev_dirty {
				d.mark_saved();
			}
		}
		self.loaded = true;
		self.page = self.page.clamp(1, self.page_count().max(1));
		if page_count_changed {
			self.thumbs.clear();
			self.thumbs_requested.clear();
		}
		self.base.clear();
		self.base_requested.clear();
		self.sharp = None;
		self.sharp_key = None;
		self.fit_pending = true;
	}

	pub fn page_count(&self) -> u32 {
		self.page_sizes.len() as u32
	}

	pub fn page_size(&self, page: u32) -> (f64, f64) {
		self.page_sizes.get(page.saturating_sub(1) as usize).copied().unwrap_or((612.0, 792.0))
	}

	pub fn page_label(&self, page: u32) -> String {
		match self.labels.get(page.saturating_sub(1) as usize).cloned().flatten() {
			Some(l) if l != page.to_string() => l,
			_ => format!("Page {page}"),
		}
	}

	/// Drain worker results; returns true when something changed visually.
	pub fn poll(&mut self, ctx: &egui::Context) -> bool {
		let mut changed = false;
		while let Ok(r) = self.worker.rx.try_recv() {
			changed = true;
			match r {
				Res::Opened(o) => self.on_opened(*o),
				Res::Base { page, image } => {
					let tex = ctx.load_texture(format!("base-{page}"), image, TextureOptions::LINEAR);
					self.base.insert(page, tex);
				}
				Res::View { page, region, generation: _, image } => {
					let tex = ctx.load_texture("sharp", image, TextureOptions::LINEAR);
					self.sharp = Some(SharpView { page, region, tex, zoom: self.zoom });
				}
				Res::Thumb { page, image } => {
					let tex = ctx.load_texture(format!("thumb-{page}"), image, TextureOptions::LINEAR);
					self.thumbs.insert(page, tex);
				}
				Res::Segments { page, segments } => {
					let (w, h) = self.page_size(page);
					self.snap = Some((page, Arc::new(SnapIndex::new(segments, w, h))));
				}
				Res::Search { generation, hits, done } => {
					if let Some(s) = &mut self.search {
						if s.generation == generation {
							s.hits.extend(hits);
							s.done = done;
						}
					}
				}
				Res::LayersApplied => {
					self.base.clear();
					self.base_requested.clear();
					self.sharp = None;
					self.sharp_key = None;
					self.thumbs.clear();
					self.thumbs_requested.clear();
				}
				Res::Error(e) => self.error = Some(e),
			}
		}
		changed
	}

	pub fn thumb(&mut self, page: u32) -> Option<TextureHandle> {
		if let Some(t) = self.thumbs.get(&page) {
			return Some(t.clone());
		}
		if self.thumbs_requested.insert(page) {
			self.worker.send(Req::Thumb { page, max_px: 280 });
		}
		None
	}

	pub fn ensure_snap(&mut self) -> Option<Arc<SnapIndex>> {
		match &self.snap {
			Some((p, idx)) if *p == self.page => Some(idx.clone()),
			_ => {
				if self.snap_requested != Some(self.page) {
					self.snap_requested = Some(self.page);
					self.worker.send(Req::Segments { page: self.page });
				}
				None
			}
		}
	}

	// ── View geometry ───────────────────────────────────────────────────

	pub fn page_rect(&self) -> Rect {
		let (w, h) = self.page_size(self.page);
		Rect::from_min_size(self.canvas_rect.min + self.pan, Vec2::new(w as f32 * self.zoom, h as f32 * self.zoom))
	}

	pub fn to_screen(&self, p: Point) -> Pos2 {
		let r = self.page_rect();
		Pos2::new(r.min.x + p.x as f32 * r.width(), r.min.y + p.y as f32 * r.height())
	}

	pub fn to_page(&self, s: Pos2) -> Point {
		let r = self.page_rect();
		Point { x: ((s.x - r.min.x) / r.width()) as f64, y: ((s.y - r.min.y) / r.height()) as f64 }
	}

	/// Screen points per page fraction, x and y.
	pub fn frac_scale(&self) -> (f32, f32) {
		let r = self.page_rect();
		(r.width(), r.height())
	}

	pub fn fit_page(&mut self) {
		let (w, h) = self.page_size(self.page);
		let avail = self.canvas_rect.size() - Vec2::splat(48.0);
		if avail.x <= 0.0 || avail.y <= 0.0 {
			return;
		}
		self.zoom = (avail.x / w as f32).min(avail.y / h as f32).clamp(0.02, 64.0);
		self.center_page();
	}

	pub fn fit_width(&mut self) {
		let (w, _) = self.page_size(self.page);
		self.zoom = ((self.canvas_rect.width() - 48.0) / w as f32).clamp(0.02, 64.0);
		self.center_page();
		self.pan.y = 24.0;
	}

	pub fn center_page(&mut self) {
		let (w, h) = self.page_size(self.page);
		let size = Vec2::new(w as f32, h as f32) * self.zoom;
		self.pan = (self.canvas_rect.size() - size) / 2.0;
		self.touch_view();
	}

	/// Zoom by `factor` keeping `anchor` (screen) fixed.
	pub fn zoom_at(&mut self, anchor: Pos2, factor: f32) {
		let new_zoom = (self.zoom * factor).clamp(0.02, 64.0);
		let f = new_zoom / self.zoom;
		let a = anchor - self.canvas_rect.min;
		self.pan = a - (a - self.pan) * f;
		self.zoom = new_zoom;
		self.touch_view();
	}

	/// Center the view on a page region (fractions), zooming so it fills ~60%.
	pub fn zoom_to(&mut self, page: u32, region: (f64, f64, f64, f64)) {
		self.go_to_page(page);
		let (w, h) = self.page_size(page);
		let (rw, rh) = ((region.2.max(0.01) * w) as f32, (region.3.max(0.01) * h) as f32);
		let avail = self.canvas_rect.size();
		self.zoom = (avail.x * 0.6 / rw).min(avail.y * 0.6 / rh).clamp(0.05, 24.0);
		let center = Vec2::new(((region.0 + region.2 / 2.0) * w) as f32, ((region.1 + region.3 / 2.0) * h) as f32) * self.zoom;
		self.pan = avail / 2.0 - center;
		self.fit_pending = false;
		self.touch_view();
	}

	pub fn go_to_page(&mut self, page: u32) {
		let page = page.clamp(1, self.page_count().max(1));
		if page != self.page {
			self.history.push(self.page);
			if self.history.len() > 50 {
				self.history.remove(0);
			}
			let keep_zoom = self.page_size(page) == self.page_size(self.page);
			self.page = page;
			self.selection.clear();
			if keep_zoom {
				self.touch_view();
			} else {
				self.fit_pending = true;
			}
		}
	}

	pub fn touch_view(&mut self) {
		self.view_changed_at = Instant::now();
	}

	/// Keep base + sharp renders in step with the view. Call once per frame.
	pub fn update_renders(&mut self, ctx: &egui::Context) {
		if !self.loaded || self.canvas_rect.width() <= 0.0 {
			return;
		}
		if self.fit_pending {
			self.fit_pending = false;
			self.fit_page();
		}
		let page = self.page;
		let max_tex = ctx.input(|i| i.max_texture_side).max(1024) as u32;
		let base_px = 2400.min(max_tex);
		if !self.base.contains_key(&page) && self.base_requested.insert(page) {
			self.worker.send(Req::Base { page, max_px: base_px });
			// Neighbours too, so paging feels instant.
			for p in [page + 1, page.saturating_sub(1)] {
				if p >= 1 && p <= self.page_count() && self.base_requested.insert(p) {
					self.worker.send(Req::Base { page: p, max_px: base_px });
				}
			}
		}
		// Sharp render of what's visible, once the view has settled.
		let ppp = ctx.pixels_per_point();
		let pr = self.page_rect();
		let vis = pr.intersect(self.canvas_rect);
		if vis.width() <= 1.0 || vis.height() <= 1.0 {
			return;
		}
		// Skip when the base texture is already sharp enough.
		if pr.width().max(pr.height()) * ppp <= base_px as f32 * 1.05 {
			self.sharp = None;
			return;
		}
		if self.view_changed_at.elapsed() < Duration::from_millis(140) {
			ctx.request_repaint_after(Duration::from_millis(150));
			return;
		}
		// Render a little beyond the visible area so small pans stay sharp.
		let pad = Vec2::new(vis.width(), vis.height()) * 0.15;
		let want = vis.expand2(pad).intersect(pr);
		let region = FracRect {
			x: ((want.min.x - pr.min.x) / pr.width()) as f64,
			y: ((want.min.y - pr.min.y) / pr.height()) as f64,
			w: (want.width() / pr.width()) as f64,
			h: (want.height() / pr.height()) as f64,
		};
		let key = (page, [region.x, region.y, region.w, region.h].map(|v| (v * 20_000.0) as i64), (self.zoom * 1000.0) as i64);
		if self.sharp_key == Some(key) {
			return;
		}
		self.sharp_key = Some(key);
		let mut px = ((want.width() * ppp) as u32, (want.height() * ppp) as u32);
		let cap = max_tex.min(6000) as f32 / px.0.max(px.1) as f32;
		if cap < 1.0 {
			px = ((px.0 as f32 * cap) as u32, (px.1 as f32 * cap) as u32);
		}
		self.worker.view(page, region, (px.0.max(1), px.1.max(1)));
	}

	pub fn set_hidden_layers(&mut self, hidden: HashSet<String>) {
		let mut v: Vec<String> = hidden.iter().cloned().collect();
		v.sort();
		self.doc.lock().edit("Layers", |d| d.hidden_layers = v);
		self.worker.send(Req::SetHidden(hidden));
	}

	pub fn hidden_layers(&self) -> HashSet<String> {
		self.doc.lock().data.hidden_layers.iter().cloned().collect()
	}

	pub fn start_search(&mut self, query: String, match_case: bool, whole_word: bool) {
		if query.trim().is_empty() {
			self.search = None;
			return;
		}
		let generation = self.worker.search(query.clone(), match_case, whole_word);
		self.search = Some(SearchState { query, match_case, whole_word, generation, hits: vec![], done: false, active: None });
	}

	// ── Autosave ────────────────────────────────────────────────────────

	/// Save 1.2 s after the last change (and never mid-gesture).
	pub fn autosave(&mut self, ctx: &egui::Context) {
		let (rev, dirty, gesture) = {
			let d = self.doc.lock();
			(d.revision(), d.is_dirty(), d.in_gesture())
		};
		if !dirty {
			self.last_change = None;
			return;
		}
		match self.last_change {
			Some((r, _)) if r == rev => {}
			_ => self.last_change = Some((rev, Instant::now())),
		}
		let (_, at) = self.last_change.unwrap();
		if gesture || at.elapsed() < Duration::from_millis(1200) {
			ctx.request_repaint_after(Duration::from_millis(300));
			return;
		}
		self.save_now();
	}

	pub fn save_now(&mut self) {
		let mut d = self.doc.lock();
		match takeoff_core::sidecar::save(&self.path, &d.data) {
			Ok(()) => {
				d.mark_saved();
				self.last_saved_rev = d.revision();
				self.save_error = None;
			}
			Err(e) => self.save_error = Some(e.to_string()),
		}
	}

	pub fn is_dirty(&self) -> bool {
		self.doc.lock().is_dirty()
	}

	/// After the PDF itself was rewritten (page tools): reopen it.
	pub fn reload_pdf(&mut self) {
		self.loaded = false;
		self.worker.send(Req::Reload);
	}
}
