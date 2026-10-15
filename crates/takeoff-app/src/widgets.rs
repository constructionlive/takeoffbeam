//! Small shared widgets in the app's Fluent style.

use eframe::egui::{self, Align2, Color32, CornerRadius, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::theme::{self, Palette};

/// Square icon button with tooltip; `active` shows the selected state.
pub fn icon_button(ui: &mut Ui, p: &Palette, icon: &str, tip: &str, active: bool) -> Response {
	let size = Vec2::splat(30.0);
	let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
	// The tooltip doubles as its name for screen readers (the icon has none).
	resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tip));
	let painter = ui.painter();
	if active {
		painter.rect_filled(rect, 4.0, if p.dark { Color32::from_rgb(0x3A, 0x22, 0x18) } else { Color32::from_rgb(0xFD, 0xE7, 0xDD) });
		painter.rect_stroke(rect, 4.0, Stroke::new(1.0, p.accent), StrokeKind::Inside);
	} else if resp.hovered() {
		painter.rect_filled(rect, 4.0, p.hover);
	}
	let color = if active { p.accent } else if resp.hovered() { p.text } else { p.text2 };
	painter.text(rect.center(), Align2::CENTER_CENTER, icon, theme::icon_font(16.0), color);
	resp.on_hover_text(tip)
}

/// Text+icon button used in toolbars and panels.
pub fn ghost_button(ui: &mut Ui, p: &Palette, icon: &str, label: &str) -> Response {
	let text = if icon.is_empty() { label.to_string() } else if label.is_empty() { icon.to_string() } else { format!("{icon}  {label}") };
	ui.add(egui::Button::new(RichText::new(text).color(p.text)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).corner_radius(4))
}

/// Primary action: orange fill, label in the theme's on-accent color.
pub fn primary_button(ui: &mut Ui, p: &Palette, label: &str) -> Response {
	ui.add(egui::Button::new(RichText::new(label).color(p.on_accent).font(theme::semibold(13.0))).fill(p.accent_fill).stroke(Stroke::NONE).corner_radius(4).min_size(Vec2::new(0.0, 28.0)))
}

pub fn secondary_button(ui: &mut Ui, p: &Palette, label: &str) -> Response {
	ui.add(egui::Button::new(RichText::new(label).color(p.text)).fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(4).min_size(Vec2::new(0.0, 28.0)))
}

/// Uppercase small section label (`do-section-label`).
pub fn section_label(ui: &mut Ui, p: &Palette, text: &str) {
	ui.label(RichText::new(text.to_uppercase()).font(theme::semibold(10.5)).color(p.muted).extra_letter_spacing(0.6));
}

/// A card: panel surface with border and subtle elevation.
pub fn card<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
	egui::Frame::new()
		.fill(p.card)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(4))
		.inner_margin(egui::Margin::same(10))
		.show(ui, add)
		.inner
}

/// Color swatch dot.
pub fn swatch(ui: &mut Ui, color: Color32, size: f32) -> Response {
	let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
	ui.painter().rect_filled(rect, 3.0, color);
	ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_black_alpha(40)), StrokeKind::Inside);
	resp
}

/// Tab strip for side panels: returns the clicked index.
pub fn tabs(ui: &mut Ui, p: &Palette, items: &[(&str, &str)], selected: &mut usize) {
	ui.horizontal(|ui| {
		ui.spacing_mut().item_spacing.x = 2.0;
		for (i, (icon, label)) in items.iter().enumerate() {
			let active = *selected == i;
			let text = RichText::new(format!("{icon}  {label}")).font(if active { theme::semibold(12.5) } else { theme::medium(12.5) }).color(if active { p.text } else { p.text2 });
			let resp = ui.add(egui::Button::new(text).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::new(0.0, 32.0)));
			if active {
				let r = resp.rect;
				ui.painter().line_segment([r.left_bottom() + Vec2::new(8.0, -1.0), r.right_bottom() + Vec2::new(-8.0, -1.0)], Stroke::new(2.5, p.accent));
			}
			if resp.clicked() {
				*selected = i;
			}
		}
	});
}

/// Muted helper text.
pub fn hint(ui: &mut Ui, p: &Palette, text: &str) -> Response {
	ui.label(RichText::new(text).color(p.muted).size(12.0))
}

/// Empty-state block: icon, title, sentence.
pub fn empty_state(ui: &mut Ui, p: &Palette, icon: &str, title: &str, body: &str) {
	ui.add_space(24.0);
	ui.vertical_centered(|ui| {
		ui.label(RichText::new(icon).font(theme::icon_font(28.0)).color(p.muted));
		ui.add_space(6.0);
		ui.label(RichText::new(title).font(theme::semibold(13.5)).color(p.text));
		ui.add_space(2.0);
		ui.label(RichText::new(body).color(p.text2).size(12.0));
	});
	ui.add_space(24.0);
}

pub fn kind_icon(kind: takeoff_core::CategoryKind) -> &'static str {
	match kind {
		takeoff_core::CategoryKind::Count => crate::icons::HASH,
		takeoff_core::CategoryKind::Length => crate::icons::RULER,
		takeoff_core::CategoryKind::Area => crate::icons::PENTAGON,
	}
}
