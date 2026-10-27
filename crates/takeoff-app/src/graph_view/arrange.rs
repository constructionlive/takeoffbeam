//! Placing the nodes: the starting arrangement and the force simulation that
//! settles it, plus fitting the result to the view.

use super::*;

impl GraphState {
	/// The board: a lane per discipline in the order the set uses them, its
	/// sheets in page order (wrapping after a few), each sheet's areas in a
	/// tidy stack beneath it: drawings, details and sections, schedules,
	/// notes, then callouts.
	pub(super) fn arrange(&mut self, map: &DocMap) {
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
	pub(super) fn arrange_with(&mut self, map: &DocMap, per_row: usize) -> (f32, f32) {
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
	pub(super) fn tick(&mut self) {
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

	pub(super) fn bounds(&self) -> Option<Rect> {
		let mut r: Option<Rect> = None;
		for n in &self.nodes {
			let nr = Rect::from_center_size(n.pos.to_pos2(), n.size);
			r = Some(r.map_or(nr, |a| a.union(nr)));
		}
		r
	}

	pub(super) fn fit(&mut self, view: Rect) {
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
}
