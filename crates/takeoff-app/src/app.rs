//! The application window: document tabs, toolbar, panels, status bar,
//! dialogs, keyboard shortcuts and the start screen.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, RichText, Stroke, Vec2};
use takeoff_core::units::{self, ScalePreset};
use takeoff_core::*;
use takeoff_pdf::tools::SplitMode;

use crate::ai_panel::AiPanel;
use crate::canvas::{Canvas, CanvasRequest};
use crate::docstate::{OpenDoc, ViewMode};
use crate::graph_view::GraphAction;
use crate::icons as i;
use crate::left_panel::{LeftPanel, PageAction};
use crate::markups::{ListAction, MarkupsList};
use crate::right_panel::{describe_scale, RightPanel};
use crate::settings::{Settings, ThemeChoice};
use crate::theme::{self, Palette};
use crate::tools::Tool;
use crate::updates::{self, DownloadState, Updater};
use crate::widgets;

mod actions;
mod chrome;
mod dialogs;
mod panels;
mod sets;

enum Dialog {
	Calibrate { page: u32, p1: Point, p2: Point, text: String, unit: String, error: Option<String>, all_pages: bool },
	Viewport { page: u32, rect: (f64, f64, f64, f64), name: String, preset: usize },
	Depth { id: String, value: String },
	Split { mode: usize, every: u32, ranges: String, error: Option<String> },
	Merge { files: Vec<PathBuf>, error: Option<String> },
	Settings,
	Shortcuts,
	About,
	ConfirmPdfEdit { action: PageAction },
	DeleteMap,
}

pub struct TakeoffApp {
	pub settings: Settings,
	docs: Vec<OpenDoc>,
	active: usize,
	canvas: Canvas,
	left: LeftPanel,
	right: RightPanel,
	markups: MarkupsList,
	ai: AiPanel,
	dialog: Option<Dialog>,
	toast: Option<(String, Instant, bool)>,
	theme_applied: Option<bool>,
	page_input: String,
	pdf_edits_confirmed: bool,
	updater: Updater,
	notes_open: bool,
	/// A crash report from last time and its logs folder, offered until sent or dismissed.
	crash: Option<(String, PathBuf)>,
	/// The open set of drawings, if any.
	set: Option<crate::set_panel::OpenSet>,
	/// A slip-sheet being reviewed or applied.
	slip: Option<crate::slipsheet::SlipReview>,
}

impl TakeoffApp {
	pub fn new(ctx: &egui::Context, settings: Settings) -> Self {
		theme::install_fonts(ctx);
		egui_extras::install_image_loaders(ctx);
		let mut app = Self {
			settings,
			docs: vec![],
			active: 0,
			canvas: Canvas::default(),
			left: LeftPanel::default(),
			right: RightPanel::default(),
			markups: MarkupsList::default(),
			ai: AiPanel::new(),
			dialog: None,
			toast: None,
			theme_applied: None,
			page_input: String::new(),
			pdf_edits_confirmed: false,
			updater: Updater::new(),
			notes_open: false,
			crash: None,
			set: None,
			slip: None,
		};
		app.apply_theme(ctx);
		app.canvas.show_badges = app.settings.show_badges;
		app
	}

	fn apply_theme(&mut self, ctx: &egui::Context) {
		let dark = match self.settings.theme {
			ThemeChoice::Dark => true,
			ThemeChoice::Light => false,
			ThemeChoice::System => ctx.system_theme().map(|t| t == egui::Theme::Dark).unwrap_or(true),
		};
		if self.theme_applied != Some(dark) {
			theme::apply(ctx, dark);
			self.theme_applied = Some(dark);
		}
	}

	pub fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
		if let Some(idx) = self.docs.iter().position(|d| d.path == path) {
			self.active = idx;
			return;
		}
		match OpenDoc::open(path.clone(), &self.settings.author, ctx) {
			Ok(mut d) => {
				d.set_layout(self.settings.page_layout);
				self.docs.push(d);
				self.active = self.docs.len() - 1;
				self.settings.push_recent(path);
				self.settings.save();
				self.canvas.set_tool(Tool::Select);
				self.canvas.active_category = None;
			}
			Err(e) => self.toast(format!("Couldn't open {}: {e}", path.display()), true),
		}
	}

	/// Offer last session's crash report (`crash::pending(dir)`) in a corner notice.
	pub fn offer_crash_report(&mut self, report: String, dir: PathBuf) {
		self.crash = Some((report, dir));
	}

	fn toast(&mut self, msg: impl Into<String>, error: bool) {
		self.toast = Some((msg.into(), Instant::now(), error));
	}

	fn doc(&mut self) -> Option<&mut OpenDoc> {
		self.docs.get_mut(self.active)
	}

	fn close_active(&mut self) {
		if self.active < self.docs.len() {
			let mut d = self.docs.remove(self.active);
			if d.is_dirty() {
				d.save_now();
			}
			self.active = self.active.min(self.docs.len().saturating_sub(1));
		}
	}

	fn open_dialog(&mut self, ctx: &egui::Context) {
		if let Some(files) = rfd::FileDialog::new().add_filter("PDF drawings", &["pdf"]).set_title("Open drawings").pick_files() {
			for f in files {
				self.open(f, ctx);
			}
		}
	}

	// ── Frame ───────────────────────────────────────────────────────────

	pub fn frame(&mut self, ui: &mut egui::Ui) {
		let ctx = ui.ctx().clone();
		self.apply_theme(&ctx);
		let p = theme::palette(&ctx);

		// Dropped files, and files Finder sent (double-click, Open With).
		let mut dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
		dropped.extend(crate::open_files::take());
		let mut images = Vec::new();
		for f in dropped {
			if f.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
				self.open(f, &ctx);
			} else if takeoff_core::set::is_set_file(&f) {
				self.open_set(f, &ctx);
			} else if crate::ai_panel::is_image_file(&f) {
				images.push(f);
			}
		}
		// Pictures go into the AI chat's next message.
		if !images.is_empty() {
			self.ai.attach_image_files(&images);
		}

		for d in &mut self.docs {
			d.poll(&ctx);
			d.autosave(&ctx);
			d.update_compare(&ctx);
		}
		self.poll_sets(&ctx);
		let active = self.active;
		self.ai.others = self.docs.iter().filter(|d| d.loaded).map(|d| takeoff_agent::tools::OtherDrawing { path: d.path.clone(), doc: d.doc.clone(), page: d.page }).collect();
		let doc = self.docs.get_mut(active);
		self.ai.poll(&ctx, &mut self.canvas, doc, &self.settings);
		self.follow_agent();
		self.shortcuts(&ctx);
		self.handle_canvas_requests();
		self.updater.tick(&ctx);
		if self.updater.manual && !self.updater.checking() && self.updater.checked_once() {
			self.updater.manual = false;
			match (self.updater.available(""), self.updater.error()) {
				(Some(r), _) => {
					self.updater.dismissed = false;
					self.toast(format!("Takeoff Desk {} is available", r.version), false);
				}
				(None, Some(e)) => self.toast(format!("Couldn't check for updates: {}", e.chars().take(120).collect::<String>()), true),
				(None, None) => self.toast(format!("You're up to date (v{})", env!("CARGO_PKG_VERSION")), false),
			}
		}

		self.top_bar(ui, &p);
		self.update_banner(ui, &p);
		if !self.docs.is_empty() {
			self.status_bar(ui, &p);
		}
		if self.docs.is_empty() {
			egui::CentralPanel::default().frame(egui::Frame::new().fill(p.app_bg)).show(ui, |ui| self.start_screen(ui, &p));
		} else {
			self.panels(ui, &p);
		}
		self.dialogs(&ctx, &p);
		self.set_windows(&ctx, &p);
		self.ai.models_window(&ctx, &mut self.settings, &p);
		self.crash_notice(&ctx, &p);
		self.toasts(&ctx, &p);
	}

	// ── Canvas requests ─────────────────────────────────────────────────

	fn handle_canvas_requests(&mut self) {
		for r in std::mem::take(&mut self.canvas.requests) {
			match r {
				CanvasRequest::Calibrate { page, p1, p2 } => {
					let unit = self.docs.get(self.active).and_then(|d| d.doc.lock().data.scale_for(page).map(|s| s.unit.clone())).unwrap_or_else(|| "ft".into());
					self.dialog = Some(Dialog::Calibrate { page, p1, p2, text: String::new(), unit, error: None, all_pages: false });
				}
				CanvasRequest::Viewport { page, rect } => {
					let n = self.docs.get(self.active).map(|d| d.doc.lock().data.viewports.len() + 1).unwrap_or(1);
					self.dialog = Some(Dialog::Viewport { page, rect, name: format!("Detail {n}"), preset: 5 });
				}
				CanvasRequest::Depth { id } => self.dialog = Some(Dialog::Depth { id, value: String::new() }),
				CanvasRequest::Status(s) => self.toast(s, false),
				CanvasRequest::ShowProperties => {
					self.settings.right_panel = true;
					self.right.tab = 1;
				}
			}
		}
	}

	// ── Test / automation hooks ─────────────────────────────────────────

	#[doc(hidden)]
	pub fn debug_doc(&mut self) -> Option<&mut OpenDoc> {
		self.docs.get_mut(self.active)
	}
	#[doc(hidden)]
	/// As if the AI just looked at `region` of `page` in `path` (Follow).
	pub fn debug_agent_looked(&mut self, path: PathBuf, page: u32, region: (f64, f64, f64, f64)) {
		self.ai.follow_request = Some((path, page, region));
	}

	pub fn debug_active_path(&self) -> Option<PathBuf> {
		self.docs.get(self.active).map(|d| d.path.clone())
	}

	pub fn debug_canvas(&mut self) -> &mut Canvas {
		&mut self.canvas
	}
	#[doc(hidden)]
	pub fn debug_right_tab(&mut self, tab: usize) {
		self.settings.right_panel = true;
		self.right.tab = tab;
	}
	#[doc(hidden)]
	pub fn debug_left_tab(&mut self, tab: usize) {
		self.settings.left_panel = true;
		self.left.tab = tab;
	}
	#[doc(hidden)]
	pub fn debug_session(&mut self, path: &std::path::Path) -> std::sync::Arc<parking_lot::Mutex<takeoff_agent::ChatSession>> {
		self.ai.debug_session(path)
	}

	#[doc(hidden)]
	pub fn debug_close_models(&mut self) {
		self.ai.models_open = false;
	}

	#[doc(hidden)]
	pub fn debug_open_models(&mut self, tab: usize) {
		self.ai.debug_open(tab);
	}

	#[doc(hidden)]
	pub fn debug_attach_images(&mut self, paths: &[PathBuf]) {
		self.ai.attach_image_files(paths);
	}

	pub fn on_exit(&mut self) {
		for d in &mut self.docs {
			if d.is_dirty() {
				d.save_now();
			}
		}
		if let Some(mut s) = self.ai.server.take() {
			s.stop();
		}
		self.settings.save();
	}
}

/// View → Page layout: (mode, icon, name, shortcut).
const LAYOUTS: [(crate::layout::PageLayout, &str, &str, &str); 4] = [
	(crate::layout::PageLayout::SINGLE, i::FILE, "Single page", "⌘4"),
	(crate::layout::PageLayout::CONTINUOUS, i::GALLERY_VERTICAL, "Continuous", "⌘5"),
	(crate::layout::PageLayout::SPREADS, i::BOOK_OPEN, "Two pages", "⌘6"),
	(crate::layout::PageLayout::SPREADS_CONTINUOUS, i::LAYOUT_GRID, "Two pages, continuous", "⌘7"),
];

fn separator(ui: &mut egui::Ui, p: &Palette) {
	ui.add_space(4.0);
	let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 22.0), egui::Sense::hover());
	ui.painter().rect_filled(r, 0.0, p.border);
	ui.add_space(4.0);
}

fn group_label(ui: &mut egui::Ui, p: &Palette, s: &str) {
	ui.label(RichText::new(s).size(10.5).color(p.muted));
}

fn units_list() -> [&'static str; 5] {
	takeoff_core::MEASURE_UNITS
}

/// "22'-6\"", "22' 6 1/2\"", "12.5", "3.2 m", "150mm", "6\"" → (value, unit).
pub fn parse_length(s: &str, default_unit: &str) -> Option<(f64, String)> {
	let t = s.trim().replace('’', "'").replace('”', "\"").replace("''", "\"");
	if t.is_empty() {
		return None;
	}
	if t.contains('\'') || t.contains('"') {
		let (ft_part, in_part) = match t.split_once('\'') {
			Some((f, rest)) => (f.trim(), rest.trim().trim_start_matches('-').trim()),
			None => ("0", t.as_str()),
		};
		let feet: f64 = if ft_part.is_empty() { 0.0 } else { ft_part.parse().ok()? };
		let inch_str = in_part.trim_end_matches('"').trim();
		let mut inches = 0.0;
		for tok in inch_str.split([' ', '-']).filter(|x| !x.is_empty()) {
			if let Some((a, b)) = tok.split_once('/') {
				inches += a.parse::<f64>().ok()? / b.parse::<f64>().ok()?;
			} else {
				inches += tok.parse::<f64>().ok()?;
			}
		}
		let total_ft = feet + inches / 12.0;
		return Some(if default_unit == "in" { (total_ft * 12.0, "in".into()) } else { (total_ft, "ft".into()) });
	}
	let num_end = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).unwrap_or(t.len());
	let v: f64 = t[..num_end].replace(',', "").parse().ok()?;
	let unit = t[num_end..].trim().to_lowercase();
	let unit = match unit.as_str() {
		"" => default_unit.to_string(),
		"m" | "meter" | "meters" | "metre" | "metres" => "m".into(),
		"cm" => "cm".into(),
		"mm" => "mm".into(),
		"ft" | "feet" | "foot" => "ft".into(),
		"in" | "inch" | "inches" => "in".into(),
		"yd" => "yd".into(),
		_ => return None,
	};
	Some((v, unit))
}

/// In macOS full screen on a display without a notch, the top of the window is
/// where the system menu bar slides down: clicks there open it instead of our
/// File / Edit row. Move the row below it. (On a notched display macOS already
/// places full-screen windows below the menu bar, so the window is shorter than
/// the screen and nothing is needed.)
pub fn fullscreen_top_inset(v: &egui::ViewportInfo) -> f32 {
	const MENU_STRIP: f32 = 26.0;
	if !cfg!(target_os = "macos") || v.fullscreen != Some(true) {
		return 0.0;
	}
	match (v.inner_rect, v.monitor_size) {
		(Some(r), Some(m)) if r.height() + 1.0 >= m.y => MENU_STRIP,
		_ => 0.0,
	}
}

#[cfg(test)]
mod tests {
	use super::{fullscreen_top_inset, parse_length};

	#[test]
	fn full_screen_inset_only_when_the_window_covers_the_menu_strip() {
		let info = |fullscreen, h: f32| eframe::egui::ViewportInfo {
			fullscreen: Some(fullscreen),
			inner_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1920.0, h))),
			monitor_size: Some(eframe::egui::vec2(1920.0, 1080.0)),
			..Default::default()
		};
		let want = if cfg!(target_os = "macos") { 26.0 } else { 0.0 };
		assert_eq!(fullscreen_top_inset(&info(true, 1080.0)), want, "external display, full screen");
		assert_eq!(fullscreen_top_inset(&info(true, 1048.0)), 0.0, "notched display: already below the menu bar");
		assert_eq!(fullscreen_top_inset(&info(false, 1080.0)), 0.0, "not full screen");
		assert_eq!(fullscreen_top_inset(&eframe::egui::ViewportInfo::default()), 0.0, "unknown");
	}

	#[test]
	fn lengths() {
		assert_eq!(parse_length("22'-6\"", "ft"), Some((22.5, "ft".into())));
		assert_eq!(parse_length("1' 6 1/2\"", "ft"), Some((1.0 + 6.5 / 12.0, "ft".into())));
		assert_eq!(parse_length("6\"", "ft"), Some((0.5, "ft".into())));
		assert_eq!(parse_length("3.2 m", "ft"), Some((3.2, "m".into())));
		assert_eq!(parse_length("150mm", "ft"), Some((150.0, "mm".into())));
		assert_eq!(parse_length("12.5", "ft"), Some((12.5, "ft".into())));
		assert_eq!(parse_length("abc", "ft"), None);
	}
}
