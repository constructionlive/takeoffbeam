//! The Graph view's top bar and its filters: disciplines, kinds and search.

use super::*;

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
pub(super) fn all_keys(groups: &[DisciplineGroup]) -> std::collections::BTreeSet<String> {
	groups.iter().flat_map(|g| g.keys().cloned()).collect()
}

/// What the discipline filter shows, in a few words: "All disciplines", "Civil", "Civil › Track", "3 disciplines".
pub(super) fn discipline_summary(groups: &[DisciplineGroup], f: &MapFilter) -> String {
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

pub(super) fn discipline_list(ui: &mut Ui, p: &Palette, groups: &[DisciplineGroup], all: &std::collections::BTreeSet<String>, f: &mut MapFilter) -> bool {
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
pub(super) fn row(ui: &mut Ui, p: &Palette, color: Option<Color32>, name: &str, count: Option<usize>, state: Option<bool>, indent: f32) -> (bool, bool) {
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
pub(super) fn kinds_menu(ui: &mut Ui, p: &Palette, f: &mut MapFilter) {
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
pub(super) fn top_bar(ui: &mut Ui, doc: &mut OpenDoc, st: &mut GraphState, map: &DocMap, p: &Palette, chat: &MapChat, bar: Rect, action: &mut Option<GraphAction>) {
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

pub(super) fn first_match(map: &DocMap, q: &str, st: &GraphState) -> Option<String> {
	let words: Vec<String> = q.to_lowercase().split_whitespace().map(str::to_string).collect();
	if words.is_empty() {
		return None;
	}
	map.node_ids().into_iter().filter(|id| st.index.contains_key(id)).find(|id| map.node(id).is_some_and(|n| matches(n, &words)))
}

pub(super) fn matches(n: NodeRef, words: &[String]) -> bool {
	let hay = format!("{} {}", n.title(), n.summary()).to_lowercase();
	words.iter().all(|w| hay.contains(w))
}
