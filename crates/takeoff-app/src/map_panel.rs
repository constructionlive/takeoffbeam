//! The Map tab: the document map from where you are. In Page view it shows
//! the sheet on screen (or the area you clicked on it); in Graph view, the
//! selected card. Each entry leads somewhere: what is on it, what it points
//! to, what points at it. Moving leaves a trail, with Back / Forward and a
//! breadcrumb, and Back restores the exact view you left.

use eframe::egui::{self, Color32, CursorIcon, RichText, Sense, Ui, Vec2};
use takeoff_core::docmap::{self, NodeRef, Part, PartKind, Region};

use crate::ai_panel::MapChat;
use crate::docstate::{OpenDoc, ViewMode};
use crate::graph_view::{crop_key, discipline_color, kind_color, link_color, link_sub, CROP_PX};
use crate::icons as i;
use crate::theme::{self, Palette};
use crate::widgets;

pub enum MapAction {
	/// Move to a node (onto the trail).
	Go(String),
	/// Jump along the trail to this index.
	Step(usize),
	/// Open a node on its sheet (from Graph view).
	OpenOnSheet(String),
	/// Show a node in Graph view.
	ShowInGraph(String),
	Ask { page: u32, region: Option<Region>, title: String },
	/// No map yet: go and start one.
	StartMapping,
	OpenChat,
}

/// A picture of a node: the sheet's thumbnail or the area cut from it, with its size in points.
fn picture(doc: &mut OpenDoc, map: &docmap::DocMap, id: &str) -> Option<(egui::TextureHandle, (f64, f64))> {
	match map.node(id)? {
		NodeRef::Sheet(s) => {
			let size = doc.page_size(s.page);
			doc.thumb(s.page).map(|t| (t, size))
		}
		NodeRef::Part(s, part) => {
			let reg = part.region?;
			let (pw, ph) = doc.page_size(s.page);
			doc.crop(&crop_key(&part.id, reg), s.page, reg, CROP_PX).map(|t| (t, (reg.w * pw, reg.h * ph)))
		}
	}
}

fn node_color(n: NodeRef) -> Color32 {
	match n {
		NodeRef::Sheet(s) => discipline_color(s.discipline.as_deref()),
		NodeRef::Part(_, p) => kind_color(p.kind),
	}
}

pub fn ui(ui: &mut Ui, doc: &mut OpenDoc, p: &Palette, chat: &MapChat) -> Option<MapAction> {
	let mut action = None;
	let Some(map) = doc.map.clone().filter(|m| !m.sheets.is_empty()) else {
		empty(ui, p, chat, &mut action);
		return action;
	};
	if chat.running_here {
		ui.horizontal(|ui| {
			ui.add(egui::Spinner::new().size(12.0));
			ui.label(RichText::new("The AI is mapping the set").size(12.0).color(p.text2));
			if ui.link(RichText::new("Open chat").size(12.0)).clicked() {
				action = Some(MapAction::OpenChat);
			}
		});
		ui.add_space(2.0);
	}
	trail_bar(ui, doc, p, &mut action);
	ui.add_space(4.0);
	let Some(id) = doc.map_focus() else {
		widgets::hint(ui, p, &format!("Page {} isn't in the map yet. Pick a sheet in Graph view, or continue mapping from there.", doc.page));
		return action;
	};
	let Some(node) = map.node(&id) else { return action };
	let graph = doc.view_mode == ViewMode::Graph;
	egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
		ui.set_width(ui.available_width());
		// What it is.
		ui.horizontal(|ui| {
			let kind = match node {
				NodeRef::Sheet(s) => s.discipline.clone().unwrap_or_else(|| "Sheet".into()),
				NodeRef::Part(_, part) => part.kind.label().to_string(),
			};
			ui.label(RichText::new(format!("●  {kind}")).size(11.5).color(node_color(node)));
			ui.label(RichText::new(format!("page {}", node.page())).size(11.5).color(p.muted));
			if let NodeRef::Part(s, part) = node {
				let host = part.parent.as_deref().map(|q| doc.crumb(q)).unwrap_or_else(|| s.number.clone().unwrap_or_default());
				if !host.is_empty() {
					ui.label(RichText::new(format!("on {host}")).size(11.5).color(p.muted));
				}
			}
		});
		ui.label(RichText::new(node.title()).font(theme::semibold(15.0)).color(p.text));
		ui.add_space(6.0);
		if let Some((t, (tw, th))) = picture(doc, &map, &id) {
			let w = ui.available_width();
			let h = (w * (th / tw.max(1e-6)) as f32).clamp(40.0, 240.0);
			let size = Vec2::new((h * (tw / th.max(1e-6)) as f32).min(w), h);
			let tip = if graph { "Open it on the sheet" } else { "Zoom to it on the sheet" };
			let r = ui.add(egui::Image::new((t.id(), size)).bg_fill(Color32::WHITE).corner_radius(3).sense(Sense::click())).on_hover_text(tip);
			if r.clicked() {
				action = Some(if graph { MapAction::OpenOnSheet(id.clone()) } else { MapAction::Go(id.clone()) });
			}
			if r.hovered() {
				ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
			}
		}
		ui.add_space(8.0);
		ui.horizontal_wrapped(|ui| {
			if graph {
				if widgets::primary_button(ui, p, &format!("{}  Open on sheet", i::FILE_TEXT)).clicked() {
					action = Some(MapAction::OpenOnSheet(id.clone()));
				}
			} else {
				if node.region().is_some() && widgets::secondary_button(ui, p, &format!("{}  Zoom to it", i::ZOOM_IN)).clicked() {
					action = Some(MapAction::Go(id.clone()));
				}
				if widgets::secondary_button(ui, p, &format!("{}  In graph", i::WAYPOINTS)).on_hover_text("Show it in Graph view (⌘G)").clicked() {
					action = Some(MapAction::ShowInGraph(id.clone()));
				}
			}
			if widgets::secondary_button(ui, p, &format!("{}  Ask AI", i::SPARKLES)).on_hover_text("Attach this area to a message in the AI tab").clicked() {
				action = Some(MapAction::Ask { page: node.page(), region: node.region(), title: node.title() });
			}
		});
		if !node.summary().is_empty() {
			ui.add_space(8.0);
			ui.label(RichText::new(node.summary()).size(12.5).color(p.text2));
		}
		if let NodeRef::Sheet(s) = node {
			if !s.mapped {
				ui.add_space(4.0);
				ui.label(RichText::new("Indexed in the survey; not mapped closely yet.").size(12.0).color(p.muted));
			}
		}
		// What is on it / inside it, in kind order.
		let (mut inner, heading): (Vec<&Part>, &str) = match node {
			NodeRef::Sheet(s) => (s.parts.iter().filter(|x| x.kind != PartKind::TitleBlock && x.parent.is_none()).collect(), "On this sheet"),
			NodeRef::Part(s, part) => (s.parts.iter().filter(|x| x.parent.as_deref() == Some(part.id.as_str())).collect(), "Inside it"),
		};
		inner.sort_by_key(|x| PartKind::ALL.iter().position(|k| *k == x.kind));
		if !inner.is_empty() {
			section(ui, p, heading, inner.len());
			for part in inner {
				let sub = if part.summary.is_empty() { part.kind.label().to_string() } else { part.summary.clone() };
				if node_row(ui, doc, &map, p, &part.id, kind_color(part.kind), &sub) {
					action = Some(MapAction::Go(part.id.clone()));
				}
			}
		}
		let out: Vec<_> = map.links_from(&id).collect();
		if !out.is_empty() {
			section(ui, p, "Points to", out.len());
			for l in out {
				if node_row(ui, doc, &map, p, &l.to, link_color(l.kind), &link_sub(l)) {
					action = Some(MapAction::Go(l.to.clone()));
				}
			}
		}
		let inc: Vec<_> = map.links_to(&id).collect();
		if !inc.is_empty() {
			section(ui, p, "Referenced from", inc.len());
			for l in inc {
				if node_row(ui, doc, &map, p, &l.from, link_color(l.kind), &link_sub(l)) {
					action = Some(MapAction::Go(l.from.clone()));
				}
			}
		}
		let page = docmap::page_of(&id);
		let loose: Vec<&str> = map.unresolved.iter().filter(|u| u.from == id || (!id.contains('.') && docmap::page_of(&u.from) == page)).map(|u| u.text.as_str()).collect();
		if !loose.is_empty() {
			section(ui, p, "Outside this set", loose.len());
			for t in loose {
				ui.label(RichText::new(format!("{}  {t}", i::EXTERNAL_LINK)).size(12.0).color(p.muted));
			}
		}
	});
	action
}

fn section(ui: &mut Ui, p: &Palette, title: &str, n: usize) {
	ui.add_space(10.0);
	widgets::section_label(ui, p, &format!("{title} ({n})"));
}

/// Back / Forward and the breadcrumb of where you have been.
fn trail_bar(ui: &mut Ui, doc: &mut OpenDoc, p: &Palette, action: &mut Option<MapAction>) {
	ui.horizontal(|ui| {
		ui.spacing_mut().item_spacing.x = 2.0;
		let arrow = |icon: &str| egui::Button::new(RichText::new(icon).font(theme::icon_font(13.0)).color(p.text)).min_size(Vec2::new(24.0, 22.0)).corner_radius(4);
		let back = ui.add_enabled(doc.can_go_back(), arrow(i::CHEVRON_LEFT)).on_hover_text("Back (⌥←)");
		if back.clicked() {
			*action = Some(MapAction::Step(doc.nav.pos - 1));
		}
		let fwd = ui.add_enabled(doc.can_go_forward(), arrow(i::CHEVRON_RIGHT)).on_hover_text("Forward (⌥→)");
		if fwd.clicked() {
			*action = Some(MapAction::Step(doc.nav.pos + 1));
		}
		ui.add_space(4.0);
		let current = doc.map_focus();
		// The trail up to where we are; the stop we're at reads as the current node.
		let mut crumbs: Vec<(Option<usize>, String)> = doc.nav.trail.iter().enumerate().take(doc.nav.pos).map(|(k, s)| (Some(k), s.node.clone())).collect();
		if let Some(c) = &current {
			crumbs.push((None, c.clone()));
		}
		let skip = crumbs.len().saturating_sub(4);
		if skip > 0 {
			ui.label(RichText::new("…").size(12.0).color(p.muted));
			ui.label(RichText::new("›").size(12.0).color(p.muted));
		}
		let n = crumbs.len();
		for (k, (step, id)) in crumbs.into_iter().enumerate().skip(skip) {
			let name = doc.crumb(&id);
			let last = k + 1 == n;
			if last {
				ui.add(egui::Label::new(RichText::new(name).size(12.0).strong().color(p.text)).truncate());
			} else {
				if ui.add(egui::Button::new(RichText::new(name).size(12.0).color(p.text2)).frame(false)).clicked() {
					if let Some(s) = step {
						*action = Some(MapAction::Step(s));
					}
				}
				ui.label(RichText::new("›").size(12.0).color(p.muted));
			}
		}
	});
}

/// One entry leading to node `id`: a small picture, its name and a line under it.
fn node_row(ui: &mut Ui, doc: &mut OpenDoc, map: &docmap::DocMap, p: &Palette, id: &str, color: Color32, sub: &str) -> bool {
	let name = map.node(id).map(|n| n.title()).unwrap_or_else(|| id.to_string());
	let pic = picture(doc, map, id);
	let resp = egui::Frame::new().corner_radius(4).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
		ui.set_width(ui.available_width());
		ui.horizontal(|ui| {
			let (r, _) = ui.allocate_exact_size(Vec2::new(44.0, 34.0), Sense::hover());
			ui.painter().rect_filled(r, 2.0, p.hover);
			if let Some((t, (tw, th))) = pic {
				let aspect = (tw / th.max(1e-6)) as f32;
				let fit = if r.width() / r.height() > aspect { Vec2::new(r.height() * aspect, r.height()) } else { Vec2::new(r.width(), r.width() / aspect) };
				let ir = egui::Rect::from_center_size(r.center(), fit);
				ui.painter().rect_filled(ir, 1.0, Color32::WHITE);
				ui.painter().image(t.id(), ir, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
			}
			ui.painter().rect_filled(egui::Rect::from_min_size(r.left_top(), Vec2::new(3.0, r.height())), 1.0, color);
			ui.vertical(|ui| {
				ui.add(egui::Label::new(RichText::new(name).size(12.5).color(p.text)).truncate());
				if !sub.is_empty() {
					ui.add(egui::Label::new(RichText::new(sub).size(11.0).color(p.muted)).truncate());
				}
			});
		});
	});
	let r = resp.response.interact(Sense::click());
	if r.hovered() {
		ui.painter().rect_filled(r.rect, 4.0, p.hover.gamma_multiply(0.5));
		ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
	}
	r.clicked()
}

fn empty(ui: &mut Ui, p: &Palette, chat: &MapChat, action: &mut Option<MapAction>) {
	ui.add_space(24.0);
	ui.vertical_centered(|ui| {
		ui.label(RichText::new(i::WAYPOINTS).font(theme::icon_font(26.0)).color(p.accent));
		ui.add_space(6.0);
		ui.label(RichText::new(if chat.running_here { "The AI is mapping the set…" } else { "No map of this set yet" }).font(theme::semibold(14.0)));
		ui.add_space(4.0);
		ui.label(RichText::new("The map shows what each sheet holds and how its details, sections, schedules and notes connect. Move along those connections here, from whatever you're looking at.").size(12.0).color(p.text2));
		ui.add_space(10.0);
		if chat.running_here {
			if widgets::secondary_button(ui, p, "Open chat").clicked() {
				*action = Some(MapAction::OpenChat);
			}
		} else if widgets::primary_button(ui, p, &format!("{}  Map this set", i::SPARKLES)).clicked() {
			*action = Some(MapAction::StartMapping);
		}
	});
}
