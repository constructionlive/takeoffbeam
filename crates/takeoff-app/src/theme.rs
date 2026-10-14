//! Look and feel: Microsoft Fluent-style surfaces (clean panels, subtle
//! shadows, 4 px corners), Inter for text, Lucide for icons, one accent
//! (construction.live orange) used for accents and primary actions only.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Visuals};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
	pub dark: bool,
	pub app_bg: Color32,
	pub panel: Color32,
	pub card: Color32,
	pub hover: Color32,
	pub border: Color32,
	pub text: Color32,
	pub text2: Color32,
	pub muted: Color32,
	pub accent: Color32,
	pub accent_fill: Color32,
	pub accent_fill_hover: Color32,
	pub on_accent: Color32,
	pub accent_soft: Color32,
	pub canvas: Color32,
	pub danger: Color32,
	pub success: Color32,
	pub warning: Color32,
	pub selection: Color32,
}

pub const DARK: Palette = Palette {
	dark: true,
	app_bg: Color32::from_rgb(0x0E, 0x15, 0x21),
	panel: Color32::from_rgb(0x13, 0x1C, 0x2B),
	card: Color32::from_rgb(0x19, 0x24, 0x36),
	hover: Color32::from_rgb(0x20, 0x2D, 0x42),
	border: Color32::from_rgb(0x26, 0x34, 0x4A),
	text: Color32::from_rgb(0xE8, 0xEE, 0xF5),
	text2: Color32::from_rgb(0xA3, 0xB0, 0xC2),
	muted: Color32::from_rgb(0x6C, 0x7A, 0x8F),
	accent: Color32::from_rgb(0xFF, 0x6B, 0x35),
	accent_fill: Color32::from_rgb(0xFF, 0x6B, 0x35),
	accent_fill_hover: Color32::from_rgb(0xFF, 0x80, 0x52),
	on_accent: Color32::from_rgb(0x0E, 0x15, 0x21),
	accent_soft: Color32::from_rgba_premultiplied(0x40, 0x1B, 0x0D, 0x40),
	canvas: Color32::from_rgb(0x0A, 0x10, 0x19),
	danger: Color32::from_rgb(0xF8, 0x71, 0x71),
	success: Color32::from_rgb(0x34, 0xD3, 0x99),
	warning: Color32::from_rgb(0xFB, 0xBF, 0x24),
	selection: Color32::from_rgb(0x4C, 0xC2, 0xFF),
};

pub const LIGHT: Palette = Palette {
	dark: false,
	app_bg: Color32::from_rgb(0xF3, 0xF5, 0xF8),
	panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
	card: Color32::from_rgb(0xFA, 0xFB, 0xFC),
	hover: Color32::from_rgb(0xEE, 0xF1, 0xF5),
	border: Color32::from_rgb(0xE0, 0xE4, 0xEA),
	text: Color32::from_rgb(0x1A, 0x23, 0x32),
	text2: Color32::from_rgb(0x56, 0x62, 0x74),
	muted: Color32::from_rgb(0x8B, 0x96, 0xA6),
	accent: Color32::from_rgb(0xE8, 0x53, 0x1A),
	accent_fill: Color32::from_rgb(0xE8, 0x53, 0x1A),
	accent_fill_hover: Color32::from_rgb(0xD1, 0x47, 0x12),
	on_accent: Color32::WHITE,
	accent_soft: Color32::from_rgba_premultiplied(0x2E, 0x10, 0x05, 0x2E),
	canvas: Color32::from_rgb(0xE4, 0xE8, 0xEE),
	danger: Color32::from_rgb(0xD9, 0x34, 0x34),
	success: Color32::from_rgb(0x10, 0x9B, 0x6C),
	warning: Color32::from_rgb(0xC7, 0x85, 0x0A),
	selection: Color32::from_rgb(0x1F, 0x7A, 0xE0),
};

pub fn palette(ctx: &egui::Context) -> Palette {
	if ctx.global_style().visuals.dark_mode { DARK } else { LIGHT }
}

pub fn install_fonts(ctx: &egui::Context) {
	let mut fonts = FontDefinitions::default();
	let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
		fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
	};
	add(&mut fonts, "inter", include_bytes!("../../../assets/fonts/Inter-Regular.ttf"));
	add(&mut fonts, "inter-medium", include_bytes!("../../../assets/fonts/Inter-Medium.ttf"));
	add(&mut fonts, "inter-semibold", include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"));
	add(&mut fonts, "inter-bold", include_bytes!("../../../assets/fonts/Inter-Bold.ttf"));
	add(&mut fonts, "lucide", include_bytes!("../../../assets/fonts/lucide.ttf"));
	// Lucide first: Inter also has glyphs in the private-use area the icons
	// live in, and the first font that has a glyph wins.
	let prop = fonts.families.entry(FontFamily::Proportional).or_default();
	prop.insert(0, "inter".into());
	prop.insert(0, "lucide".into());
	for (fam, font) in [("medium", "inter-medium"), ("semibold", "inter-semibold"), ("bold", "inter-bold")] {
		let mut list = vec!["lucide".to_string(), font.to_string(), "inter".to_string()];
		list.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default().into_iter().skip(2));
		fonts.families.insert(FontFamily::Name(fam.into()), list);
	}
	if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
		mono.insert(0, "lucide".into());
	}
	fonts.families.insert(FontFamily::Name("icons".into()), vec!["lucide".into(), "inter".into()]);
	ctx.set_fonts(fonts);
}

pub fn icon_font(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name("icons".into()))
}
pub fn semibold(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn medium(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn bold(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn apply(ctx: &egui::Context, dark: bool) {
	let p = if dark { DARK } else { LIGHT };
	let mut v = if dark { Visuals::dark() } else { Visuals::light() };
	v.panel_fill = p.panel;
	v.window_fill = p.panel;
	v.extreme_bg_color = if dark { Color32::from_rgb(0x0C, 0x13, 0x1E) } else { Color32::WHITE };
	v.faint_bg_color = p.card;
	v.code_bg_color = p.card;
	v.window_stroke = Stroke::new(1.0, p.border);
	v.window_corner_radius = CornerRadius::same(6);
	v.menu_corner_radius = CornerRadius::same(6);
	v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 38 }) };
	v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if dark { 100 } else { 30 }) };
	v.selection.bg_fill = if dark { Color32::from_rgb(0x5A, 0x2A, 0x16) } else { Color32::from_rgb(0xFC, 0xDD, 0xD0) };
	v.selection.stroke = Stroke::new(1.0, p.accent);
	v.hyperlink_color = p.accent;
	v.override_text_color = None;
	let r4 = CornerRadius::same(4);
	for (w, fill, stroke) in [
		(&mut v.widgets.noninteractive, p.panel, p.border),
		(&mut v.widgets.inactive, p.card, p.border),
		(&mut v.widgets.hovered, p.hover, p.border),
		(&mut v.widgets.active, p.hover, p.accent),
		(&mut v.widgets.open, p.hover, p.border),
	] {
		w.bg_fill = fill;
		w.weak_bg_fill = fill;
		w.bg_stroke = Stroke::new(1.0, stroke);
		w.corner_radius = r4;
		w.expansion = 0.0;
	}
	v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text2);
	v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
	v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
	v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
	v.widgets.open.fg_stroke = Stroke::new(1.0, p.text);
	v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
	v.striped = true;
	ctx.set_visuals(v);

	ctx.global_style_mut(|s| {
		s.spacing.item_spacing = egui::vec2(8.0, 6.0);
		s.spacing.button_padding = egui::vec2(10.0, 5.0);
		s.spacing.interact_size.y = 26.0;
		s.spacing.menu_margin = Margin::same(6);
		s.spacing.window_margin = Margin::same(14);
		s.spacing.scroll.bar_width = 8.0;
		s.spacing.scroll.floating = true;
		s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
		s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
		s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
		s.text_styles.insert(TextStyle::Heading, semibold(16.0));
		s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.0));
		s.interaction.tooltip_delay = 0.35;
	});
}

/// Hex "#RRGGBB" → Color32.
pub fn hex(s: &str) -> Color32 {
	let h = s.trim_start_matches('#');
	let v = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
	if h.len() == 3 {
		let c = |i: usize| u8::from_str_radix(&h[i..i + 1].repeat(2), 16).unwrap_or(255);
		return Color32::from_rgb(c(0), c(1), c(2));
	}
	Color32::from_rgb(v(0), v(2), v(4))
}

pub fn to_hex(c: Color32) -> String {
	format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}
