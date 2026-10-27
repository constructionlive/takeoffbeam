//! Graph view: the document map as a graph. Sheets are cards with their
//! thumbnail, the areas the AI recorded on them (details, sections,
//! schedules, notes, callouts…) sit around them as cards cut from the sheet,
//! and arrows follow how they relate; sheets gather by discipline. Click a
//! card to show it in the Map tab; double-click to open it on the sheet.
//! Focus shows only what is linked to the selection (a local graph). The map
//! is built by the AI in a chat (Graph view starts it); the graph fills in
//! as it records.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use takeoff_core::docmap::{self, DocMap, Link, LinkKind, NodeRef, Part, PartKind, Region};

use crate::ai_panel::MapChat;
use crate::docstate::{discipline_key, discipline_parts, MapFilter, OpenDoc};
use crate::icons as i;
use crate::theme::{self, Palette};
use crate::widgets;

mod arrange;
mod draw;
mod menus;
pub use draw::*;
pub use menus::*;

const SHEET_SIZE: Vec2 = Vec2::new(176.0, 132.0);
const PART_SIZE: Vec2 = Vec2::new(128.0, 96.0);
const CALLOUT_SIZE: Vec2 = Vec2::new(84.0, 64.0);
/// Areas shown under each sheet on the board.
const BOARD_AREAS: usize = 8;
const CELL: f32 = 380.0;
/// Below this on-screen width a card is a coloured chip, not a picture.
const CHIP_W: f32 = 34.0;
pub const CROP_PX: u32 = 360;

pub enum GraphAction {
	/// Map the set in a new chat (fresh: throw the old map away first).
	StartChat { instructions: String, fresh: bool },
	/// Stop the AI.
	Stop,
	/// Show the AI tab.
	OpenChat,
	Delete,
	/// A card was clicked: show it in the Map tab (and on the trail).
	Focus(String),
	/// A card was double-clicked: open it on the sheet.
	Open(String),
}

struct GNode {
	id: String,
	sheet: bool,
	pos: Vec2,
	vel: Vec2,
	pinned: bool,
	size: Vec2,
	color: Color32,
	/// Where its discipline gathers (sheets only).
	anchor: Vec2,
}

struct GEdge {
	a: usize,
	b: usize,
	/// None: an area on its sheet (or inside another area).
	kind: Option<LinkKind>,
	count: usize,
}

/// The continue / start-over dialog.
struct BuildDialog {
	fresh: bool,
}

pub struct GraphState {
	nodes: Vec<GNode>,
	edges: Vec<GEdge>,
	index: HashMap<String, usize>,
	/// What the layout was made from (map, filter, focus).
	synced: Option<u64>,
	pub selected: Option<String>,
	hovered: Option<usize>,
	/// World point at the middle of the view; screen points per world unit.
	center: Vec2,
	zoom: f32,
	/// Keep the whole map in view while the layout settles (until the person moves it).
	auto_fit: bool,
	/// Layout temperature: 0 = still.
	alpha: f32,
	drag_node: Option<usize>,
	pub search: String,
	/// Focus: only what is within this many links of the selection (0 = all).
	pub local: u8,
	/// Discipline cluster labels: (name, world position).
	clusters: Vec<(String, Vec2)>,
	/// Arranged (lanes of sheets with their areas beneath) rather than a web.
	pub board: bool,
	/// Board: a lane per discipline (name, colour, sheets, world rect) and a column per sheet.
	lanes: Vec<(String, Color32, usize, Rect)>,
	/// Each sheet's column, and how many of its areas don't fit on the board.
	columns: Vec<(Rect, usize)>,
	more: HashMap<u32, usize>,
	/// The graph area's shape last frame (the board is arranged to suit it).
	aspect: f32,
	trackpad: bool,
	dialog: Option<BuildDialog>,
	/// What the estimator wants the AI to know when mapping.
	instructions: String,
	/// Center on this node next frame.
	focus: Option<String>,
	/// Where the graph was drawn last frame.
	view: Rect,
}

impl Default for GraphState {
	fn default() -> Self {
		Self {
			nodes: vec![],
			edges: vec![],
			index: HashMap::new(),
			synced: None,
			selected: None,
			hovered: None,
			center: Vec2::ZERO,
			zoom: 1.0,
			auto_fit: true,
			alpha: 0.0,
			drag_node: None,
			search: String::new(),
			local: 0,
			clusters: vec![],
			board: true,
			lanes: vec![],
			columns: vec![],
			more: HashMap::new(),
			aspect: 1.6,
			trackpad: false,
			dialog: None,
			instructions: String::new(),
			focus: None,
			view: Rect::NOTHING,
		}
	}
}

pub fn link_color(k: LinkKind) -> Color32 {
	match k {
		LinkKind::Detail => Color32::from_rgb(0x4C, 0xC2, 0xFF),
		LinkKind::Section => Color32::from_rgb(0xA7, 0x8B, 0xFA),
		LinkKind::Elevation => Color32::from_rgb(0x34, 0xD3, 0x99),
		LinkKind::Enlarged => Color32::from_rgb(0xFB, 0xBF, 0x24),
		LinkKind::Continues => Color32::from_rgb(0xF4, 0x72, 0xB6),
		LinkKind::Schedule => Color32::from_rgb(0x22, 0xD3, 0xEE),
		LinkKind::Governs => Color32::from_rgb(0xFB, 0x92, 0x3C),
		LinkKind::Same => Color32::from_rgb(0xA3, 0xE6, 0x35),
		LinkKind::Reference => Color32::from_rgb(0x94, 0xA3, 0xB8),
	}
}

pub fn discipline_color(d: Option<&str>) -> Color32 {
	let d = d.unwrap_or("").to_lowercase();
	match () {
		_ if d.starts_with("arch") => Color32::from_rgb(0x4C, 0xC2, 0xFF),
		_ if d.starts_with("struct") => Color32::from_rgb(0xF8, 0x71, 0x71),
		_ if d.starts_with("mech") => Color32::from_rgb(0x34, 0xD3, 0x99),
		_ if d.starts_with("elec") => Color32::from_rgb(0xFB, 0xBF, 0x24),
		_ if d.starts_with("plumb") => Color32::from_rgb(0x22, 0xD3, 0xEE),
		_ if d.starts_with("fire") => Color32::from_rgb(0xF4, 0x72, 0xB6),
		_ if d.starts_with("civil") || d.starts_with("land") => Color32::from_rgb(0xA3, 0xE6, 0x35),
		_ => Color32::from_rgb(0x94, 0xA3, 0xB8),
	}
}

pub fn kind_color(k: PartKind) -> Color32 {
	match k {
		PartKind::Plan => Color32::from_rgb(0x4C, 0xC2, 0xFF),
		PartKind::Detail => Color32::from_rgb(0x4C, 0xC2, 0xFF),
		PartKind::Section => Color32::from_rgb(0xA7, 0x8B, 0xFA),
		PartKind::Elevation => Color32::from_rgb(0x34, 0xD3, 0x99),
		PartKind::Schedule => Color32::from_rgb(0x22, 0xD3, 0xEE),
		PartKind::Legend => Color32::from_rgb(0xFB, 0xBF, 0x24),
		PartKind::KeyPlan => Color32::from_rgb(0xF4, 0x72, 0xB6),
		PartKind::Notes => Color32::from_rgb(0xFB, 0x92, 0x3C),
		PartKind::Element => Color32::from_rgb(0xA3, 0xE6, 0x35),
		_ => Color32::from_rgb(0x94, 0xA3, 0xB8),
	}
}

fn signature(m: &DocMap) -> u64 {
	let mut h = std::collections::hash_map::DefaultHasher::new();
	m.page_count.hash(&mut h);
	for s in &m.sheets {
		(s.page, s.parts.len(), s.mapped, &s.number, &s.discipline).hash(&mut h);
		for p in &s.parts {
			(&p.id, p.kind, &p.parent, p.region.map(|r| [r.x, r.y, r.w, r.h].map(f64::to_bits))).hash(&mut h);
		}
	}
	for l in &m.links {
		(&l.from, &l.to, l.kind).hash(&mut h);
	}
	h.finish()
}

/// The one-line description of a link under its target's name.
pub fn link_sub(l: &Link) -> String {
	let mut s = l.kind.label().to_string();
	if !l.text.is_empty() {
		s.push_str(&format!(" · \"{}\"", l.text));
	}
	if !l.note.is_empty() {
		s.push_str(&format!(" · {}", l.note));
	}
	s
}

/// The key a part's picture is cached under (changes when its box does).
pub fn crop_key(id: &str, r: Region) -> String {
	format!("{id}@{:.4},{:.4},{:.4},{:.4}", r.x, r.y, r.w, r.h)
}

impl GraphState {
	/// Make the nodes and edges match the map, the filter and the focus,
	/// keeping where existing nodes are.
	fn sync(&mut self, map: &DocMap, filter: &MapFilter) {
		let root = if self.local > 0 { self.selected.clone().filter(|s| map.node(s).is_some()) } else { None };
		let key = {
			let mut h = std::collections::hash_map::DefaultHasher::new();
			signature(map).hash(&mut h);
			filter.disciplines.hash(&mut h);
			let mut kinds: Vec<&str> = filter.kinds.iter().map(|k| k.key()).collect();
			kinds.sort_unstable();
			kinds.hash(&mut h);
			(&root, self.local, self.board).hash(&mut h);
			h.finish()
		};
		if self.synced == Some(key) {
			return;
		}
		let first = self.synced.is_none();
		self.synced = Some(key);
		// What passes the filter.
		let mut keep: HashSet<String> = HashSet::new();
		for s in map.sheets.iter().filter(|s| filter.sheet(s.discipline.as_deref())) {
			keep.insert(s.id());
			keep.extend(s.parts.iter().filter(|p| filter.kinds.contains(&p.kind)).map(|p| p.id.clone()));
		}
		// Focus: what is within `local` steps of the root (areas reach their sheet and container).
		if let Some(root) = &root {
			let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
			for s in &map.sheets {
				for p in &s.parts {
					let host = p.parent.as_deref().unwrap_or("");
					adj.entry(p.id.as_str()).or_default();
					if !host.is_empty() {
						adj.entry(p.id.as_str()).or_default().push(host);
						adj.entry(host).or_default().push(p.id.as_str());
					}
				}
			}
			for l in &map.links {
				adj.entry(l.from.as_str()).or_default().push(l.to.as_str());
				adj.entry(l.to.as_str()).or_default().push(l.from.as_str());
			}
			let sheet_of = |id: &str| docmap::page_of(id).map(docmap::sheet_id);
			let mut near: HashSet<String> = HashSet::from([root.clone()]);
			let mut queue = VecDeque::from([(root.clone(), 0u8)]);
			while let Some((id, d)) = queue.pop_front() {
				if d >= self.local {
					continue;
				}
				let mut next: Vec<String> = adj.get(id.as_str()).map(|v| v.iter().map(|x| x.to_string()).collect()).unwrap_or_default();
				// A sheet reaches what is on it; an area reaches its sheet.
				if id.contains('.') {
					next.extend(sheet_of(&id));
				} else if let Some(s) = docmap::page_of(&id).and_then(|p| map.sheet(p)) {
					next.extend(s.parts.iter().filter(|p| p.parent.is_none()).map(|p| p.id.clone()));
				}
				for n in next {
					if near.insert(n.clone()) {
						queue.push_back((n, d + 1));
					}
				}
			}
			// Areas keep their sheet in view, so you see where they are.
			let sheets: Vec<String> = near.iter().filter_map(|n| sheet_of(n)).collect();
			near.extend(sheets);
			keep.retain(|k| near.contains(k));
		}
		let old: HashMap<String, (Vec2, bool)> = self.nodes.drain(..).map(|n| (n.id, (n.pos, n.pinned))).collect();
		self.more.clear();
		self.edges.clear();
		self.index.clear();
		// Disciplines gather around a ring of anchors, biggest first.
		let sheets: Vec<&docmap::Sheet> = map.sheets.iter().filter(|s| keep.contains(&s.id())).collect();
		let mut groups: BTreeMap<String, Vec<&docmap::Sheet>> = BTreeMap::new();
		for s in &sheets {
			let key = discipline_key(s.discipline.as_deref());
			groups.entry(discipline_parts(&key).0.to_string()).or_default().push(s);
		}
		let mut order: Vec<(String, Vec<&docmap::Sheet>)> = groups.into_iter().collect();
		order.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
		let spacing = 460.0;
		let ring = if order.len() <= 1 { 0.0 } else { spacing * (sheets.len() as f32).sqrt().max(1.2) * 0.6 };
		self.clusters.clear();
		let mut added = 0usize;
		for (gi, (name, group)) in order.iter().enumerate() {
			let a = gi as f32 / order.len() as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
			let anchor = Vec2::new(a.cos(), a.sin()) * ring;
			let cols = (group.len() as f32).sqrt().ceil().max(1.0) as usize;
			self.clusters.push((name.clone(), anchor));
			for (k, s) in group.iter().enumerate() {
				let id = s.id();
				let grid = anchor + Vec2::new((k % cols) as f32, (k / cols) as f32) * spacing - Vec2::splat((cols as f32 - 1.0) * spacing / 2.0);
				let (pos, pinned) = old.get(&id).copied().unwrap_or_else(|| {
					added += 1;
					(grid, false)
				});
				let si = self.nodes.len();
				self.index.insert(id.clone(), si);
				self.nodes.push(GNode { id, sheet: true, pos, vel: Vec2::ZERO, pinned, size: SHEET_SIZE, color: discipline_color(s.discipline.as_deref()), anchor });
				let mut parts: Vec<&Part> = s.parts.iter().filter(|p| keep.contains(&p.id)).collect();
				// The board shows a sheet's main areas (drawings first); the rest are in the Map tab.
				if self.board && parts.len() > BOARD_AREAS {
					parts.sort_by_key(|p| PartKind::ALL.iter().position(|k| *k == p.kind).unwrap_or(99));
					self.more.insert(s.page, parts.len() - BOARD_AREAS);
					parts.truncate(BOARD_AREAS);
				}
				for (j, p) in parts.iter().enumerate() {
					// Hung from the area it sits in when that has a card, else from the sheet.
					let host = p.parent.as_ref().and_then(|q| self.index.get(q).copied()).unwrap_or(si);
					let a = j as f32 / parts.len() as f32 * std::f32::consts::TAU;
					let r = if host == si { 210.0 } else { 120.0 };
					let base = self.nodes[host].pos;
					let (ppos, ppinned) = old.get(&p.id).copied().unwrap_or_else(|| {
						added += 1;
						(base + Vec2::new(a.cos(), a.sin()) * r, false)
					});
					let pi = self.nodes.len();
					self.index.insert(p.id.clone(), pi);
					let size = if p.kind == PartKind::Callout { CALLOUT_SIZE } else { PART_SIZE };
					self.nodes.push(GNode { id: p.id.clone(), sheet: false, pos: ppos, vel: Vec2::ZERO, pinned: ppinned, size, color: kind_color(p.kind), anchor });
					self.edges.push(GEdge { a: host, b: pi, kind: None, count: 1 });
				}
			}
		}
		// Links between what is shown: an area without a card stands for the
		// area it sits in, else its sheet.
		let parents: HashMap<&str, &str> = map.sheets.iter().flat_map(|s| s.parts.iter()).filter_map(|p| Some((p.id.as_str(), p.parent.as_deref()?))).collect();
		let rep = |id: &str| {
			let mut cur = id;
			for _ in 0..8 {
				if let Some(&i) = self.index.get(cur) {
					return Some(i);
				}
				match parents.get(cur) {
					Some(q) => cur = q,
					None => break,
				}
			}
			docmap::page_of(id).and_then(|p| self.index.get(&docmap::sheet_id(p)).copied())
		};
		let mut seen: HashMap<(usize, usize, LinkKind), usize> = HashMap::new();
		let mut links = Vec::new();
		for l in &map.links {
			let (Some(a), Some(b)) = (rep(&l.from), rep(&l.to)) else { continue };
			if a != b {
				links.push((a, b, l.kind));
			}
		}
		for (a, b, kind) in links {
			match seen.get(&(a, b, kind)) {
				Some(&e) => self.edges[e].count += 1,
				None => {
					seen.insert((a, b, kind), self.edges.len());
					self.edges.push(GEdge { a, b, kind: Some(kind), count: 1 });
				}
			}
		}
		if self.board {
			self.arrange(map);
			self.alpha = 0.0;
			self.auto_fit = true;
		} else {
			self.lanes.clear();
			self.columns.clear();
			if first || added > 0 || root.is_some() {
				self.alpha = if first || added * 2 > self.nodes.len() { 1.0 } else { 0.5 };
				self.auto_fit = true;
			}
		}
	}

	fn to_screen(&self, view: Rect, w: Vec2) -> Pos2 {
		view.center() + (w - self.center) * self.zoom
	}

	fn node_rect(&self, view: Rect, n: &GNode) -> Rect {
		Rect::from_center_size(self.to_screen(view, n.pos), n.size * self.zoom)
	}

	fn node_at(&self, view: Rect, pos: Pos2) -> Option<usize> {
		// Topmost first: sheets are drawn over parts.
		let order = self.draw_order();
		order.into_iter().rev().find(|&i| self.node_rect(view, &self.nodes[i]).expand(2.0).contains(pos))
	}

	fn draw_order(&self) -> Vec<usize> {
		let mut v: Vec<usize> = (0..self.nodes.len()).collect();
		v.sort_by_key(|&i| self.nodes[i].sheet);
		v
	}

	fn neighbours(&self, i: usize) -> HashSet<usize> {
		let mut s = HashSet::from([i]);
		for e in &self.edges {
			if e.a == i {
				s.insert(e.b);
			} else if e.b == i {
				s.insert(e.a);
			}
		}
		s
	}

	fn select(&mut self, id: &str) {
		self.selected = Some(id.to_string());
		self.focus = Some(id.to_string());
	}

	/// Select a node and bring it into view if it is off screen.
	pub fn select_node(&mut self, id: &str) {
		self.select(id);
	}

	/// Select a page's sheet (picked in the Pages panel while the graph shows).
	pub fn select_page(&mut self, page: u32) {
		let id = docmap::sheet_id(page);
		if self.index.contains_key(&id) {
			self.select(&id);
		}
	}

	pub fn deselect(&mut self) {
		self.selected = None;
	}

	/// Where node `id` is on screen (tests).
	#[doc(hidden)]
	pub fn debug_node_center(&self, id: &str) -> Option<Pos2> {
		let n = self.index.get(id)?;
		Some(self.to_screen(self.view, self.nodes[*n].pos))
	}

	/// Whether the layout has settled (tests).
	#[doc(hidden)]
	pub fn debug_settled(&self) -> bool {
		self.alpha <= 0.0 && !self.nodes.is_empty()
	}
}

// ── The view ──────────────────────────────────────────────────────────────

pub fn ui(ui: &mut Ui, doc: &mut OpenDoc, p: &Palette, chat: &MapChat, wheel_zooms: bool) -> Option<GraphAction> {
	let mut st = std::mem::take(&mut doc.graph);
	let mut action = None;
	let rect = ui.max_rect();
	ui.painter().rect_filled(rect, 0.0, p.canvas);
	match doc.map.clone().filter(|m| !m.sheets.is_empty()) {
		None if chat.running_here => waiting_state(ui, chat, p, rect, &mut action),
		None => {
			let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(16.0)).layout(egui::Layout::top_down(egui::Align::Center)));
			child.add_space((rect.height() * 0.08).max(12.0));
			build_card(&mut child, doc, &mut st, p, chat, false, &mut action);
		}
		Some(map) => {
			let filter = doc.map_filter.clone();
			st.sync(&map, &filter);
			let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 40.0));
			let graph = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom()), rect.max);
			graph_canvas(ui, doc, &mut st, &map, p, graph, wheel_zooms, &mut action);
			// On the board links show only around what you hover or pick; so does their key.
			if !st.board || st.hovered.is_some() || st.selected.is_some() {
				legend(ui, p, graph, st.board);
			}
			top_bar(ui, doc, &mut st, &map, p, chat, bar, &mut action);
		}
	}
	if st.dialog.is_some() {
		build_dialog(ui.ctx(), doc, &mut st, p, chat, &mut action);
	}
	doc.graph = st;
	action
}

/// A discipline group ("Civil") and the disciplines filed under it
/// ("Civil/Track", "Civil/Yard"; "Civil" itself when sheets say only that).
pub struct DisciplineGroup {
	pub name: String,
	pub color: Color32,
	pub count: usize,
	/// (key, label, sheets), in set order.
	pub subs: Vec<(String, String, usize)>,
}

impl DisciplineGroup {
	pub fn keys(&self) -> impl Iterator<Item = &String> {
		self.subs.iter().map(|s| &s.0)
	}
}

