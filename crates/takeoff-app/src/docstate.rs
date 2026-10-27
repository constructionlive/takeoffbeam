//! One open drawing: the takeoff document (shared with the agent), the
//! render worker, page textures, the view (page, zoom, pan), selection and
//! autosave.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use parking_lot::Mutex;
use takeoff_core::docmap::{self, DocMap, NodeRef, PartKind, Region};
use takeoff_core::snap::SnapIndex;
use takeoff_core::{Document, Point, TakeoffData};
use takeoff_pdf::outline::Bookmark;
use takeoff_pdf::search::Hit;
use takeoff_pdf::FracRect;

use crate::layout::{Arrangement, PageLayout};
use crate::worker::{Opened, Req, Res, Worker};

pub struct SharpView {
	pub page: u32,
	pub region: FracRect,
	pub tex: TextureHandle,
	pub zoom: f32,
}

pub struct CompareState {
	pub other: PathBuf,
	pub other_page: u32,
	pub page: u32,
	pub tex: Option<TextureHandle>,
	/// Share of pixels that differ (0–1).
	pub changed: f32,
	pub show_overlay: bool,
	/// Against the page's previous revision (follows to the next revised page).
	pub revision: bool,
	/// Items on the parts of the sheet that changed.
	pub flagged: Vec<String>,
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

/// What the central area shows: the sheet, or the map of the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
	#[default]
	Page,
	Graph,
}

/// One place in the map trail: a node, and where the sheet view was when
/// we were there (restored exactly on Back, like a previous view).
#[derive(Debug, Clone)]
pub struct Stop {
	pub node: String,
	pub view: Option<(u32, f32, Vec2)>,
}

/// Moving around the document map: what the Map tab shows and the trail
/// behind it (Back / Forward, the breadcrumb).
#[derive(Debug, Default)]
pub struct MapNav {
	pub trail: Vec<Stop>,
	pub pos: usize,
	/// What the Map tab shows when set; None = the sheet on screen.
	pub focus: Option<String>,
	/// The node just arrived at, outlined on the sheet for a moment.
	pub flash: Option<(String, Instant)>,
}

/// What the map shows, shared by the graph, the Pages list and the sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct MapFilter {
	/// Empty = every discipline.
	pub disciplines: BTreeSet<String>,
	/// Area kinds shown as cards in the graph.
	pub kinds: HashSet<PartKind>,
}

impl Default for MapFilter {
	fn default() -> Self {
		Self { disciplines: BTreeSet::new(), kinds: PartKind::ALL.into_iter().filter(|k| *k != PartKind::TitleBlock).collect() }
	}
}

impl MapFilter {
	/// Whether a sheet of this discipline passes.
	pub fn sheet(&self, discipline: Option<&str>) -> bool {
		self.disciplines.is_empty() || self.disciplines.contains(&discipline_key(discipline))
	}
}

/// The discipline a sheet is filed under, tidied: "civil/track" and
/// "CIVIL - Track" both read "Civil/Track" ("Other" when the AI gave none).
/// The part before the slash is its group.
pub fn discipline_key(d: Option<&str>) -> String {
	let d = d.map(str::trim).filter(|d| !d.is_empty()).unwrap_or("Other");
	let title = |w: &str| {
		let w = w.trim();
		let mut c = w.chars();
		c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
	};
	d.replace(" - ", "/").replace(['\\', '|', ':', '>'], "/").split('/').map(title).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("/")
}

/// "Civil/Track" → ("Civil", Some("Track")).
pub fn discipline_parts(key: &str) -> (&str, Option<&str>) {
	match key.split_once('/') {
		Some((g, s)) => (g, Some(s)),
		None => (key, None),
	}
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

	/// The current page: the one in front of you (under the view's centre
	/// when continuous), or the one a tool is working on.
	pub page: u32,
	/// Screen points per PDF point.
	pub zoom: f32,
	/// Page top-left relative to the canvas top-left, in screen points
	/// (continuous: the layout's origin; spreads: the row's top-left).
	pub pan: Vec2,
	/// Single page, continuous, spreads (view only).
	pub layout: PageLayout,
	/// Where every page sits for `layout`.
	pub arr: Arrangement,
	/// The view the current page was last picked for (continuous).
	followed: Option<(Vec2, f32, Rect)>,
	pub fit_pending: bool,
	pub canvas_rect: Rect,

	pub base: HashMap<u32, TextureHandle>,
	/// Base renders asked for: page → long edge in pixels.
	base_requested: HashMap<u32, u32>,
	/// Base renders on their way (continuous asks a few at a time).
	base_inflight: HashMap<u32, Instant>,
	pub sharp: Option<SharpView>,
	/// Sharp renders of the other pages on screen (continuous, spreads).
	pub side_sharp: HashMap<u32, SharpView>,
	sharp_key: Option<Vec<(u32, [i64; 4], i64)>>,
	view_changed_at: Instant,
	pub thumbs: HashMap<u32, TextureHandle>,
	thumbs_requested: HashSet<u32>,
	/// Snap indexes of the last few pages (the pointer moves between pages when continuous).
	pub snap: Vec<(u32, Arc<SnapIndex>)>,
	snap_requested: HashSet<u32>,

	pub selection: HashSet<String>,
	pub search: Option<SearchState>,
	pub compare: Option<CompareState>,
	last_saved_rev: u64,
	last_change: Option<(u64, Instant)>,
	pub save_error: Option<String>,
	/// Page history for Back/Forward.
	pub history: Vec<u32>,

	pub view_mode: ViewMode,
	/// The document map, from `plan.pdf.map.json` (None until one is built).
	/// The AI writes the file as it maps; it is read again when it changes.
	pub map: Option<DocMap>,
	map_stamp: Option<std::time::SystemTime>,
	map_checked: Instant,
	/// Why the last map action failed.
	pub map_status: Option<(String, bool)>,
	pub graph: crate::graph_view::GraphState,
	pub nav: MapNav,
	pub map_filter: MapFilter,
	/// Lines followed in the page's picture, by (page, ends); None = no line.
	traced: HashMap<(u32, [u64; 4]), Option<Vec<Point>>>,
	traced_requested: HashSet<(u32, [u64; 4])>,
	/// Fills asked of the worker, by token, once done.
	filled: HashMap<u64, Option<Result<takeoff_pdf::fill::Fill, takeoff_pdf::fill::NoFill>>>,
	fill_seq: u64,
	/// Graph view card images, by key.
	pub crops: HashMap<String, TextureHandle>,
	crops_requested: HashSet<String>,
	/// The PDF file that goes with each undo step (page tools, slip-sheets).
	pub versions: crate::pdf_undo::PdfVersions,
	/// Something to tell the user once (the app shows it as a toast).
	pub notice: Option<(String, bool)>,
}

impl OpenDoc {
	pub fn open(path: PathBuf, author: &str, ctx: &egui::Context) -> Result<Self, String> {
		let data: TakeoffData = takeoff_core::sidecar::load(&path).map_err(|e| e.to_string())?;
		let hidden: HashSet<String> = data.hidden_layers.iter().cloned().collect();
		let worker = Worker::spawn(path.clone(), hidden, ctx.clone());
		let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
		// Page sizes arrive with `Opened`; until then the document has none.
		let doc = Document::new(data, vec![], author);
		let map = takeoff_core::docmap::load(&path);
		let map_stamp = map_stamp(&path);
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
			layout: PageLayout::SINGLE,
			arr: Arrangement::default(),
			followed: None,
			fit_pending: true,
			canvas_rect: Rect::NOTHING,
			base: HashMap::new(),
			base_requested: HashMap::new(),
			base_inflight: HashMap::new(),
			sharp: None,
			side_sharp: HashMap::new(),
			sharp_key: None,
			view_changed_at: Instant::now(),
			thumbs: HashMap::new(),
			thumbs_requested: HashSet::new(),
			snap: vec![],
			snap_requested: HashSet::new(),
			selection: HashSet::new(),
			search: None,
			compare: None,
			last_saved_rev: 0,
			last_change: None,
			save_error: None,
			history: vec![],
			view_mode: ViewMode::Page,
			map,
			map_stamp,
			map_checked: Instant::now(),
			map_status: None,
			graph: Default::default(),
			nav: MapNav::default(),
			map_filter: MapFilter::default(),
			crops: HashMap::new(),
			crops_requested: HashSet::new(),
			traced: HashMap::new(),
			traced_requested: HashSet::new(),
			filled: HashMap::new(),
			fill_seq: 0,
			versions: Default::default(),
			notice: None,
		})
	}

	fn on_opened(&mut self, o: Opened) {
		self.page_sizes = o.page_sizes.clone();
		self.labels = o.labels;
		self.layers = o.layers;
		self.outline = o.outline;
		{
			// Same document, so the undo history survives the PDF being rewritten.
			let mut d = self.doc.lock();
			d.set_page_sizes(o.page_sizes);
			// Keep the layer registry (names) in the takeoff for platform consumers.
			for name in &self.layers {
				if !d.data.layers.iter().any(|l| &l.name == name) {
					d.data.layers.push(takeoff_core::LayerInfo { name: name.clone(), description: None });
				}
			}
		}
		self.loaded = true;
		self.page = self.page.clamp(1, self.page_count().max(1));
		self.arr = Arrangement::new(&self.page_sizes, self.layout);
		// Pages may show something else now (rotated, replaced), even at the same count.
		self.thumbs.clear();
		self.thumbs_requested.clear();
		self.crops.clear();
		self.crops_requested.clear();
		self.base.clear();
		self.base_requested.clear();
		self.base_inflight.clear();
		self.sharp = None;
		self.side_sharp.clear();
		self.sharp_key = None;
		self.snap.clear();
		self.snap_requested.clear();
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
		// Undo/redo across a page edit or slip-sheet: put that step's PDF back.
		let want = self.doc.lock().data.pdf_state;
		match self.versions.sync(&self.path, want) {
			Ok(true) => {
				self.compare = None;
				self.reload_pdf();
			}
			Ok(false) => {}
			Err(e) => self.notice = Some((e, true)),
		}
		while let Ok(r) = self.worker.rx.try_recv() {
			changed = true;
			match r {
				Res::Opened(o) => self.on_opened(*o),
				Res::Base { page, image } => {
					let tex = ctx.load_texture(format!("base-{page}"), image, TextureOptions::LINEAR);
					self.base.insert(page, tex);
					self.base_inflight.remove(&page);
				}
				Res::View { page, region, generation: _, image } => {
					let tex = ctx.load_texture(format!("sharp-{page}"), image, TextureOptions::LINEAR);
					let view = SharpView { page, region, tex, zoom: self.zoom };
					if page == self.page {
						// The page that was current is still on screen beside it.
						if let Some(old) = self.sharp.replace(view).filter(|o| o.page != page) {
							self.side_sharp.insert(old.page, old);
						}
						self.side_sharp.remove(&page);
					} else {
						self.side_sharp.insert(page, view);
					}
				}
				Res::Thumb { page, image } => {
					let tex = ctx.load_texture(format!("thumb-{page}"), image, TextureOptions::LINEAR);
					self.thumbs.insert(page, tex);
				}
				Res::Crop { key, image } => {
					let tex = ctx.load_texture(format!("crop-{key}"), image, TextureOptions::LINEAR);
					self.crops.insert(key, tex);
				}
				Res::Trace { page, a, b, run } => {
					self.traced.insert(trace_key(page, a, b), run);
					changed = true;
				}
				Res::Fill { token, fill } => {
					self.filled.insert(token, fill);
				}
				Res::Segments { page, segments } => {
					let (w, h) = self.page_size(page);
					self.snap.retain(|(p, _)| *p != page);
					self.snap.insert(0, (page, Arc::new(SnapIndex::new(segments, w, h))));
					if self.snap.len() > 4 {
						let (gone, _) = self.snap.pop().unwrap();
						self.snap_requested.remove(&gone);
					}
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
					self.base_inflight.clear();
					self.sharp = None;
					self.side_sharp.clear();
					self.sharp_key = None;
					self.thumbs.clear();
					self.thumbs_requested.clear();
					self.crops.clear();
					self.crops_requested.clear();
				}
				Res::Compare { page, image, changed, grid } => {
					if let Some(c) = &mut self.compare {
						if c.page == page {
							c.tex = Some(ctx.load_texture("compare", image, TextureOptions::LINEAR));
							c.changed = changed;
							c.flagged = takeoff_core::slip::items_on_changes(&self.doc.lock().data, page, &grid);
						}
					}
				}
				Res::Error(e) => self.error = Some(e),
			}
		}
		changed |= self.poll_map(ctx);
		changed
	}

	/// The line followed in this page's picture from `a` to `b`: Some(result)
	/// once the worker has traced it, None while it works (asked for here).
	pub fn trace_raster(&mut self, a: Point, b: Point) -> Option<Option<Vec<Point>>> {
		let key = trace_key(self.page, a, b);
		if let Some(r) = self.traced.get(&key) {
			return Some(r.clone());
		}
		if self.traced_requested.insert(key) {
			self.worker.send(Req::Trace { page: self.page, a, b });
		}
		None
	}

	/// Ask for the space around `at` on this page to be filled; the token to
	/// collect it with [`OpenDoc::take_fill`]. Openings tried are real door
	/// widths when the page has a scale.
	pub fn request_fill(&mut self, at: Vec<Point>) -> u64 {
		let (w, h) = self.page_size(self.page);
		let pt_per_m = {
			let d = self.doc.lock();
			at.first().and_then(|p| d.data.scale_at(self.page, *p)).and_then(|s| {
				let upp = takeoff_core::geometry::scale_units_per_px(s, w, h)?;
				takeoff_core::units::length_factor("m", &s.unit).map(|f| f / upp)
			})
		};
		self.fill_seq += 1;
		self.worker.send(Req::Fill { token: self.fill_seq, page: self.page, at, gaps: takeoff_pdf::fill::gaps(pt_per_m) });
		self.fill_seq
	}

	/// The fill asked for with `token`, once the worker is done.
	pub fn take_fill(&mut self, token: u64) -> Option<Option<Result<takeoff_pdf::fill::Fill, takeoff_pdf::fill::NoFill>>> {
		self.filled.remove(&token)
	}

	/// A render of `region` of `page` for a graph card, cached under `key`.
	pub fn crop(&mut self, key: &str, page: u32, region: Region, max_px: u32) -> Option<TextureHandle> {
		if let Some(t) = self.crops.get(key) {
			return Some(t.clone());
		}
		if self.crops_requested.insert(key.to_string()) {
			self.worker.send(Req::Crop { key: key.to_string(), page, region: FracRect { x: region.x, y: region.y, w: region.w, h: region.h }, max_px });
		}
		None
	}

	// ── Document map ────────────────────────────────────────────────────

	pub fn delete_map(&mut self) {
		let _ = takeoff_core::docmap::delete(&self.path);
		self.map = None;
		self.map_stamp = None;
		self.map_status = None;
		self.graph = Default::default();
		self.nav = MapNav::default();
		self.map_filter = MapFilter::default();
	}

	/// The node the Map tab shows: the one moved to, while its sheet is the
	/// one on screen (or the graph shows); else the sheet on screen.
	pub fn map_focus(&self) -> Option<String> {
		let map = self.map.as_ref()?;
		let graph = self.view_mode == crate::docstate::ViewMode::Graph;
		self.nav.focus.clone().filter(|f| map.node(f).is_some() && (graph || docmap::page_of(f) == Some(self.page))).or_else(|| {
			let id = docmap::sheet_id(self.page);
			map.node(&id).map(|_| id)
		})
	}

	/// Show a node in the Map tab without moving (a click on the sheet).
	pub fn map_peek(&mut self, id: &str) {
		self.nav.focus = Some(id.to_string());
		self.graph.selected = Some(id.to_string());
	}

	fn view_now(&self) -> (u32, f32, Vec2) {
		(self.page, self.zoom, self.pan)
	}

	/// Go to a node: onto the trail, and there (the sheet zoomed to it, or
	/// the graph centred on it).
	pub fn map_go(&mut self, id: &str) {
		let Some(map) = &self.map else { return };
		if map.node(id).is_none() {
			return;
		}
		// Remember where we are, so Back comes here.
		let here = self.map_focus().unwrap_or_else(|| docmap::sheet_id(self.page));
		let view = Some(self.view_now());
		match self.nav.trail.get_mut(self.nav.pos) {
			Some(s) if s.node == here => s.view = view,
			_ => {
				self.nav.trail.truncate(self.nav.pos + usize::from(!self.nav.trail.is_empty()));
				self.nav.trail.push(Stop { node: here, view });
				self.nav.pos = self.nav.trail.len() - 1;
			}
		}
		if self.nav.trail[self.nav.pos].node == id {
			self.show_node(id, None);
			return;
		}
		self.nav.trail.truncate(self.nav.pos + 1);
		self.nav.trail.push(Stop { node: id.to_string(), view: None });
		if self.nav.trail.len() > 60 {
			self.nav.trail.remove(0);
		}
		self.nav.pos = self.nav.trail.len() - 1;
		self.show_node(id, None);
	}

	pub fn can_go_back(&self) -> bool {
		self.nav.pos > 0 && !self.nav.trail.is_empty()
	}

	pub fn can_go_forward(&self) -> bool {
		self.nav.pos + 1 < self.nav.trail.len()
	}

	/// Step along the trail (-1 back, +1 forward, or to index `to`).
	pub fn map_step(&mut self, to: usize) {
		if to >= self.nav.trail.len() || to == self.nav.pos {
			return;
		}
		let view = self.view_now();
		if let Some(s) = self.nav.trail.get_mut(self.nav.pos) {
			s.view = Some(view);
		}
		self.nav.pos = to;
		let stop = self.nav.trail[to].clone();
		self.show_node(&stop.node, stop.view);
	}

	pub fn map_back(&mut self) {
		if self.can_go_back() {
			self.map_step(self.nav.pos - 1);
		}
	}

	pub fn map_forward(&mut self) {
		if self.can_go_forward() {
			self.map_step(self.nav.pos + 1);
		}
	}

	/// Put a node on screen: restore a remembered view, or frame it.
	fn show_node(&mut self, id: &str, view: Option<(u32, f32, Vec2)>) {
		self.nav.focus = Some(id.to_string());
		self.graph.select_node(id);
		if self.view_mode == ViewMode::Graph {
			return;
		}
		let Some(node) = self.map.as_ref().and_then(|m| m.node(id)).map(|n| (n.page(), n.region())) else { return };
		match (view, node) {
			(Some((page, zoom, pan)), _) => {
				self.go_to_page(page);
				self.zoom = zoom;
				self.pan = pan;
				self.fit_pending = false;
				self.touch_view();
			}
			(None, (page, Some(r))) => {
				// A little room around it, so you see where it sits; tiny callouts
				// get more, or the view would be all bubble.
				let pad = (r.w.max(r.h) * 0.12).max(0.03);
				self.zoom_to(page, (r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad));
			}
			(None, (page, None)) => {
				self.go_to_page(page);
				self.fit_pending = true;
			}
		}
		self.nav.flash = Some((id.to_string(), Instant::now()));
	}

	/// The map's sheets that pass the discipline filter, in page order (None = no map).
	pub fn filtered_pages(&self) -> Option<Vec<u32>> {
		let map = self.map.as_ref()?;
		if self.map_filter.disciplines.is_empty() {
			return None;
		}
		Some(map.sheets.iter().filter(|s| self.map_filter.sheet(s.discipline.as_deref())).map(|s| s.page).collect())
	}

	/// "A-101 · Floor plan" from the map, if the page is in it.
	pub fn sheet_name(&self, page: u32) -> Option<String> {
		let s = self.map.as_ref()?.sheet(page)?;
		s.number.as_ref()?;
		Some(s.name())
	}

	/// The map area on this page under a page point: the smallest, callouts first.
	pub fn map_area_at(&self, pt: Point) -> Option<String> {
		let s = self.map.as_ref()?.sheet(self.page)?;
		let hits = s.parts.iter().filter(|p| p.kind != PartKind::TitleBlock && p.region.is_some_and(|r| r.contains(pt.x, pt.y)));
		hits.min_by(|a, b| (a.kind != PartKind::Callout, a.region.unwrap().area()).partial_cmp(&(b.kind != PartKind::Callout, b.region.unwrap().area())).unwrap_or(std::cmp::Ordering::Equal)).map(|p| p.id.clone())
	}

	/// Where a node's links lead (targets of links from it), for a click on it.
	pub fn map_targets(&self, id: &str) -> Vec<String> {
		let Some(map) = &self.map else { return vec![] };
		map.links.iter().filter(|l| l.from == id).map(|l| l.to.clone()).collect()
	}

	/// A node's short name for the breadcrumb ("A-101", "Detail 3").
	pub fn crumb(&self, id: &str) -> String {
		let Some(n) = self.map.as_ref().and_then(|m| m.node(id)) else { return id.to_string() };
		match n {
			NodeRef::Sheet(s) => s.number.clone().unwrap_or_else(|| format!("p{}", s.page)),
			NodeRef::Part(_, p) => match (&p.label, p.title.trim()) {
				(Some(l), _) if p.kind == PartKind::Callout => l.clone(),
				(Some(l), _) => format!("{} {l}", p.kind.label()),
				(None, "") => p.kind.label().to_string(),
				(None, t) => t.chars().take(24).collect(),
			},
		}
	}

	/// Read the map again when its file changed (the AI saves after every step).
	fn poll_map(&mut self, ctx: &egui::Context) -> bool {
		// While the graph shows, look for the AI's writes even when nothing else repaints.
		if self.view_mode == ViewMode::Graph {
			ctx.request_repaint_after(Duration::from_millis(800));
		}
		if self.map_checked.elapsed() < Duration::from_millis(700) {
			return false;
		}
		self.map_checked = Instant::now();
		let stamp = map_stamp(&self.path);
		if stamp == self.map_stamp {
			return false;
		}
		self.map_stamp = stamp;
		self.map = takeoff_core::docmap::load(&self.path);
		true
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
		match self.snap.iter().find(|(p, _)| *p == self.page) {
			Some((_, idx)) => Some(idx.clone()),
			None => {
				if self.snap_requested.insert(self.page) {
					self.worker.send(Req::Segments { page: self.page });
				}
				None
			}
		}
	}

	// ── View geometry ───────────────────────────────────────────────────

	/// Switch the page layout, keeping the current page where it is on screen.
	pub fn set_layout(&mut self, layout: PageLayout) {
		if layout == self.layout {
			return;
		}
		let before = self.page_rect().min;
		self.layout = layout;
		self.arr = Arrangement::new(&self.page_sizes, layout);
		self.pan += before - self.page_rect().min;
		self.side_sharp.clear();
		self.sharp_key = None;
		self.followed = None;
		self.touch_view();
	}

	/// Where a page's corner is from the drawn frame's corner, in points.
	fn offset(&self, page: u32) -> Vec2 {
		self.arr.rect(page).min.to_vec2() - self.arr.base(self.page)
	}

	/// Where a page is on screen (meaningful for the pages the layout shows).
	pub fn page_rect_of(&self, page: u32) -> Rect {
		let r = self.arr.rect(page);
		Rect::from_min_size(self.canvas_rect.min + self.pan + self.offset(page) * self.zoom, r.size() * self.zoom)
	}

	pub fn page_rect(&self) -> Rect {
		self.page_rect_of(self.page)
	}

	pub fn to_screen(&self, p: Point) -> Pos2 {
		let r = self.page_rect();
		Pos2::new(r.min.x + p.x as f32 * r.width(), r.min.y + p.y as f32 * r.height())
	}

	pub fn to_page(&self, s: Pos2) -> Point {
		let r = self.page_rect();
		Point { x: ((s.x - r.min.x) / r.width()) as f64, y: ((s.y - r.min.y) / r.height()) as f64 }
	}

	/// A screen point in layout space.
	fn to_layout(&self, s: Pos2) -> Pos2 {
		(self.arr.base(self.page) + (s - self.canvas_rect.min - self.pan) / self.zoom).to_pos2()
	}

	/// The pages on screen, in page order.
	pub fn visible_pages(&self) -> Vec<u32> {
		self.pages_in(self.canvas_rect)
	}

	/// The pages the layout shows that meet a screen rect.
	fn pages_in(&self, screen: Rect) -> Vec<u32> {
		self.arr.visible(self.page, Rect::from_min_max(self.to_layout(screen.min), self.to_layout(screen.max)))
	}

	/// The page under a screen point (None in the gaps and around the pages).
	pub fn page_at(&self, s: Pos2) -> Option<u32> {
		self.arr.page_at(self.page, self.to_layout(s))
	}

	/// Continuous: once the view has moved, the page under its centre is the
	/// current one (no history; the selection stays).
	pub fn follow_view(&mut self) {
		let key = (self.pan, self.zoom, self.canvas_rect);
		if !self.layout.continuous || !self.loaded || self.followed == Some(key) {
			return;
		}
		self.followed = Some(key);
		self.page = self.arr.page_for_centre(self.page, self.to_layout(self.canvas_rect.center()));
	}

	/// Pan so the point `local` points from `page`'s corner is at `at` (canvas-relative).
	fn place(&mut self, page: u32, local: Vec2, at: Vec2) {
		self.pan = at - (self.offset(page) + local) * self.zoom;
		self.touch_view();
	}

	/// Screen points per page fraction, x and y.
	pub fn frac_scale(&self) -> (f32, f32) {
		let r = self.page_rect();
		(r.width(), r.height())
	}

	pub fn fit_page(&mut self) {
		let r = self.arr.row_rect(self.page);
		let avail = self.canvas_rect.size() - Vec2::splat(48.0);
		if avail.x <= 0.0 || avail.y <= 0.0 {
			return;
		}
		self.zoom = (avail.x / r.width()).min(avail.y / r.height()).clamp(0.02, 64.0);
		self.center_page();
	}

	pub fn fit_width(&mut self) {
		let r = self.arr.row_rect(self.page);
		self.zoom = ((self.canvas_rect.width() - 48.0) / r.width()).clamp(0.02, 64.0);
		if self.layout.continuous {
			self.scroll_to(self.page);
			return;
		}
		self.center_page();
		self.pan.y = 24.0;
	}

	/// Centre the current page (its spread, with spreads).
	pub fn center_page(&mut self) {
		let r = self.arr.row_rect(self.page);
		let corner = r.min - self.arr.rect(self.page).min;
		self.place(self.page, corner, (self.canvas_rect.size() - r.size() * self.zoom) / 2.0);
	}

	/// Continuous: bring a page into view at this zoom, its row centred when
	/// it fits, else its top near the top, so the view's centre is on it.
	fn scroll_to(&mut self, page: u32) {
		let (row, p) = (self.arr.row_rect(page), self.arr.rect(page));
		let (c, s) = (self.canvas_rect.size(), row.size() * self.zoom);
		let y = if s.y <= c.y - 48.0 { (c.y - s.y) / 2.0 } else { 24.0 };
		let (x, at_x) = if s.x <= c.x - 48.0 {
			(row.min.x, (c.x - s.x) / 2.0)
		} else if p.width() * self.zoom <= c.x - 48.0 {
			(p.min.x, (c.x - p.width() * self.zoom) / 2.0)
		} else {
			(p.min.x, 24.0)
		};
		self.place(page, Pos2::new(x, row.min.y) - p.min, Vec2::new(at_x, y));
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
		let center = Vec2::new(((region.0 + region.2 / 2.0) * w) as f32, ((region.1 + region.3 / 2.0) * h) as f32);
		self.place(self.page, center, avail / 2.0);
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
			let (was, row) = (self.arr.row_rect(self.page), self.arr.row_rect(page));
			let same_row = self.arr.row(page) == self.arr.row(self.page);
			self.page = page;
			self.selection.clear();
			if self.layout.continuous {
				// Every page is laid out already: scroll there, keeping the zoom.
				if !same_row {
					self.scroll_to(page);
				}
				self.touch_view();
			} else if was.size() == row.size() {
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
		let ppp = ctx.pixels_per_point();
		if self.layout.multi() {
			self.request_bases(base_px, ppp);
		} else if self.base_requested.get(&page).is_none_or(|px| *px < base_px) {
			self.base_requested.insert(page, base_px);
			self.worker.send(Req::Base { page, max_px: base_px });
			// Neighbours too, so paging feels instant.
			for p in [page + 1, page.saturating_sub(1)] {
				if p >= 1 && p <= self.page_count() && self.base_requested.get(&p).is_none_or(|px| *px < base_px) {
					self.base_requested.insert(p, base_px);
					self.worker.send(Req::Base { page: p, max_px: base_px });
				}
			}
		}
		// Sharp renders of what's visible, once the view has settled.
		let mut want = Vec::new();
		for pg in self.visible_pages() {
			let pr = self.page_rect_of(pg);
			let vis = pr.intersect(self.canvas_rect);
			if vis.width() <= 1.0 || vis.height() <= 1.0 {
				continue;
			}
			// Skip when the base texture is already sharp enough.
			if pr.width().max(pr.height()) * ppp <= base_px as f32 * 1.05 {
				if pg == page {
					self.sharp = None;
				}
				self.side_sharp.remove(&pg);
				continue;
			}
			// Render a little beyond the visible area so small pans stay sharp.
			let pad = Vec2::new(vis.width(), vis.height()) * 0.15;
			let area = vis.expand2(pad).intersect(pr);
			let region = FracRect {
				x: ((area.min.x - pr.min.x) / pr.width()) as f64,
				y: ((area.min.y - pr.min.y) / pr.height()) as f64,
				w: (area.width() / pr.width()) as f64,
				h: (area.height() / pr.height()) as f64,
			};
			let mut px = ((area.width() * ppp) as u32, (area.height() * ppp) as u32);
			let cap = max_tex.min(6000) as f32 / px.0.max(px.1) as f32;
			if cap < 1.0 {
				px = ((px.0 as f32 * cap) as u32, (px.1 as f32 * cap) as u32);
			}
			want.push((pg, region, (px.0.max(1), px.1.max(1))));
		}
		self.side_sharp.retain(|p, _| want.iter().any(|w| w.0 == *p));
		if want.is_empty() {
			return;
		}
		if self.view_changed_at.elapsed() < Duration::from_millis(140) {
			ctx.request_repaint_after(Duration::from_millis(150));
			return;
		}
		// The current page first.
		want.sort_by_key(|w| w.0 != page);
		let key: Vec<_> = want.iter().map(|(pg, r, _)| (*pg, [r.x, r.y, r.w, r.h].map(|v| (v * 20_000.0) as i64), (self.zoom * 1000.0) as i64)).collect();
		if self.sharp_key.as_ref() == Some(&key) {
			return;
		}
		self.sharp_key = Some(key);
		self.worker.views(want);
	}

	/// Continuous / spreads: base renders for the pages on screen and a
	/// screen's height around them, nearest first and a few at a time, sized
	/// to the page on screen (a thumbnail when it is tiny). Textures far
	/// from the view are let go, so scrolling a long set doesn't fill memory.
	fn request_bases(&mut self, base_px: u32, ppp: f32) {
		let now = Instant::now();
		self.base_inflight.retain(|_, t| now.duration_since(*t) < Duration::from_secs(5));
		let mut near = self.pages_in(self.canvas_rect.expand2(Vec2::new(0.0, self.canvas_rect.height())));
		let c = self.canvas_rect.center();
		let dist = |d: &Self, p: u32| (d.page_rect_of(p).center() - c).length();
		near.sort_by(|a, b| dist(self, *a).total_cmp(&dist(self, *b)));
		for &pg in &near {
			if self.base_inflight.len() >= 3 {
				break;
			}
			let r = self.page_rect_of(pg);
			let edge = r.width().max(r.height()) * ppp;
			if edge <= 240.0 && !self.base.contains_key(&pg) {
				self.thumb(pg);
				continue;
			}
			let px = [600, 1200].into_iter().find(|t| *t as f32 >= edge).unwrap_or(base_px).min(base_px);
			if self.base_requested.get(&pg).is_some_and(|had| *had >= px) {
				continue;
			}
			self.base_requested.insert(pg, px);
			self.base_inflight.insert(pg, now);
			self.worker.send(Req::Base { page: pg, max_px: px });
		}
		if self.base.len() > 32 {
			let keep: HashSet<u32> = near.iter().copied().chain([self.page]).collect();
			self.base.retain(|p, _| keep.contains(p));
			let inflight = &self.base_inflight;
			self.base_requested.retain(|p, _| keep.contains(p) || inflight.contains_key(p));
		}
	}

	/// Start (or retarget) a revision compare against another PDF page.
	pub fn start_compare(&mut self, other: PathBuf, other_page: u32, ctx: &egui::Context) {
		let max_px = 3000.min(ctx.input(|i| i.max_texture_side) as u32);
		self.worker.send(Req::Compare { page: self.page, other: other.clone(), other_page, max_px });
		self.compare = Some(CompareState { other, other_page, page: self.page, tex: None, changed: 0.0, show_overlay: true, revision: false, flagged: vec![] });
	}

	/// Compare this page with the revision it replaced; false when it has none (or its file is gone).
	pub fn compare_previous(&mut self, ctx: &egui::Context) -> bool {
		let Some(r) = self.doc.lock().data.previous_revision(self.page).cloned() else { return false };
		let file = takeoff_pdf::slip::history_path(&self.path, &r);
		if !file.exists() {
			return false;
		}
		self.start_compare(file, r.file_page, ctx);
		if let Some(c) = &mut self.compare {
			c.revision = true;
		}
		true
	}

	/// Keep the compare in step with page changes (same page number in both
	/// files; a revision compare moves to that page's previous revision).
	pub fn update_compare(&mut self, ctx: &egui::Context) {
		let needs = self.compare.as_ref().is_some_and(|c| c.page != self.page);
		if needs {
			let c = self.compare.take().unwrap();
			if c.revision {
				self.compare_previous(ctx);
				return;
			}
			let delta = self.page as i64 - c.page as i64;
			let other_page = (c.other_page as i64 + delta).max(1) as u32;
			self.start_compare(c.other, other_page, ctx);
		}
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

fn map_stamp(pdf: &std::path::Path) -> Option<std::time::SystemTime> {
	std::fs::metadata(takeoff_core::docmap::map_path(pdf)).and_then(|m| m.modified()).ok()
}

fn trace_key(page: u32, a: Point, b: Point) -> (u32, [u64; 4]) {
	(page, [a.x, a.y, b.x, b.y].map(f64::to_bits))
}
