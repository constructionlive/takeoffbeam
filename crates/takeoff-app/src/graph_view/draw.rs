//! Drawing the graph: the canvas with its nodes, edges and legend, the waiting
//! state, and the build card and dialog.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn graph_canvas(ui: &mut Ui, doc: &mut OpenDoc, st: &mut GraphState, map: &DocMap, p: &Palette, view: Rect, wheel_zooms: bool, action: &mut Option<GraphAction>) {
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
pub(super) fn edge_point(rect: Rect, toward: Pos2) -> Pos2 {
	let c = rect.center();
	let d = toward - c;
	if d.length_sq() < 1e-6 {
		return c;
	}
	let sx = if d.x.abs() > 1e-6 { rect.width() / 2.0 / d.x.abs() } else { f32::INFINITY };
	let sy = if d.y.abs() > 1e-6 { rect.height() / 2.0 / d.y.abs() } else { f32::INFINITY };
	c + d * sx.min(sy).min(1.0)
}

pub(super) fn arrow_head(painter: &egui::Painter, from: Pos2, to: Pos2, size: f32, c: Color32) {
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
pub(super) fn draw_node(painter: &egui::Painter, doc: &mut OpenDoc, g: &GNode, n: NodeRef, r: Rect, p: &Palette, alpha: f32, selected: bool, hovered: bool) {
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
pub(super) fn unmapped_tint(c: Color32, n: NodeRef) -> Color32 {
	match n {
		NodeRef::Sheet(s) if !s.mapped => c.gamma_multiply(0.4),
		_ => c,
	}
}

pub(super) fn one_line(painter: &egui::Painter, text: &str, font: egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
	let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
	job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(8.0));
	painter.layout_job(job)
}

/// The link colours, in a corner (bottom right on the board, where lanes start at the left).
pub(super) fn legend(ui: &mut Ui, p: &Palette, graph: Rect, right: bool) {
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

pub(super) fn waiting_state(ui: &mut Ui, chat: &MapChat, p: &Palette, rect: Rect, action: &mut Option<GraphAction>) {
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
pub(super) fn build_card(ui: &mut Ui, doc: &mut OpenDoc, st: &mut GraphState, p: &Palette, chat: &MapChat, in_dialog: bool, action: &mut Option<GraphAction>) {
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

pub(super) fn build_dialog(ctx: &egui::Context, doc: &mut OpenDoc, st: &mut GraphState, p: &Palette, chat: &MapChat, action: &mut Option<GraphAction>) {
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
