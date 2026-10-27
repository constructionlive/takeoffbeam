//! Markup legends on the sheet: drawn from the same layout as exports, so
//! what you see is what prints; picked, moved and resized like markups; and
//! their options in the Properties tab.

use std::collections::HashSet;

use eframe::egui::{self, Align2, Color32, Pos2, Rect, RichText, Shape, Stroke, StrokeKind};
use takeoff_core::legend::{hatch_lines, layout, rows, Swatch};
use takeoff_core::*;

use crate::docstate::OpenDoc;
use crate::geom;
use crate::icons as i;
use crate::theme::{self, hex, Palette};
use crate::widgets;

/// Smallest legend box, in page fractions.
const MIN: f64 = 0.02;

fn with_alpha(c: Color32, a: f32) -> Color32 {
	Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn screen_rect(doc: &OpenDoc, l: &Legend) -> Rect {
	Rect::from_two_pos(doc.to_screen(Point { x: l.x, y: l.y }), doc.to_screen(Point { x: l.x + l.w, y: l.y + l.h }))
}

/// The page's shape (height / width).
fn aspect(doc: &OpenDoc) -> f64 {
	let (w, h) = doc.page_size(doc.page);
	h / w.max(1.0)
}

/// Legends on the page, on top of the takeoff.
pub fn paint(painter: &egui::Painter, doc: &OpenDoc, d: &Document, flash: &HashSet<String>, p: &Palette) {
	let page_h = doc.page_rect().height() as f64;
	let ink = Color32::from_gray(40);
	let to = |q: Point| doc.to_screen(q);
	for l in d.data.legends.iter().filter(|l| l.page == doc.page) {
		let r = screen_rect(doc, l);
		if !r.intersects(doc.canvas_rect) {
			continue;
		}
		let rows = rows(d, l);
		let lay = layout(l, &rows, aspect(doc));
		let mut shapes = vec![Shape::rect_filled(r, 0.0, with_alpha(Color32::WHITE, 0.94)), Shape::rect_stroke(r, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside)];
		for (a, b) in &lay.rules {
			shapes.push(Shape::line_segment([to(*a), to(*b)], Stroke::new(0.8, Color32::from_gray(160))));
		}
		for ((x, y, w, h), k) in &lay.swatches {
			let row = &rows[*k];
			let color = hex(&row.color);
			let sr = Rect::from_two_pos(to(Point { x: *x, y: *y }), to(Point { x: x + w, y: y + h }));
			match row.swatch {
				Swatch::Count => {
					shapes.push(Shape::rect_filled(sr, 1.0, with_alpha(color, 0.18 * row.opacity as f32)));
					shapes.push(Shape::rect_stroke(sr, 1.0, Stroke::new(1.5, color), StrokeKind::Inside));
				}
				Swatch::Length(style) => {
					let pts = [sr.left_center(), sr.right_center()];
					let stroke = Stroke::new(2.0, color);
					match style {
						Some(LineStyle::Dashed) => geom::dashed(&pts, false, stroke, 6.0, 4.0, &mut shapes),
						Some(LineStyle::Dotted) => geom::dashed(&pts, false, stroke, 1.5, 3.0, &mut shapes),
						_ => shapes.push(Shape::line_segment(pts, stroke)),
					}
				}
				Swatch::Area(hk) => {
					shapes.push(Shape::rect_filled(sr, 0.0, with_alpha(color, 0.25 * row.opacity as f32)));
					for (a, b) in hk.map(|hk| hatch_lines((*x, *y, *w, *h), hk, aspect(doc), 6)).unwrap_or_default() {
						shapes.push(Shape::line_segment([to(a), to(b)], Stroke::new(1.0, with_alpha(color, 0.7))));
					}
					shapes.push(Shape::rect_stroke(sr, 0.0, Stroke::new(1.2, color), StrokeKind::Inside));
				}
				Swatch::Markup(_) => shapes.push(Shape::rect_stroke(sr, 0.0, Stroke::new(1.5, with_alpha(color, row.opacity as f32)), StrokeKind::Inside)),
			}
		}
		painter.extend(shapes);
		for t in &lay.texts {
			let px = (t.size * page_h) as f32;
			if px < 3.0 {
				continue;
			}
			let font = if t.bold { theme::semibold(px) } else { theme::medium(px) };
			painter.text(to(t.at), if t.right { Align2::RIGHT_TOP } else { Align2::LEFT_TOP }, &t.text, font, if t.muted { Color32::from_gray(110) } else { ink });
		}
		if doc.selection.contains(&l.id) || flash.contains(&l.id) {
			painter.rect_stroke(r.expand(3.0), 2.0, Stroke::new(1.5, p.selection), StrokeKind::Outside);
			for c in [r.left_top(), r.right_bottom()] {
				painter.rect_filled(Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 1.0, p.selection);
			}
		}
	}
}

/// The topmost legend under the screen point.
pub fn hit(doc: &OpenDoc, data: &TakeoffData, pos: Pos2) -> Option<String> {
	data.legends.iter().rev().filter(|l| l.page == doc.page).find(|l| screen_rect(doc, l).expand(2.0).contains(pos)).map(|l| l.id.clone())
}

/// The resize handle (0 = top-left, 1 = bottom-right) of a legend under the screen point.
pub fn corner_at(doc: &OpenDoc, l: &Legend, pos: Pos2) -> Option<usize> {
	let r = screen_rect(doc, l);
	[r.left_top(), r.right_bottom()].iter().position(|c| (*c - pos).length() <= 7.0)
}

/// Drag a corner to `pt`, keeping the other corner where it is.
pub fn resize(l: &mut Legend, corner: usize, pt: Point) {
	let (x1, y1) = (l.x + l.w, l.y + l.h);
	if corner == 0 {
		l.x = pt.x.clamp(0.0, x1 - MIN);
		l.y = pt.y.clamp(0.0, y1 - MIN);
		l.w = x1 - l.x;
		l.h = y1 - l.y;
	} else {
		l.w = (pt.x.min(1.0) - l.x).max(MIN);
		l.h = (pt.y.min(1.0) - l.y).max(MIN);
	}
	l.meta.modified_at = Some(now_ms());
}

/// A new legend of this page's items in the box a–b.
pub fn add(doc: &OpenDoc, a: Point, b: Point) -> String {
	let (x, y) = (a.x.min(b.x), a.y.min(b.y));
	let (mut w, mut h) = ((a.x - b.x).abs(), (a.y - b.y).abs());
	if w < 0.05 || h < 0.04 {
		// A click or a sliver: a default-size box (4" × 2.5" on paper).
		let (pw, ph) = doc.page_size(doc.page);
		(w, h) = ((288.0 / pw).min(0.5), (180.0 / ph).min(0.5));
	}
	let mut d = doc.doc.lock();
	let mut l = Legend::new(new_id("lg"), doc.page, x.min(1.0 - w), y.min(1.0 - h), w, h);
	l.meta = d.new_meta();
	let id = l.id.clone();
	d.edit("Legend", |t| t.legends.push(l));
	id
}

/// The Legends section of the Takeoff tab: place one, find the ones there are.
pub fn panel(ui: &mut egui::Ui, doc: &mut OpenDoc, canvas: &mut crate::canvas::Canvas, p: &Palette) {
	let list: Vec<(String, u32, String)> = doc.doc.lock().data.legends.iter().map(|l| (l.id.clone(), l.page, l.title().to_string())).collect();
	ui.add_space(10.0);
	ui.horizontal(|ui| {
		widgets::section_label(ui, p, "Legends");
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			let active = canvas.tool == crate::tools::Tool::Legend;
			if ui.selectable_label(active, RichText::new(format!("{}  Place", i::PLUS)).size(12.5)).on_hover_text(crate::tools::Tool::Legend.hint()).clicked() {
				canvas.set_tool(if active { crate::tools::Tool::Select } else { crate::tools::Tool::Legend });
			}
		});
	});
	if list.is_empty() {
		widgets::hint(ui, p, "A box on the sheet listing the items with their colours, counts and totals. It updates as you take off and prints with the PDF.");
		return;
	}
	for (id, page, title) in list {
		let text = RichText::new(format!("{}  {title} · p{page}", i::TABLE)).size(12.5).color(if doc.selection.contains(&id) { p.accent } else { p.text });
		if ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click())).on_hover_text("Select it").clicked() {
			if page != doc.page {
				doc.go_to_page(page);
			}
			doc.selection.clear();
			doc.selection.insert(id);
		}
	}
}

/// The Properties tab for a selected legend. False when `id` isn't one.
pub fn props(ui: &mut egui::Ui, doc: &mut OpenDoc, id: &str, p: &Palette) -> bool {
	let (l, cats, n) = {
		let d = doc.doc.lock();
		let Some(l) = d.data.legends.iter().find(|l| l.id == id).cloned() else { return false };
		let n = rows(&d, &l).len();
		(l, d.data.categories.iter().map(|c| (c.id.clone(), c.name.clone(), c.color.clone())).collect::<Vec<_>>(), n)
	};
	let set = |doc: &mut OpenDoc, label: &str, f: &dyn Fn(&mut Legend)| {
		doc.doc.lock().edit(label, |d| {
			if let Some(x) = d.legends.iter_mut().find(|x| x.id == id) {
				f(x);
				x.meta.modified_at = Some(now_ms());
			}
		});
	};
	ui.horizontal(|ui| {
		ui.label(RichText::new(i::TABLE).font(theme::icon_font(18.0)).color(p.accent));
		ui.vertical(|ui| {
			ui.label(RichText::new("Legend").font(theme::semibold(14.5)));
			ui.label(RichText::new(format!("{n} row{} · updates as you take off", if n == 1 { "" } else { "s" })).size(11.5).color(p.text2));
		});
	});
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Title");
	let mut title = l.title.clone().unwrap_or_default();
	if ui.add(egui::TextEdit::singleline(&mut title).hint_text("Legend").desired_width(f32::INFINITY)).changed() {
		set(doc, "Legend title", &|x| x.title = (!title.trim().is_empty()).then(|| title.clone()));
	}
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Lists");
	let mut scope = if l.scope == LegendScope::Document { LegendScope::Document } else { LegendScope::Page };
	ui.horizontal(|ui| {
		let a = ui.selectable_value(&mut scope, LegendScope::Page, "This page").changed();
		let b = ui.selectable_value(&mut scope, LegendScope::Document, "Every page").changed();
		if a || b {
			set(doc, "Legend scope", &|x| x.scope = scope);
		}
	});
	let mut markups = l.markups;
	if ui.checkbox(&mut markups, "Markups too (by subject)").changed() {
		set(doc, "Legend markups", &|x| x.markups = markups);
	}
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Columns");
	let mut cols = l.columns;
	ui.horizontal(|ui| {
		let a = ui.checkbox(&mut cols.count, "Count").changed();
		let b = ui.checkbox(&mut cols.quantity, "Quantity").changed();
		let c = ui.checkbox(&mut cols.cost, "Cost").changed();
		if a || b || c {
			set(doc, "Legend columns", &|x| x.columns = cols);
		}
	});
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Items");
	let mut all = l.category_ids.is_empty();
	if ui.checkbox(&mut all, "Every item in scope").changed() {
		let ids: Vec<String> = if all { vec![] } else { cats.iter().map(|c| c.0.clone()).collect() };
		set(doc, "Legend items", &|x| x.category_ids = ids.clone());
	}
	if !all {
		for (cid, name, color) in &cats {
			let mut on = l.category_ids.contains(cid);
			ui.horizontal(|ui| {
				ui.add_space(16.0);
				widgets::swatch(ui, hex(color), 10.0);
				if ui.checkbox(&mut on, name).changed() {
					let cid = cid.clone();
					set(doc, "Legend items", &|x| {
						x.category_ids.retain(|c| *c != cid);
						if on {
							x.category_ids.push(cid.clone());
						}
					});
				}
			});
		}
	}
	ui.add_space(8.0);
	if ui.button(RichText::new(format!("{}  Delete legend", i::TRASH_2)).color(p.danger)).clicked() {
		doc.doc.lock().edit("Delete legend", |d| d.legends.retain(|x| x.id != id));
		doc.selection.clear();
	}
	true
}
