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

	/// The board: a lane per discipline in the order the set uses them, its
	/// sheets in page order (wrapping after a few), each sheet's areas in a
	/// tidy stack beneath it: drawings, details and sections, schedules,
	/// notes, then callouts.
	fn arrange(&mut self, map: &DocMap) {
		// As many sheets a row as make the board about the shape of the view.
		let mut best = (f32::MAX, 6usize);
		for per_row in 2..=24 {
			let (w, h) = self.arrange_with(map, per_row);
			let off = ((w / h.max(1.0)).ln() - self.aspect.max(0.3).ln()).abs();
			if off < best.0 {
				best = (off, per_row);
			}
		}
		self.arrange_with(map, best.1);
	}

	/// Lay the board out with `per_row` sheets a row; returns its size.
	fn arrange_with(&mut self, map: &DocMap, per_row: usize) -> (f32, f32) {
		const COL_GAP: f32 = 44.0;
		const ROW_GAP: f32 = 56.0;
		const LANE_GAP: f32 = 90.0;
		const LABEL_H: f32 = 96.0;
		const PAD: f32 = 12.0;
		let order = |k: PartKind| PartKind::ALL.iter().position(|x| *x == k).unwrap_or(99);
		// Sheets shown, grouped by discipline in set order.
		let mut lanes: Vec<(String, Color32, Vec<usize>)> = Vec::new();
		for s in &map.sheets {
			let Some(&si) = self.index.get(&s.id()) else { continue };
			let key = discipline_key(s.discipline.as_deref());
			let g = discipline_parts(&key).0.to_string();
			match lanes.iter_mut().find(|l| l.0 == g) {
				Some(l) => l.2.push(si),
				None => lanes.push((g.clone(), discipline_color(Some(&g)), vec![si])),
			}
		}
		// Each sheet's areas, in kind order.
		let mut areas: HashMap<u32, Vec<(usize, usize)>> = HashMap::new();
		for (i, n) in self.nodes.iter().enumerate().filter(|(_, n)| !n.sheet) {
			let Some(NodeRef::Part(s, part)) = map.node(&n.id) else { continue };
			areas.entry(s.page).or_default().push((order(part.kind), i));
		}
		for v in areas.values_mut() {
			v.sort();
		}
		self.lanes.clear();
		self.columns.clear();
		let col_w = (PART_SIZE.x * 2.0 + PAD).max(SHEET_SIZE.x);
		let mut y = 0.0;
		let mut right: f32 = 0.0;
		for (name, color, sheets) in lanes {
			let top = y;
			y += LABEL_H;
			for row in sheets.chunks(per_row) {
				let mut row_h: f32 = 0.0;
				for (c, &si) in row.iter().enumerate() {
					let x = c as f32 * (col_w + COL_GAP);
					let page = docmap::page_of(&self.nodes[si].id).unwrap_or(0);
					// The sheet fills its column's width: on a board it's what you read first.
					let sheet = Vec2::new(col_w, col_w * 0.74);
					self.nodes[si].size = sheet;
					self.nodes[si].pos = Vec2::new(x + col_w / 2.0, y + sheet.y / 2.0);
					let mut cy = y + sheet.y + PAD * 2.0;
					// Areas two across, callouts three across (they are smaller).
					let list = areas.get(&page).cloned().unwrap_or_default();
					let (big, small): (Vec<usize>, Vec<usize>) = list.iter().map(|x| x.1).partition(|&i| self.nodes[i].size != CALLOUT_SIZE);
					for (k, &i) in big.iter().enumerate() {
						let (cx, r) = ((k % 2) as f32, (k / 2) as f32);
						self.nodes[i].pos = Vec2::new(x + PART_SIZE.x / 2.0 + cx * (PART_SIZE.x + PAD), cy + PART_SIZE.y / 2.0 + r * (PART_SIZE.y + PAD));
					}
					if !big.is_empty() {
						cy += big.len().div_ceil(2) as f32 * (PART_SIZE.y + PAD);
					}
					let cw = (col_w - 2.0 * PAD) / 3.0;
					for (k, &i) in small.iter().enumerate() {
						let (cx, r) = ((k % 3) as f32, (k / 3) as f32);
						self.nodes[i].pos = Vec2::new(x + cw / 2.0 + cx * (cw + PAD), cy + CALLOUT_SIZE.y / 2.0 + r * (CALLOUT_SIZE.y + PAD));
					}
					if !small.is_empty() {
						cy += small.len().div_ceil(3) as f32 * (CALLOUT_SIZE.y + PAD);
					}
					let more = self.more.get(&page).copied().unwrap_or(0);
					if more > 0 {
						cy += 26.0;
					}
					let col = Rect::from_min_max(egui::pos2(x - PAD, y - PAD), egui::pos2(x + col_w + PAD, cy));
					self.columns.push((col, more));
					row_h = row_h.max(cy - y);
					right = right.max(col.right());
				}
				y += row_h + ROW_GAP;
			}
			self.lanes.push((name, color, 0, Rect::from_min_max(egui::pos2(-PAD * 3.0, top), egui::pos2(0.0, y - ROW_GAP + PAD * 2.0))));
			y += LANE_GAP;
		}
		// Lanes span the widest row; remember each lane's sheet count.
		for l in self.lanes.iter_mut() {
			l.3.max.x = right + PAD * 2.0;
		}
		let counts: Vec<usize> = self.lanes.iter().map(|l| self.nodes.iter().filter(|n| n.sheet && l.3.contains(n.pos.to_pos2())).count()).collect();
		for (l, n) in self.lanes.iter_mut().zip(counts) {
			l.2 = n;
		}
		for n in self.nodes.iter_mut() {
			n.vel = Vec2::ZERO;
			n.pinned = false;
		}
		(right, (y - LANE_GAP).max(1.0))
	}

	/// One step of the force layout (the board stays where it was arranged).
	fn tick(&mut self) {
		if self.board {
			self.alpha = 0.0;
			return;
		}
		let n = self.nodes.len();
		if n == 0 || self.alpha <= 0.0 {
			return;
		}
		let mut force = vec![Vec2::ZERO; n];
		// Nearby nodes push apart (spatial hash: only neighbouring cells).
		let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
		let cell = |p: Vec2| ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32);
		for (i, nd) in self.nodes.iter().enumerate() {
			grid.entry(cell(nd.pos)).or_default().push(i);
		}
		for (i, nd) in self.nodes.iter().enumerate() {
			let (cx, cy) = cell(nd.pos);
			for dx in -1..=1 {
				for dy in -1..=1 {
					let Some(bucket) = grid.get(&(cx + dx, cy + dy)) else { continue };
					for &j in bucket {
						if j <= i {
							continue;
						}
						let other = &self.nodes[j];
						let mut d = nd.pos - other.pos;
						if d.length_sq() < 0.01 {
							d = Vec2::new((i as f32).sin(), (j as f32).cos());
						}
						let dist = d.length().max(1.0);
						let dir = d / dist;
						let min = (nd.size.length() + other.size.length()) * 0.5 + 24.0;
						let mut f = 9000.0 / dist;
						if dist < min {
							f += (min - dist) * 0.6;
						}
						force[i] += dir * f.min(200.0);
						force[j] -= dir * f.min(200.0);
					}
				}
			}
		}
		for e in &self.edges {
			let (rest, k) = match e.kind {
				None => (210.0, 0.08),
				Some(_) => (380.0, 0.012 * (e.count as f32).min(3.0)),
			};
			let d = self.nodes[e.b].pos - self.nodes[e.a].pos;
			let dist = d.length().max(1.0);
			let f = (dist - rest) * k;
			force[e.a] += d / dist * f;
			force[e.b] -= d / dist * f;
		}
		for (i, nd) in self.nodes.iter_mut().enumerate() {
			if nd.pinned || self.drag_node == Some(i) {
				nd.vel = Vec2::ZERO;
				continue;
			}
			// Sheets gather at their discipline's anchor; areas follow their sheet.
			let gravity = if nd.sheet { 0.01 } else { 0.001 };
			let f = force[i] - (nd.pos - nd.anchor) * gravity;
			nd.vel = (nd.vel + f * self.alpha) * 0.55;
			if nd.vel.length() > 60.0 {
				nd.vel = nd.vel.normalized() * 60.0;
			}
			nd.pos += nd.vel;
		}
		self.alpha *= 0.985;
		if self.alpha < 0.015 {
			self.alpha = 0.0;
		}
	}

	fn bounds(&self) -> Option<Rect> {
		let mut r: Option<Rect> = None;
		for n in &self.nodes {
			let nr = Rect::from_center_size(n.pos.to_pos2(), n.size);
			r = Some(r.map_or(nr, |a| a.union(nr)));
		}
		r
	}

	fn fit(&mut self, view: Rect) {
		let Some(mut b) = self.bounds() else { return };
		for l in &self.lanes {
			b = b.union(l.3);
		}
		// Room above for the discipline names.
		if self.clusters.len() > 1 {
			b.min.y -= 80.0;
		}
		self.zoom = ((view.width() - 40.0) / b.width()).min((view.height() - 40.0) / b.height()).clamp(0.04, 1.6);
		self.center = b.center().to_vec2();
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

/// The map's disciplines, grouped, in the order the set first uses them.
pub fn discipline_groups(map: &DocMap) -> Vec<DisciplineGroup> {
	let mut groups: Vec<DisciplineGroup> = Vec::new();
	for s in &map.sheets {
		let key = discipline_key(s.discipline.as_deref());
		let (g, sub) = discipline_parts(&key);
		let gi = match groups.iter().position(|x| x.name == g) {
			Some(i) => i,
			None => {
				groups.push(DisciplineGroup { name: g.to_string(), color: discipline_color(Some(g)), count: 0, subs: vec![] });
				groups.len() - 1
			}
		};
		let grp = &mut groups[gi];
		grp.count += 1;
		match grp.subs.iter_mut().find(|x| x.0 == key) {
			Some(x) => x.2 += 1,
			None => grp.subs.push((key.clone(), sub.unwrap_or("General").to_string(), 1)),
		}
	}
	groups
}

/// Every discipline key in the map.
fn all_keys(groups: &[DisciplineGroup]) -> std::collections::BTreeSet<String> {
	groups.iter().flat_map(|g| g.keys().cloned()).collect()
}

/// What the discipline filter shows, in a few words: "All disciplines", "Civil", "Civil › Track", "3 disciplines".
fn discipline_summary(groups: &[DisciplineGroup], f: &MapFilter) -> String {
	if f.disciplines.is_empty() {
		return "All disciplines".into();
	}
	let whole: Vec<&DisciplineGroup> = groups.iter().filter(|g| g.keys().all(|k| f.disciplines.contains(k))).collect();
	let covered: usize = whole.iter().map(|g| g.subs.len()).sum();
	if whole.len() == 1 && covered == f.disciplines.len() {
		return whole[0].name.clone();
	}
	if f.disciplines.len() == 1 {
		let k = f.disciplines.iter().next().unwrap();
		return k.replace('/', " › ");
	}
	format!("{} disciplines", f.disciplines.len())
}

/// The discipline filter: a button saying what shows, opening a checklist
/// grouped by discipline (with counts, "Only", and search on long lists).
/// Returns whether the filter changed.
pub fn discipline_menu(ui: &mut Ui, p: &Palette, map: &DocMap, f: &mut MapFilter, width: f32) -> bool {
	let groups = discipline_groups(map);
	let all = all_keys(&groups);
	let shown: usize = if f.disciplines.is_empty() { map.sheets.len() } else { groups.iter().flat_map(|g| g.subs.iter()).filter(|s| f.disciplines.contains(&s.0)).map(|s| s.2).sum() };
	let active = !f.disciplines.is_empty();
	let mut changed = false;
	ui.horizontal(|ui| {
		ui.spacing_mut().item_spacing.x = 2.0;
		let label = format!("{}  {}   {shown}", i::FILTER, discipline_summary(&groups, f));
		let text = RichText::new(label).size(12.5).color(if active { p.text } else { p.text2 });
		let fill = if active { p.accent_fill.gamma_multiply(0.22) } else { Color32::TRANSPARENT };
		let stroke = Stroke::new(1.0, if active { p.accent_fill.gamma_multiply(0.7) } else { p.border });
		// Leave room for the clear button, so the row never grows its panel.
		let w = if active { width - 30.0 } else { width };
		let b = egui::Button::new(text).fill(fill).stroke(stroke).corner_radius(5).min_size(Vec2::new(w.max(60.0), 26.0)).right_text(RichText::new(i::CHEVRON_DOWN).font(theme::icon_font(11.0)).color(p.muted));
		let r = ui.add(b).on_hover_text("Show the sheets of some disciplines only");
		egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(w.max(260.0)).show(|ui| {
			changed |= discipline_list(ui, p, &groups, &all, f);
		});
		if active && widgets::icon_button(ui, p, i::X, "Show every discipline", false).clicked() {
			f.disciplines.clear();
			changed = true;
		}
	});
	changed
}

fn discipline_list(ui: &mut Ui, p: &Palette, groups: &[DisciplineGroup], all: &std::collections::BTreeSet<String>, f: &mut MapFilter) -> bool {
	let mut changed = false;
	let search_id = egui::Id::new("discipline-search");
	let mut q: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
	if all.len() > 8 {
		ui.add(egui::TextEdit::singleline(&mut q).hint_text("Find a discipline…").desired_width(f32::INFINITY));
		ui.data_mut(|d| d.insert_temp(search_id, q.clone()));
		ui.add_space(4.0);
	}
	let q = q.trim().to_lowercase();
	let total: usize = groups.iter().map(|g| g.count).sum();
	// Ticked = shown; an empty filter shows everything.
	let shown = |k: &String, f: &MapFilter| f.disciplines.is_empty() || f.disciplines.contains(k);
	let set = |keys: Vec<String>, on: bool, f: &mut MapFilter| {
		let mut cur: std::collections::BTreeSet<String> = if f.disciplines.is_empty() { all.clone() } else { f.disciplines.clone() };
		for k in keys {
			if on {
				cur.insert(k);
			} else {
				cur.remove(&k);
			}
		}
		f.disciplines = if cur == *all { Default::default() } else { cur };
	};
	if q.is_empty() && row(ui, p, None, "All disciplines", Some(total), Some(f.disciplines.is_empty()), 0.0).0 {
		f.disciplines.clear();
		changed = true;
	}
	ui.separator();
	egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
		for g in groups {
			let group_hit = g.name.to_lowercase().contains(&q);
			let subs: Vec<&(String, String, usize)> = g.subs.iter().filter(|s| q.is_empty() || group_hit || s.1.to_lowercase().contains(&q)).collect();
			if subs.is_empty() {
				continue;
			}
			let on = g.keys().filter(|k| shown(k, f)).count();
			let state = if on == g.subs.len() { Some(true) } else if on == 0 { Some(false) } else { None };
			let (clicked, only) = row(ui, p, Some(g.color), &g.name, Some(g.count), state, 0.0);
			if clicked {
				set(g.keys().cloned().collect(), state != Some(true), f);
				changed = true;
			}
			if only {
				f.disciplines = g.keys().cloned().collect();
				changed = true;
			}
			if g.subs.len() > 1 || g.subs[0].0 != g.name {
				for (key, label, n) in subs {
					let (clicked, only) = row(ui, p, None, label, Some(*n), Some(shown(key, f)), 18.0);
					if clicked {
						set(vec![key.clone()], !shown(key, f), f);
						changed = true;
					}
					if only {
						f.disciplines = [key.clone()].into();
						changed = true;
					}
				}
			}
		}
	});
	changed
}

/// A checklist row: box (None = some), dot, name, count; "Only" on hover.
/// Returns (toggled, only).
fn row(ui: &mut Ui, p: &Palette, color: Option<Color32>, name: &str, count: Option<usize>, state: Option<bool>, indent: f32) -> (bool, bool) {
	let w = ui.available_width();
	let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
	let hovered = resp.hovered();
	if hovered {
		ui.painter().rect_filled(r, 4.0, p.hover);
	}
	let pnt = ui.painter();
	let bx = Rect::from_center_size(Pos2::new(r.left() + 12.0 + indent, r.center().y), Vec2::splat(13.0));
	let on = state != Some(false);
	pnt.rect_filled(bx, 3.0, if on { p.accent_fill } else { Color32::TRANSPARENT });
	pnt.rect_stroke(bx, 3.0, Stroke::new(1.0, if on { p.accent_fill } else { p.muted }), StrokeKind::Inside);
	match state {
		Some(true) => {
			pnt.text(bx.center(), egui::Align2::CENTER_CENTER, i::CHECK, theme::icon_font(10.0), p.on_accent);
		}
		None => {
			pnt.line_segment([bx.left_center() + Vec2::new(3.0, 0.0), bx.right_center() - Vec2::new(3.0, 0.0)], Stroke::new(1.6, p.on_accent));
		}
		Some(false) => {}
	}
	let mut x = bx.right() + 8.0;
	if let Some(c) = color {
		pnt.circle_filled(Pos2::new(x + 3.0, r.center().y), 3.5, c);
		x += 12.0;
	}
	let font = if color.is_some() || indent == 0.0 { theme::semibold(12.5) } else { egui::FontId::proportional(12.5) };
	pnt.text(Pos2::new(x, r.center().y), egui::Align2::LEFT_CENTER, name, font, p.text);
	if let Some(n) = count {
		pnt.text(Pos2::new(r.right() - 8.0, r.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), egui::FontId::proportional(11.5), p.muted);
	}
	let mut only = false;
	if hovered && state.is_some() {
		let o = Rect::from_min_size(Pos2::new(r.right() - 64.0, r.top() + 3.0), Vec2::new(32.0, 18.0));
		let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|h| o.contains(h));
		ui.painter().rect_filled(o, 3.0, if over { p.accent_fill } else { p.panel });
		ui.painter().text(o.center(), egui::Align2::CENTER_CENTER, "Only", egui::FontId::proportional(11.0), if over { p.on_accent } else { p.text2 });
		if resp.clicked() && over {
			only = true;
		}
	}
	if hovered {
		ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
	}
	(resp.clicked() && !only, only)
}

/// Which kinds of area show as cards: a button saying how many, opening a checklist.
fn kinds_menu(ui: &mut Ui, p: &Palette, f: &mut MapFilter) {
	let groups: [(&str, &[PartKind]); 5] = [
		("Drawings", &[PartKind::Plan, PartKind::KeyPlan, PartKind::Diagram, PartKind::Other]),
		("Details & sections", &[PartKind::Detail, PartKind::Section, PartKind::Elevation]),
		("Schedules & legends", &[PartKind::Schedule, PartKind::Legend]),
		("Notes & elements", &[PartKind::Notes, PartKind::Element]),
		("Callouts", &[PartKind::Callout]),
	];
	let on = groups.iter().filter(|(_, ks)| ks.iter().all(|k| f.kinds.contains(k))).count();
	let label = if on == groups.len() { "Show".to_string() } else if on == 0 { "Sheets only".to_string() } else { format!("Show {on}/{}", groups.len()) };
	let active = on != groups.len();
	let b = egui::Button::new(RichText::new(format!("{}  {label}", i::LAYERS)).size(12.5).color(if active { p.text } else { p.text2 }))
		.fill(if active { p.accent_fill.gamma_multiply(0.22) } else { Color32::TRANSPARENT })
		.stroke(Stroke::new(1.0, if active { p.accent_fill.gamma_multiply(0.7) } else { p.border }))
		.corner_radius(5)
		.min_size(Vec2::new(0.0, 26.0))
		.right_text(RichText::new(i::CHEVRON_DOWN).font(theme::icon_font(11.0)).color(p.muted));
	let r = ui.add(b).on_hover_text("Which kinds of area show as cards around their sheets");
	egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(240.0).show(|ui| {
		for (name, ks) in groups {
			let all_on = ks.iter().all(|k| f.kinds.contains(k));
			if row(ui, p, Some(kind_color(ks[0])), name, None, Some(all_on), 0.0).0 {
				for k in ks {
					if all_on {
						f.kinds.remove(k);
					} else {
						f.kinds.insert(*k);
					}
				}
			}
		}
	});
}

#[allow(clippy::too_many_arguments)]
fn top_bar(ui: &mut Ui, doc: &mut OpenDoc, st: &mut GraphState, map: &DocMap, p: &Palette, chat: &MapChat, bar: Rect, action: &mut Option<GraphAction>) {
	ui.painter().rect_filled(bar, 0.0, p.panel);
	ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, p.border));
	let inner = bar.shrink2(Vec2::new(10.0, 5.0));
	// The right end is laid out first, so the Map menu always fits; the
	// counts show there only when there is room (else in the Map menu).
	let wide = inner.width() > 960.0;
	let right_w = 84.0 + if wide { 300.0 } else { 0.0 } + if chat.running_here { 190.0 } else { 0.0 };
	let right = Rect::from_min_max(egui::pos2(inner.right() - right_w, inner.top()), inner.max);
	let left = Rect::from_min_max(inner.min, egui::pos2(right.left() - 8.0, inner.bottom()));
	let mapped = map.pages_mapped().len();
	let areas: usize = map.sheets.iter().map(|s| s.parts.len()).sum();
	let counts = format!("{} sheets · {mapped} mapped · {areas} areas · {} links", map.sheets.len(), map.links.len());
	let tip = format!("{} of {} pages indexed. Built by {}{}", map.sheets.len(), map.page_count, map.model.as_deref().unwrap_or("the AI"), map.built_at.as_deref().map(|t| format!(", last change {}", t.split('T').next().unwrap_or(t))).unwrap_or_default());
	{
		let mut child = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
		let ui = &mut child;
		ui.set_clip_rect(right.expand(4.0));
		ui.spacing_mut().item_spacing.x = 8.0;
		ui.menu_button(RichText::new(format!("{}  Map", i::WAYPOINTS)).size(12.5), |ui| {
			ui.set_min_width(250.0);
			if !wide {
				ui.label(RichText::new(&counts).size(12.0).color(p.text2)).on_hover_text(&tip);
				ui.separator();
			}
			let busy = chat.running_here || chat.busy_elsewhere;
			if ui.add_enabled(!busy, egui::Button::new(format!("{}  Continue mapping…", i::SPARKLES))).on_hover_text("The AI picks up the pages not mapped yet, and anything you ask").clicked() {
				st.dialog = Some(BuildDialog { fresh: false });
				ui.close();
			}
			if ui.add_enabled(!busy, egui::Button::new(format!("{}  Map again from scratch…", i::REFRESH_CW))).clicked() {
				st.dialog = Some(BuildDialog { fresh: true });
				ui.close();
			}
			ui.separator();
			if ui.add_enabled(!chat.running_here, egui::Button::new(format!("{}  Delete map", i::TRASH_2))).clicked() {
				*action = Some(GraphAction::Delete);
				ui.close();
			}
		});
		if chat.running_here {
			if ui.small_button("Stop").on_hover_text("Stop the AI. What it recorded stays in the map.").clicked() {
				*action = Some(GraphAction::Stop);
			}
			if ui.small_button("Open chat").clicked() {
				*action = Some(GraphAction::OpenChat);
			}
			ui.add(egui::Spinner::new().size(13.0));
			ui.label(RichText::new("Mapping").size(12.0).color(p.text2));
		}
		if wide {
			ui.add(egui::Label::new(RichText::new(&counts).size(12.0).color(p.text2)).truncate()).on_hover_text(&tip);
		}
	}
	let mut child = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::left_to_right(egui::Align::Center)));
	let ui = &mut child;
	ui.set_clip_rect(left.expand2(Vec2::new(4.0, 6.0)));
	ui.spacing_mut().item_spacing.x = 6.0;
	if map.page_count != doc.page_count() && doc.loaded {
		ui.label(RichText::new(i::CIRCLE_ALERT).font(theme::icon_font(14.0)).color(p.warning)).on_hover_text("The PDF has changed since it was mapped: map it again (Map menu)");
	}
	if let Some((msg, err)) = &doc.map_status {
		ui.label(RichText::new(i::CIRCLE_ALERT).font(theme::icon_font(14.0)).color(if *err { p.danger } else { p.muted })).on_hover_text(msg.as_str());
	}
	// Search shrinks first when the bar is short.
	let search_w = (left.width() - 430.0).clamp(90.0, 210.0);
	ui.label(RichText::new(i::SEARCH).font(theme::icon_font(13.0)).color(p.muted));
	let r = ui.add(egui::TextEdit::singleline(&mut st.search).id(egui::Id::new("graph-search")).hint_text("Find a sheet, detail, note…").desired_width(search_w));
	if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
		if let Some(id) = first_match(map, &st.search, st) {
			*action = Some(GraphAction::Focus(id));
		}
	}
	discipline_menu(ui, p, map, &mut doc.map_filter, 150.0);
	kinds_menu(ui, p, &mut doc.map_filter);
	// Board (arranged) or web.
	{
		let frame = egui::Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(5).inner_margin(egui::Margin::same(2));
		frame.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			for (board, label, tip) in [(true, "Board", "Sheets arranged by discipline, each with its areas beneath it"), (false, "Web", "Laid out by how things link")] {
				let on = st.board == board;
				let b = egui::Button::new(RichText::new(label).size(12.0).color(if on { p.text } else { p.text2 })).fill(if on { p.hover } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).corner_radius(4);
				if ui.add(b).on_hover_text(tip).clicked() && !on {
					st.board = board;
					st.auto_fit = true;
				}
			}
		});
	}
	// Focus: a local graph around the selection.
	let has_sel = st.selected.is_some();
	let focus_tip = if has_sel { "Show only what is linked to the selection: 1 step, 2 steps, everything" } else { "Select a card first, then show only what is linked to it" };
	ui.add_enabled_ui(has_sel || st.local > 0, |ui| {
		let label = match st.local {
			0 => format!("{}  Focus", i::CROSSHAIR),
			n => format!("{}  Focus · {n}", i::CROSSHAIR),
		};
		let on = st.local > 0;
		let b = egui::Button::new(RichText::new(label).size(12.5).color(if on { p.on_accent } else { p.text })).fill(if on { p.accent_fill } else { Color32::TRANSPARENT }).corner_radius(5).min_size(Vec2::new(0.0, 26.0));
		if ui.add(b).on_hover_text(focus_tip).clicked() {
			st.local = (st.local + 1) % 3;
			st.auto_fit = true;
		}
	});
	if widgets::icon_button(ui, p, i::MAXIMIZE, "Fit the map in view", false).clicked() {
		st.auto_fit = true;
	}
}

fn first_match(map: &DocMap, q: &str, st: &GraphState) -> Option<String> {
	let words: Vec<String> = q.to_lowercase().split_whitespace().map(str::to_string).collect();
	if words.is_empty() {
		return None;
	}
	map.node_ids().into_iter().filter(|id| st.index.contains_key(id)).find(|id| map.node(id).is_some_and(|n| matches(n, &words)))
}

fn matches(n: NodeRef, words: &[String]) -> bool {
	let hay = format!("{} {}", n.title(), n.summary()).to_lowercase();
	words.iter().all(|w| hay.contains(w))
}

#[allow(clippy::too_many_arguments)]
fn graph_canvas(ui: &mut Ui, doc: &mut OpenDoc, st: &mut GraphState, map: &DocMap, p: &Palette, view: Rect, wheel_zooms: bool, action: &mut Option<GraphAction>) {
	// The details panel opening or closing resizes the view: keep what's on
	// screen where it is (the second click of a double-click lands on its card).
	if st.view.is_positive() && st.view != view && !st.auto_fit {
		st.center += (view.center() - st.view.center()) / st.zoom;
	}
	st.view = view;
	// Re-arrange the board when the view's shape changes much.
	let aspect = view.width() / view.height().max(1.0);
	if st.board && ((aspect / st.aspect).ln()).abs() > 0.25 {
		st.aspect = aspect;
		st.synced = None;
	}
	let resp = ui.interact(view, egui::Id::new("graph-canvas"), Sense::click_and_drag());
	// Layout.
	if st.alpha > 0.0 {
		for _ in 0..3 {
			st.tick();
		}
		ui.ctx().request_repaint();
	}
	// Bring a node chosen elsewhere (Map tab, search, the sheet) into view, if it isn't.
	// (Not while the first layout is fitting itself: everything will be in view.)
	if let Some(id) = st.focus.take().filter(|_| !st.auto_fit) {
		if let Some(&n) = st.index.get(&id) {
			let r = st.node_rect(view, &st.nodes[n]);
			if !view.shrink(24.0).contains_rect(r) || r.width() < CHIP_W {
				st.center = st.nodes[n].pos;
				st.zoom = st.zoom.max(0.55);
			}
			st.auto_fit = false;
		}
	}
	if st.auto_fit {
		st.fit(view);
		if st.alpha <= 0.0 {
			st.auto_fit = false;
		}
	}
	// Navigation: as on the sheet (pinch zooms; the wheel zooms or scrolls).
	let (zoom, scroll, mods, hover) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers, i.pointer.hover_pos()));
	if let Some(t) = ui.input(|i| i.raw.events.iter().rev().find_map(|e| if let egui::Event::MouseWheel { unit, .. } = e { Some(*unit == egui::MouseWheelUnit::Point) } else { None })) {
		st.trackpad = t;
	}
	if resp.hovered() {
		let anchor = hover.unwrap_or(view.center());
		let mut factor = 1.0;
		if zoom != 1.0 {
			factor = zoom;
		} else if scroll != Vec2::ZERO {
			if wheel_zooms && !st.trackpad && !mods.shift && !mods.alt {
				factor = (scroll.y / 400.0).exp();
			} else {
				st.center -= scroll / st.zoom;
				st.auto_fit = false;
			}
		}
		if factor != 1.0 {
			let before = st.center + (anchor - view.center()) / st.zoom;
			st.zoom = (st.zoom * factor).clamp(0.03, 3.0);
			st.center = before - (anchor - view.center()) / st.zoom;
			st.auto_fit = false;
		}
	}
	let under = hover.filter(|h| view.contains(*h)).and_then(|h| st.node_at(view, h));
	if resp.drag_started() {
		st.drag_node = ui.input(|i| i.pointer.press_origin()).and_then(|o| st.node_at(view, o));
	}
	if resp.dragged() {
		let d = resp.drag_delta() / st.zoom;
		match st.drag_node {
			Some(n) => {
				st.nodes[n].pos += d;
				st.nodes[n].pinned = true;
				st.alpha = st.alpha.max(0.12);
			}
			None => {
				st.center -= d;
				ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
			}
		}
		st.auto_fit = false;
	}
	if resp.drag_stopped() {
		st.drag_node = None;
	}
	if resp.clicked() {
		match under {
			Some(n) => {
				st.selected = Some(st.nodes[n].id.clone());
				*action = Some(GraphAction::Focus(st.nodes[n].id.clone()));
			}
			None => st.selected = None,
		}
	}
	if resp.double_clicked() {
		if let Some(n) = under {
			*action = Some(GraphAction::Open(st.nodes[n].id.clone()));
		}
	}
	st.hovered = under;
	if under.is_some() && st.drag_node.is_none() {
		ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
	}

	// What to bring forward: the hovered or selected node and its neighbours, or search hits.
	let focus = under.or_else(|| st.selected.as_ref().and_then(|s| st.index.get(s).copied()));
	let near = focus.map(|f| st.neighbours(f));
	let words: Vec<String> = st.search.to_lowercase().split_whitespace().map(str::to_string).collect();
	let hits: Option<HashSet<usize>> = (!words.is_empty()).then(|| (0..st.nodes.len()).filter(|&i| map.node(&st.nodes[i].id).is_some_and(|n| matches(n, &words))).collect());
	let lit = |i: usize| near.as_ref().is_none_or(|s| s.contains(&i)) && hits.as_ref().is_none_or(|h| h.contains(&i) || near.as_ref().is_some_and(|s| s.contains(&i)));

	let painter = ui.painter_at(view);
	// Dot grid.
	let step = 48.0 * st.zoom;
	if step > 10.0 {
		let origin = st.to_screen(view, Vec2::ZERO);
		let dot = if p.dark { Color32::from_white_alpha(14) } else { Color32::from_black_alpha(18) };
		let mut x = view.left() + (origin.x - view.left()).rem_euclid(step);
		while x < view.right() {
			let mut y = view.top() + (origin.y - view.top()).rem_euclid(step);
			while y < view.bottom() {
				painter.circle_filled(Pos2::new(x, y), 1.0, dot);
				y += step;
			}
			x += step;
		}
	}
	// The board: lanes per discipline, a panel behind each sheet's column.
	for (k, (name, color, n, r)) in st.lanes.iter().enumerate() {
		let sr = Rect::from_min_max(st.to_screen(view, r.min.to_vec2()), st.to_screen(view, r.max.to_vec2()));
		if !view.intersects(sr) {
			continue;
		}
		let tint = if p.dark { Color32::from_white_alpha(if k % 2 == 0 { 6 } else { 3 }) } else { Color32::from_black_alpha(if k % 2 == 0 { 8 } else { 4 }) };
		painter.rect_filled(sr, (10.0 * st.zoom).clamp(2.0, 12.0), tint);
		painter.rect_filled(Rect::from_min_size(sr.min, Vec2::new((4.0 * st.zoom).clamp(2.0, 5.0), sr.height())), 2.0, color.gamma_multiply(0.8));
		let size = (20.0 * st.zoom.sqrt()).clamp(11.0, 17.0);
		let label = format!("{}  ·  {n} sheet{}", name.to_uppercase(), if *n == 1 { "" } else { "s" });
		painter.text(sr.min + Vec2::new((24.0 * st.zoom).max(8.0), (16.0 * st.zoom).max(4.0)), egui::Align2::LEFT_TOP, label, theme::semibold(size), *color);
	}
	for (c, more) in &st.columns {
		let sr = Rect::from_min_max(st.to_screen(view, c.min.to_vec2()), st.to_screen(view, c.max.to_vec2()));
		if view.intersects(sr) && sr.width() > 6.0 {
			let fill = if p.dark { Color32::from_white_alpha(7) } else { Color32::from_black_alpha(6) };
			painter.rect_filled(sr, (8.0 * st.zoom).clamp(1.0, 10.0), fill);
			if *more > 0 && sr.width() > 60.0 {
				let size = (12.0 * st.zoom).clamp(9.0, 13.0);
				painter.text(egui::pos2(sr.center().x, sr.bottom() - 13.0 * st.zoom), egui::Align2::CENTER_CENTER, format!("+{more} more · in the Map tab"), egui::FontId::proportional(size), p.muted);
			}
		}
	}
	// Discipline names over their clusters.
	if st.clusters.len() > 1 && !st.board {
		for (name, anchor) in &st.clusters {
			let members: Vec<&GNode> = st.nodes.iter().filter(|n| n.anchor == *anchor).collect();
			let Some(top) = members.iter().map(|n| n.pos.y - n.size.y / 2.0).min_by(f32::total_cmp) else { continue };
			let cx = members.iter().map(|n| n.pos.x).sum::<f32>() / members.len() as f32;
			let at = st.to_screen(view, Vec2::new(cx, top - 40.0));
			let size = (22.0 * st.zoom.sqrt()).clamp(11.0, 26.0);
			let c = discipline_color(Some(name));
			painter.text(at, egui::Align2::CENTER_BOTTOM, name.to_uppercase(), theme::semibold(size), c.gamma_multiply(if focus.is_some() { 0.35 } else { 0.85 }));
		}
	}
	// Edges: sheet–part first, then links on top.
	for pass in [false, true] {
		for e in st.edges.iter().filter(|e| e.kind.is_some() == pass) {
			let (a, b) = (&st.nodes[e.a], &st.nodes[e.b]);
			let (ra, rb) = (st.node_rect(view, a), st.node_rect(view, b));
			if !view.intersects(ra.union(rb)) {
				continue;
			}
			let on = focus.is_some_and(|f| f == e.a || f == e.b);
			let dim = (focus.is_some() && !on) || (hits.is_some() && !(lit(e.a) && lit(e.b)));
			match e.kind {
				// On the board an area's column says which sheet it is on.
				None if st.board => {}
				None => {
					let c = if dim { p.border.gamma_multiply(0.4) } else { p.border };
					painter.line_segment([ra.center(), rb.center()], Stroke::new(1.0, c));
				}
				// A board shows the links of what you hover or pick, not all at once.
				Some(_) if st.board && !on => {}
				Some(k) => {
					let mut c = link_color(k);
					if dim {
						c = c.gamma_multiply(0.15);
					} else if st.board && focus.is_none() {
						// Many links cross a board: quiet until something is picked.
						c = c.gamma_multiply(0.45);
					}
					let w = (1.4 + (e.count.min(4) as f32 - 1.0) * 0.6) * if on { 1.6 } else { 1.0 };
					let from = edge_point(ra, rb.center());
					let to = edge_point(rb, ra.center());
					if st.board {
						// A soft arc, so links between neighbours don't run over the cards between.
						let d = to - from;
						let bend = Vec2::new(-d.y, d.x) * 0.18;
						let (c1, c2) = (from + d * 0.25 + bend, from + d * 0.75 + bend);
						painter.add(egui::epaint::CubicBezierShape::from_points_stroke([from, c1, c2, to], false, Color32::TRANSPARENT, Stroke::new(w, c)));
						arrow_head(&painter, c2, to, (6.0 + w * 1.5).min(12.0), c);
					} else {
						painter.line_segment([from, to], Stroke::new(w, c));
						arrow_head(&painter, from, to, (6.0 + w * 1.5).min(12.0), c);
					}
				}
			}
		}
	}
	// Nodes.
	let selected = st.selected.as_ref().and_then(|s| st.index.get(s).copied());
	let mut order = st.draw_order();
	// Bring the focus to the top.
	if let Some(f) = focus {
		order.retain(|&i| i != f);
		order.push(f);
	}
	for i in order {
		let r = st.node_rect(view, &st.nodes[i]);
		if !view.intersects(r) {
			continue;
		}
		let Some(node) = map.node(&st.nodes[i].id) else { continue };
		let alpha = if lit(i) { 1.0 } else { 0.22 };
		draw_node(&painter, doc, &st.nodes[i], node, r, p, alpha, selected == Some(i), under == Some(i));
	}
	// Name on hover.
	if let Some(n) = under.and_then(|u| map.node(&st.nodes[u].id)) {
		let tip = if n.summary().is_empty() { n.title() } else { format!("{}\n{}", n.title(), n.summary()) };
		resp.on_hover_text_at_pointer(format!("{tip}\n\nClick for its links · double-click to open it on the sheet"));
	}
	let _ = action;
}

/// Where the line from `rect`'s centre towards `toward` leaves the rect.
fn edge_point(rect: Rect, toward: Pos2) -> Pos2 {
	let c = rect.center();
	let d = toward - c;
	if d.length_sq() < 1e-6 {
		return c;
	}
	let sx = if d.x.abs() > 1e-6 { rect.width() / 2.0 / d.x.abs() } else { f32::INFINITY };
	let sy = if d.y.abs() > 1e-6 { rect.height() / 2.0 / d.y.abs() } else { f32::INFINITY };
	c + d * sx.min(sy).min(1.0)
}

fn arrow_head(painter: &egui::Painter, from: Pos2, to: Pos2, size: f32, c: Color32) {
	let d = to - from;
	if d.length() < size * 1.5 {
		return;
	}
	let dir = d.normalized();
	let n = Vec2::new(-dir.y, dir.x);
	let base = to - dir * size;
	painter.add(egui::Shape::convex_polygon(vec![to, base + n * size * 0.5, base - n * size * 0.5], c, Stroke::NONE));
}

#[allow(clippy::too_many_arguments)]
fn draw_node(painter: &egui::Painter, doc: &mut OpenDoc, g: &GNode, n: NodeRef, r: Rect, p: &Palette, alpha: f32, selected: bool, hovered: bool) {
	let fade = |c: Color32| if alpha < 1.0 { c.gamma_multiply(alpha) } else { c };
	if r.width() < CHIP_W {
		painter.rect_filled(r, 3.0, fade(unmapped_tint(g.color, n)));
		if selected {
			painter.rect_stroke(r.expand(2.0), 4.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
		}
		return;
	}
	let corner = (6.0 * (r.width() / SHEET_SIZE.x)).clamp(2.0, 8.0);
	if alpha >= 1.0 {
		painter.rect_filled(r.translate(Vec2::new(0.0, 2.0)), corner, Color32::from_black_alpha(if p.dark { 60 } else { 22 }));
	}
	painter.rect_filled(r, corner, fade(p.card));
	let stroke = if selected { Stroke::new(2.0, p.accent) } else if hovered { Stroke::new(1.5, p.text2) } else { Stroke::new(1.0, fade(p.border)) };
	painter.rect_stroke(r, corner, stroke, StrokeKind::Inside);
	// Colour band on top: discipline for a sheet, kind for a part.
	let band = Rect::from_min_size(r.min, Vec2::new(r.width(), (4.0 * r.width() / SHEET_SIZE.x).clamp(2.0, 4.0)));
	painter.rect_filled(band, egui::CornerRadius { nw: corner as u8, ne: corner as u8, sw: 0, se: 0 }, fade(unmapped_tint(g.color, n)));

	// Small cards give their room to the picture: a slim label, a thin margin.
	let label_h = if r.height() > 64.0 { 32.0 } else if r.height() > 22.0 { 13.0 } else { 0.0 };
	let pad = (r.width() * 0.035).clamp(1.5, 5.0);
	let img_area = Rect::from_min_max(r.min + Vec2::new(pad, band.height() + pad), Pos2::new(r.right() - pad, r.bottom() - label_h - pad * 0.6));
	if img_area.height() > 9.0 {
		let tex = match n {
			NodeRef::Sheet(s) => doc.thumb(s.page).map(|t| (t, doc.page_size(s.page))),
			NodeRef::Part(s, part) => part.region.and_then(|reg| {
				let (w, h) = doc.page_size(s.page);
				doc.crop(&crop_key(&part.id, reg), s.page, reg, CROP_PX).map(|t| (t, (reg.w * w, reg.h * h)))
			}),
		};
		match tex {
			Some((t, (w, h))) => {
				let aspect = (w / h.max(1e-6)) as f32;
				let fit = if img_area.width() / img_area.height() > aspect { Vec2::new(img_area.height() * aspect, img_area.height()) } else { Vec2::new(img_area.width(), img_area.width() / aspect) };
				let ir = Rect::from_center_size(img_area.center(), fit);
				painter.rect_filled(ir, 1.0, fade(Color32::WHITE));
				painter.image(t.id(), ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), fade(Color32::WHITE));
			}
			None => {
				painter.rect_filled(img_area, 2.0, fade(p.hover));
				let icon = match n {
					NodeRef::Sheet(_) => i::FILE_TEXT,
					NodeRef::Part(..) => i::FRAME,
				};
				painter.text(img_area.center(), egui::Align2::CENTER_CENTER, icon, theme::icon_font((img_area.height() * 0.35).min(22.0)), fade(p.muted));
			}
		}
	}
	if label_h > 0.0 {
		let (head, sub) = match n {
			NodeRef::Sheet(s) => (s.number.clone().unwrap_or_else(|| format!("Page {}", s.page)), s.title.clone()),
			NodeRef::Part(_, part) => (match &part.label {
				// A callout reads as what is printed in it: "3/A-501".
				Some(l) if part.kind == PartKind::Callout => l.clone(),
				Some(l) => format!("{} {l}", part.kind.label()),
				None => part.kind.label().to_string(),
			}, part.title.clone()),
		};
		let w = r.width() - pad * 2.0;
		let top = r.bottom() - label_h - 1.0;
		let g1 = one_line(painter, &head, theme::semibold(if label_h > 20.0 { 11.5 } else { 9.5 }), fade(p.text), w);
		painter.galley(Pos2::new(r.left() + pad, top), g1, fade(p.text));
		if label_h > 20.0 && !sub.is_empty() {
			let g2 = one_line(painter, &sub, egui::FontId::proportional(10.5), fade(p.text2), w);
			painter.galley(Pos2::new(r.left() + pad, top + 15.0), g2, fade(p.text2));
		}
	}
}

/// A sheet only indexed so far shows its colour faded.
fn unmapped_tint(c: Color32, n: NodeRef) -> Color32 {
	match n {
		NodeRef::Sheet(s) if !s.mapped => c.gamma_multiply(0.4),
		_ => c,
	}
}

fn one_line(painter: &egui::Painter, text: &str, font: egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
	let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
	job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(8.0));
	painter.layout_job(job)
}

/// The link colours, in a corner (bottom right on the board, where lanes start at the left).
fn legend(ui: &mut Ui, p: &Palette, graph: Rect, right: bool) {
	let kinds = LinkKind::ALL;
	let painter = ui.painter_at(graph);
	let row = 16.0;
	let h = kinds.len() as f32 * row + 12.0;
	let x = if right { graph.right() - 118.0 - 12.0 } else { graph.left() + 12.0 };
	let r = Rect::from_min_size(Pos2::new(x, graph.bottom() - h - 12.0), Vec2::new(118.0, h));
	if graph.height() < h + 120.0 {
		return;
	}
	painter.rect_filled(r, 6.0, p.panel.gamma_multiply(0.92));
	painter.rect_stroke(r, 6.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
	for (k, kind) in kinds.iter().enumerate() {
		let y = r.top() + 6.0 + row * (k as f32 + 0.5);
		let c = link_color(*kind);
		painter.line_segment([Pos2::new(r.left() + 10.0, y), Pos2::new(r.left() + 28.0, y)], Stroke::new(2.0, c));
		arrow_head(&painter, Pos2::new(r.left() + 10.0, y), Pos2::new(r.left() + 30.0, y), 6.0, c);
		painter.text(Pos2::new(r.left() + 38.0, y), egui::Align2::LEFT_CENTER, kind.label(), egui::FontId::proportional(11.0), p.text2);
	}
}

pub fn link_row(ui: &mut Ui, p: &Palette, color: Color32, title: &str, sub: &str) -> egui::Response {
	let resp = egui::Frame::new().corner_radius(4).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
		ui.set_width(ui.available_width());
		ui.horizontal(|ui| {
			ui.label(RichText::new("●").size(10.0).color(color));
			ui.vertical(|ui| {
				ui.add(egui::Label::new(RichText::new(title).size(12.5).color(p.text)).truncate());
				if !sub.is_empty() {
					ui.add(egui::Label::new(RichText::new(sub).size(11.0).color(p.muted)).truncate());
				}
			});
		});
	});
	let r = resp.response.interact(Sense::click());
	if r.hovered() {
		ui.painter().rect_filled(r.rect, 4.0, p.hover.gamma_multiply(0.6));
		ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
	}
	r
}

fn waiting_state(ui: &mut Ui, chat: &MapChat, p: &Palette, rect: Rect, action: &mut Option<GraphAction>) {
	let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(16.0)).layout(egui::Layout::top_down(egui::Align::Center)));
	let ui = &mut child;
	ui.add_space((rect.height() * 0.25).max(12.0));
	ui.label(RichText::new(i::WAYPOINTS).font(theme::icon_font(30.0)).color(p.accent));
	ui.add_space(8.0);
	ui.label(RichText::new("The AI is surveying the set…").font(theme::semibold(16.0)));
	ui.add_space(4.0);
	ui.label(RichText::new(format!("On {}. Sheets appear here as it records them; follow along or add instructions in the chat.", chat.model)).size(12.5).color(p.text2));
	ui.add_space(12.0);
	ui.add(egui::Spinner::new().size(18.0));
	ui.add_space(12.0);
	ui.horizontal(|ui| {
		ui.add_space((ui.available_width() - 190.0).max(0.0) / 2.0);
		if widgets::secondary_button(ui, p, &format!("{}  Open chat", i::SPARKLES)).clicked() {
			*action = Some(GraphAction::OpenChat);
		}
		if widgets::secondary_button(ui, p, "Stop").clicked() {
			*action = Some(GraphAction::Stop);
		}
	});
}

#[allow(clippy::too_many_arguments)]
fn build_card(ui: &mut Ui, doc: &mut OpenDoc, st: &mut GraphState, p: &Palette, chat: &MapChat, in_dialog: bool, action: &mut Option<GraphAction>) {
	let width = ui.available_width().min(500.0);
	let fresh = st.dialog.as_ref().is_some_and(|d| d.fresh);
	let has_map = doc.map.as_ref().is_some_and(|m| !m.sheets.is_empty());
	ui.allocate_ui_with_layout(Vec2::new(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
		ui.set_width(width);
		if !in_dialog {
			ui.vertical_centered(|ui| {
				ui.label(RichText::new(i::WAYPOINTS).font(theme::icon_font(30.0)).color(p.accent));
				ui.add_space(6.0);
				ui.label(RichText::new("Map this drawing set").font(theme::semibold(17.0)));
			});
			ui.add_space(6.0);
			ui.label(
				RichText::new("The AI maps the set the way it does a takeoff: it skims every sheet, then looks closely, zooms in and records what matters (plans, details, sections, schedules, notes, callouts) and how each one relates to the others. Browse it here as a graph of pictures cut from the sheets; the AI uses it whenever you ask about the drawings.")
					.size(12.5)
					.color(p.text2),
			);
			ui.add_space(14.0);
		}
		widgets::card(ui, p, |ui| {
			ui.horizontal(|ui| {
				ui.label(RichText::new("Runs in the AI chat on").size(12.5).color(p.text2));
				ui.label(RichText::new("●").size(11.0).color(chat.color));
				ui.label(RichText::new(chat.model.as_str()).font(theme::semibold(12.5)));
				if ui.link(RichText::new("Change").size(12.0)).on_hover_text("Pick the model in the AI tab").clicked() {
					*action = Some(GraphAction::OpenChat);
				}
			});
			ui.add_space(6.0);
			ui.label(RichText::new("Instructions for the AI (optional)").size(12.0).color(p.text2));
			ui.add(egui::TextEdit::multiline(&mut st.instructions).hint_text("e.g. Only the architectural and structural sheets. Pay attention to the wall types and where each is detailed. Sheet numbers are bottom-left on this set.").desired_rows(3).desired_width(f32::INFINITY));
		});
		ui.add_space(6.0);
		widgets::hint(ui, p, "On a big set this takes a while: the AI looks at every sheet up close and may split the work among subagents running in parallel. It uses your model's credits or plan limits like any chat. You can watch, add instructions and stop it in the chat; what it recorded is kept.");
		if fresh {
			ui.label(RichText::new("The current map is deleted and the AI starts over.").size(12.0).color(p.warning));
		}
		let blocked = if chat.unset {
			Some("Choose a model in the AI tab first.")
		} else if chat.running_here || chat.busy_elsewhere {
			Some("The AI is busy with another message: stop it or let it finish first.")
		} else {
			None
		};
		if let Some(b) = blocked {
			ui.label(RichText::new(b).size(12.0).color(p.warning));
		}
		if let Some((msg, true)) = &doc.map_status {
			ui.label(RichText::new(msg.as_str()).size(12.0).color(p.danger));
		}
		ui.add_space(10.0);
		ui.horizontal(|ui| {
			let label = if fresh { "Map again" } else if has_map { "Continue mapping" } else { "Start mapping" };
			if ui.add_enabled(blocked.is_none(), egui::Button::new(RichText::new(format!("{}  {label}", i::SPARKLES)).font(theme::semibold(13.0)).color(p.on_accent)).fill(p.accent_fill).corner_radius(5).min_size(Vec2::new(0.0, 30.0))).clicked() {
				*action = Some(GraphAction::StartChat { instructions: st.instructions.trim().to_string(), fresh });
				st.dialog = None;
			}
			if in_dialog && widgets::secondary_button(ui, p, "Cancel").clicked() {
				st.dialog = None;
			}
		});
	});
}

fn build_dialog(ctx: &egui::Context, doc: &mut OpenDoc, st: &mut GraphState, p: &Palette, chat: &MapChat, action: &mut Option<GraphAction>) {
	let fresh = st.dialog.as_ref().is_some_and(|d| d.fresh);
	let modal = egui::Modal::new(egui::Id::new("map-dialog")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(18).corner_radius(8));
	let resp = modal.show(ctx, |ui| {
		ui.set_width(480.0);
		ui.label(RichText::new(if fresh { "Map the set again" } else { "Continue mapping" }).font(theme::semibold(16.0)));
		ui.add_space(4.0);
		widgets::hint(ui, p, if fresh { "The AI starts over from an empty map." } else { "The AI picks up the sheets not mapped yet. Tell it anything else to do: map one discipline more closely, link the notes, check a sheet again." });
		ui.add_space(8.0);
		build_card(ui, doc, st, p, chat, true, action);
	});
	if resp.should_close() {
		st.dialog = None;
	}
}
