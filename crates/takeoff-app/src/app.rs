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
			Ok(d) => {
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

		// Dropped files.
		let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
		let mut images = Vec::new();
		for f in dropped {
			if f.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
				self.open(f, &ctx);
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
		self.ai.models_window(&ctx, &mut self.settings, &p);
		self.toasts(&ctx, &p);
	}

	fn top_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		let inset = ctx.input(|i| fullscreen_top_inset(i.viewport()));
		egui::Panel::top("menu").frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(8, 4)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			if inset > 0.0 {
				ui.add_space(inset);
			}
			ui.horizontal(|ui| {
				// Brand mark.
				let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
				ui.painter().rect_filled(r, 4.0, p.accent_fill);
				ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, i::RULER, theme::icon_font(12.0), p.on_accent);
				ui.label(RichText::new("Takeoff Desk").font(theme::semibold(13.5)));
				ui.add_space(8.0);
				self.menus(ui, p);
				ui.add_space(8.0);
				// Document tabs.
				let mut close = None;
				for (idx, d) in self.docs.iter().enumerate() {
					let active = idx == self.active;
					let dirty = d.is_dirty();
					let frame = egui::Frame::new()
						.fill(if active { p.app_bg } else { Color32::TRANSPARENT })
						.stroke(if active { Stroke::new(1.0, p.border) } else { Stroke::NONE })
						.corner_radius(4)
						.inner_margin(egui::Margin::symmetric(8, 3));
					frame.show(ui, |ui| {
						ui.horizontal(|ui| {
							ui.spacing_mut().item_spacing.x = 6.0;
							ui.label(RichText::new(i::FILE_TEXT).font(theme::icon_font(12.0)).color(if active { p.accent } else { p.muted }));
							let name = if d.name.chars().count() > 34 { format!("{}…", d.name.chars().take(33).collect::<String>()) } else { d.name.clone() };
							let r = ui.add(egui::Label::new(RichText::new(name).size(12.5).color(if active { p.text } else { p.text2 })).sense(egui::Sense::click()));
							if r.clicked() {
								self.active = idx;
							}
							r.on_hover_text(d.path.display().to_string());
							if dirty {
								ui.label(RichText::new("●").size(9.0).color(p.accent));
							}
							if ui.add(egui::Button::new(RichText::new(i::X).font(theme::icon_font(11.0)).color(p.muted)).frame(false)).clicked() {
								close = Some(idx);
							}
						});
					});
				}
				if let Some(c) = close {
					self.active = c;
					self.close_active();
				}
				if ui.add(egui::Button::new(RichText::new(i::PLUS).font(theme::icon_font(14.0)).color(p.text2)).frame(false)).on_hover_text("Open drawing (⌘O)").clicked() {
					self.open_dialog(&ctx);
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if ui.add(egui::Button::new(RichText::new(format!("{}  Feedback", i::MESSAGE_SQUARE_TEXT)).size(12.5).color(p.text2)).frame(false)).on_hover_text("Email the Takeoff Desk team (rahul@construction.live)").clicked() {
						self.send_feedback();
					}
					if self.updater.available(&self.settings.skipped_update).is_some() && self.updater.dismissed {
						if ui.add(egui::Button::new(RichText::new(format!("{}  Update", i::DOWNLOAD)).size(12.5).color(p.accent)).frame(false)).on_hover_text("A new version is available").clicked() {
							self.updater.dismissed = false;
						}
					}
				});
			});
		});
		if self.docs.is_empty() {
			return;
		}
		egui::Panel::top("tools").frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(8, 4)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			ui.horizontal(|ui| {
				ui.spacing_mut().item_spacing.x = 2.0;
				let undo = self.doc().map(|d| d.doc.lock().undo_label().map(str::to_string)).flatten();
				let redo = self.doc().map(|d| d.doc.lock().redo_label().map(str::to_string)).flatten();
				if widgets::icon_button(ui, p, i::UNDO_2, &undo.clone().map(|l| format!("Undo {l} (⌘Z)")).unwrap_or("Nothing to undo".into()), false).clicked() {
					if let Some(d) = self.doc() {
						d.doc.lock().undo();
					}
				}
				if widgets::icon_button(ui, p, i::REDO_2, &redo.clone().map(|l| format!("Redo {l} (⇧⌘Z)")).unwrap_or("Nothing to redo".into()), false).clicked() {
					if let Some(d) = self.doc() {
						d.doc.lock().redo();
					}
				}
				separator(ui, p);
				for t in [Tool::Select, Tool::Pan] {
					self.tool_button(ui, p, t);
				}
				separator(ui, p);
				group_label(ui, p, "Measure");
				for t in Tool::MEASURE {
					self.tool_button(ui, p, t);
				}
				separator(ui, p);
				group_label(ui, p, "Markup");
				for t in Tool::MARKUP {
					self.tool_button(ui, p, t);
				}
				separator(ui, p);
				let snap = self.settings.snap;
				if widgets::icon_button(ui, p, i::MAGNET, if snap { "Snap to drawing lines: on (S) · hold Alt to bypass" } else { "Snap to drawing lines: off (S)" }, snap).clicked() {
					self.settings.snap = !snap;
					self.settings.save();
				}
				let badges = self.canvas.show_badges;
				if widgets::icon_button(ui, p, i::HASH, if badges { "Hide number badges" } else { "Show number badges" }, badges).clicked() {
					self.canvas.show_badges = !badges;
					self.settings.show_badges = !badges;
					self.settings.save();
				}
				if self.docs.get(self.active).is_some_and(|d| d.map.is_some()) {
					let links = self.settings.map_links;
					if widgets::icon_button(ui, p, i::EXTERNAL_LINK, if links { "Map links on the sheet: on · click a callout to go to what it names" } else { "Map links on the sheet: off" }, links).clicked() {
						self.settings.map_links = !links;
						self.settings.save();
					}
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					let ai_on = self.settings.right_panel && self.right.tab == 2;
					let mode = self.docs.get(self.active).map(|d| d.view_mode);
					let r = ui.add(egui::Button::new(RichText::new(format!("{}  AI", i::SPARKLES)).font(theme::semibold(12.5)).color(if ai_on { p.on_accent } else { p.accent })).fill(if ai_on { p.accent_fill } else { Color32::TRANSPARENT }).stroke(Stroke::new(1.0, p.accent)).corner_radius(4));
					if r.on_hover_text("Ask the drawing agent (⌘J)").clicked() {
						self.toggle_ai();
					}
					if self.ai.busy() {
						ui.spinner();
					}
					ui.add_space(10.0);
					if let Some(m) = mode {
						self.view_switch(ui, p, m);
					}
				});
			});
		});
	}

	/// Page | Graph: the sheet itself, or the map of the whole set.
	fn view_switch(&mut self, ui: &mut egui::Ui, p: &Palette, mode: ViewMode) {
		let building = self.docs.get(self.active).is_some_and(|d| self.ai.map_chat(&self.settings, &d.path).running_here);
		let frame = egui::Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(5).inner_margin(egui::Margin::same(2));
		frame.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			// Right-to-left: Graph is added first so it sits on the right.
			for (m, icon, label, tip) in [(ViewMode::Graph, i::WAYPOINTS, "Graph", "The map of the whole set: sheets, details, notes and how they relate (⌘G)"), (ViewMode::Page, i::FILE_TEXT, "Page", "The sheet (⌘G)")] {
				let on = mode == m;
				let text = if m == ViewMode::Graph && building { format!("{icon}  {label} …") } else { format!("{icon}  {label}") };
				let r = ui.add(egui::Button::new(RichText::new(text).size(12.5).color(if on { p.text } else { p.text2 })).fill(if on { p.hover } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).corner_radius(4));
				if r.on_hover_text(tip).clicked() {
					self.set_view_mode(m);
				}
			}
		});
	}

	fn set_view_mode(&mut self, m: ViewMode) {
		if let Some(d) = self.docs.get_mut(self.active) {
			if d.view_mode != m {
				// The graph opens on what the Map tab shows (the sheet on screen, or the area picked on it).
				let focus = d.map_focus();
				d.view_mode = m;
				if let (ViewMode::Graph, Some(f)) = (m, focus) {
					d.graph.select_node(&f);
				}
			}
		}
	}

	fn toggle_ai(&mut self) {
		if self.settings.right_panel && self.right.tab == 2 {
			self.right.tab = 0;
		} else {
			self.settings.right_panel = true;
			self.right.tab = 2;
		}
	}

	fn tool_button(&mut self, ui: &mut egui::Ui, p: &Palette, t: Tool) {
		let tip = match t.shortcut() {
			Some(k) => format!("{} ({k})\n{}", t.label(), t.hint()),
			None => t.label().to_string(),
		};
		if widgets::icon_button(ui, p, t.icon(), &tip, self.canvas.tool == t).clicked() {
			self.set_view_mode(ViewMode::Page);
			self.canvas.set_tool(t);
			if matches!(t, Tool::Annot(_)) {
				self.settings.right_panel = true;
				if self.docs.get(self.active).is_some_and(|d| d.selection.is_empty()) {
					self.right.tab = 1;
				}
			}
		}
	}

	fn menus(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		let has_doc = !self.docs.is_empty();
		ui.menu_button("File", |ui| {
			ui.set_min_width(240.0);
			if ui.button(format!("{}  Open…    ⌘O", i::FOLDER_OPEN)).clicked() {
				ui.close();
				self.open_dialog(&ctx);
			}
			ui.menu_button(format!("{}  Open recent", i::HISTORY), |ui| {
				if self.settings.recent.is_empty() {
					ui.label(RichText::new("Nothing yet").color(p.muted));
				}
				for r in self.settings.recent.clone() {
					let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
					if ui.button(name).on_hover_text(r.display().to_string()).clicked() {
						ui.close();
						self.open(r, &ctx);
					}
				}
			});
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Save takeoff    ⌘S", i::SAVE))).clicked() {
				if let Some(d) = self.doc() {
					d.save_now();
				}
				ui.close();
			}
			ui.menu_button(format!("{}  Export", i::FILE_OUTPUT), |ui| {
				ui.set_min_width(260.0);
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Marked-up PDF (flattened)…", i::FILE_TEXT))).clicked() {
					ui.close();
					self.export_pdf();
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Page image with markups (PNG)…", i::IMAGE))).clicked() {
					ui.close();
					self.export_png();
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Markups list (CSV)…", i::FILE_SPREADSHEET))).clicked() {
					ui.close();
					self.export_csv(false);
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Quantity summary (CSV)…", i::SIGMA))).clicked() {
					ui.close();
					self.export_csv(true);
				}
				if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Takeoff file (JSON)…", i::DOWNLOAD))).clicked() {
					ui.close();
					self.export_json();
				}
			});
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Close    ⌘W", i::X))).clicked() {
				ui.close();
				self.close_active();
			}
		});
		ui.menu_button("Edit", |ui| {
			ui.set_min_width(220.0);
			let enabled = has_doc;
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Undo    ⌘Z", i::UNDO_2))).clicked() {
				if let Some(d) = self.doc() {
					d.doc.lock().undo();
				}
				ui.close();
			}
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Redo    ⇧⌘Z", i::REDO_2))).clicked() {
				if let Some(d) = self.doc() {
					d.doc.lock().redo();
				}
				ui.close();
			}
			ui.separator();
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Select all on page    ⌘A", i::MOUSE_POINTER_2))).clicked() {
				if let Some(d) = self.docs.get_mut(self.active) {
					self.canvas.select_all(d);
				}
				ui.close();
			}
			if ui.add_enabled(enabled, egui::Button::new(format!("{}  Confirm all AI marks", i::CHECK))).clicked() {
				if let Some(d) = self.doc() {
					d.doc.lock().edit("Confirm all", |t| t.marks.iter_mut().for_each(|m| m.status = MarkStatus::Confirmed));
				}
				ui.close();
			}
			ui.separator();
			if ui.button(format!("{}  Preferences…", i::SETTINGS)).clicked() {
				self.dialog = Some(Dialog::Settings);
				ui.close();
			}
		});
		ui.menu_button("Document", |ui| {
			ui.set_min_width(240.0);
			let page = self.docs.get(self.active).map(|d| d.page).unwrap_or(1);
			let picked = |s: &Self| -> Vec<u32> {
				let mut v: Vec<u32> = s.left.picked.iter().copied().collect();
				v.sort();
				if v.is_empty() { vec![page] } else { v }
			};
			let mut act = None;
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Rotate pages clockwise", i::ROTATE_CW))).clicked() {
				act = Some(PageAction::Rotate { pages: picked(self), quarter_turns: 1 });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Insert blank page", i::FILE_PLUS))).clicked() {
				act = Some(PageAction::InsertBlank { at: page + 1 });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Insert pages from PDF…", i::FILE_INPUT))).clicked() {
				act = Some(PageAction::InsertFile { at: page + 1 });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Extract pages…", i::FILE_OUTPUT))).clicked() {
				act = Some(PageAction::Extract { pages: picked(self) });
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Delete pages", i::FILE_MINUS))).clicked() {
				act = Some(PageAction::Delete { pages: picked(self) });
			}
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Split document…", i::SPLIT))).clicked() {
				act = Some(PageAction::Split);
			}
			if ui.button(format!("{}  Merge PDFs…", i::COMBINE)).clicked() {
				act = Some(PageAction::Merge);
			}
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Compare revisions…", i::GIT_COMPARE_ARROWS))).on_hover_text("Overlay another revision: red = only here, blue = only there").clicked() {
				ui.close();
				if let Some(other) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_title("Choose the other revision").pick_file() {
					if let Some(d) = self.docs.get_mut(self.active) {
						let pg = d.page;
						d.start_compare(other, pg, &ctx);
					}
				}
			}
			if let Some(a) = act {
				ui.close();
				self.page_action(a, &ctx);
			}
		});
		ui.menu_button("View", |ui| {
			ui.set_min_width(220.0);
			let mode = self.docs.get(self.active).map(|d| d.view_mode);
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Page view", i::FILE_TEXT)).selected(mode == Some(ViewMode::Page))).clicked() {
				self.set_view_mode(ViewMode::Page);
				ui.close();
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Graph view    ⌘G", i::WAYPOINTS)).selected(mode == Some(ViewMode::Graph))).clicked() {
				self.set_view_mode(ViewMode::Graph);
				ui.close();
			}
			ui.separator();
			ui.checkbox(&mut self.settings.left_panel, "Pages panel");
			ui.checkbox(&mut self.settings.right_panel, "Side panel");
			ui.checkbox(&mut self.settings.markups_list, "Markups list");
			ui.separator();
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Fit page    ⌘0", i::MAXIMIZE))).clicked() {
				if let Some(d) = self.doc() {
					d.fit_page();
				}
				ui.close();
			}
			if ui.add_enabled(has_doc, egui::Button::new(format!("{}  Fit width", i::ARROW_LEFT_RIGHT))).clicked() {
				if let Some(d) = self.doc() {
					d.fit_width();
				}
				ui.close();
			}
			ui.separator();
			ui.label(RichText::new("Theme").color(p.muted).size(11.5));
			let mut t = self.settings.theme;
			ui.horizontal(|ui| {
				ui.selectable_value(&mut t, ThemeChoice::System, format!("{} System", i::MONITOR));
				ui.selectable_value(&mut t, ThemeChoice::Light, format!("{} Light", i::SUN));
				ui.selectable_value(&mut t, ThemeChoice::Dark, format!("{} Dark", i::MOON));
			});
			if t != self.settings.theme {
				self.settings.theme = t;
				self.settings.save();
				self.theme_applied = None;
			}
		});
		ui.menu_button("Help", |ui| {
			if ui.button(format!("{}  Send feedback…", i::MESSAGE_SQUARE_TEXT)).clicked() {
				ui.close();
				self.send_feedback();
			}
			if ui.button(format!("{}  Check for updates…", i::REFRESH_CW)).clicked() {
				ui.close();
				self.updater.manual = true;
				self.updater.check(&ctx);
			}
			ui.separator();
			if ui.button(format!("{}  Keyboard shortcuts    ?", i::KEYBOARD)).clicked() {
				self.dialog = Some(Dialog::Shortcuts);
				ui.close();
			}
			if ui.button(format!("{}  About Takeoff Desk", i::INFO)).clicked() {
				self.dialog = Some(Dialog::About);
				ui.close();
			}
		});
	}

	fn panels(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		let active = self.active;
		if self.settings.left_panel {
			let mut action = None;
			egui::Panel::left("left").resizable(true).default_size(236.0).size_range(170.0..=420.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
				ui.set_min_height(ui.available_height());
				ui.set_max_width(ui.available_width());
				if let Some(d) = self.docs.get_mut(active) {
					let before = d.page;
					action = self.left.ui(ui, d, p);
					if d.view_mode == ViewMode::Graph && d.page != before {
						let page = d.page;
						d.map_go(&takeoff_core::docmap::sheet_id(page));
					}
				}
			});
			if let Some(a) = action {
				self.page_action(a, &ctx);
			}
		}
		let mut map_action = None;
		if self.settings.right_panel {
			egui::Panel::right("right").resizable(true).default_size(340.0).size_range(260.0..=620.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |outer| {
				// Lay the contents out in a child that can't grow the panel: if anything
				// in it is too wide, egui would otherwise widen the panel past the window
				// and the canvas would paint over its left edge. Overflow just clips.
				let rect = outer.max_rect();
				let mut inner = outer.new_child(egui::UiBuilder::new().max_rect(rect).layout(*outer.layout()));
				inner.set_clip_rect(rect.intersect(outer.clip_rect()));
				outer.advance_cursor_after_rect(rect);
				let ui = &mut inner;
				let sel = self.docs.get(active).map(|d| d.selection.len()).unwrap_or(0);
				let props = if sel > 0 { format!("Properties ({sel})") } else { "Properties".into() };
				widgets::tabs(ui, p, &[(i::SIGMA, "Takeoff"), (i::LIST, &props), (i::SPARKLES, "AI"), (i::WAYPOINTS, "Map")], &mut self.right.tab);
				ui.separator();
				match self.right.tab {
					3 => {
						let chat = self.docs.get(active).map(|d| self.ai.map_chat(&self.settings, &d.path));
						if let (Some(d), Some(chat)) = (self.docs.get_mut(active), chat) {
							map_action = crate::map_panel::ui(ui, d, p, &chat);
						}
					}
					0 => {
						if let Some(d) = self.docs.get_mut(active) {
							self.right.takeoff(ui, d, &mut self.canvas, &mut self.settings, p);
						}
					}
					1 => {
						if let Some(d) = self.docs.get_mut(active) {
							self.right.properties(ui, d, &mut self.canvas, p);
						}
					}
					_ => {
						let doc = self.docs.get_mut(active);
						self.ai.ui(ui, doc, &mut self.canvas, &mut self.settings, p);
					}
				}
			});
		}
		if let Some(a) = map_action {
			self.map_action(a);
		}
		if self.settings.markups_list {
			let mut action = None;
			let r = egui::Panel::bottom("markups").resizable(true).show_separator_line(false).default_size(210.0).size_range(90.0..=600.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
				// Fill the panel even when the list is short, so its size sticks.
				ui.set_min_height(ui.available_height());
				if let Some(d) = self.docs.get_mut(active) {
					action = self.markups.ui(ui, d, p);
				}
			});
			let _ = r;
			match action {
				Some(ListAction::ExportCsv) => self.export_csv(false),
				Some(ListAction::ExportSummary) => self.export_csv(true),
				None => {}
			}
		}
		let graph = self.docs.get(active).is_some_and(|d| d.view_mode == ViewMode::Graph);
		if graph {
			let chat = self.docs.get(active).map(|d| self.ai.map_chat(&self.settings, &d.path));
			let mut action = None;
			egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas)).show(ui, |ui| {
				if let (Some(d), Some(chat)) = (self.docs.get_mut(active), &chat) {
					action = crate::graph_view::ui(ui, d, p, chat, self.settings.wheel_zooms);
				}
			});
			if let Some(a) = action {
				self.graph_action(a);
			}
			return;
		}
		egui::CentralPanel::default().frame(egui::Frame::new().fill(p.canvas)).show(ui, |ui| {
			if let Some(d) = self.docs.get_mut(active) {
				let (snap, wheel) = (self.settings.snap, self.settings.wheel_zooms);
				self.canvas.user_moved = false;
				self.canvas.map_links = self.settings.map_links;
				self.canvas.ui(ui, d, p, snap, wheel);
				// Panning or zooming yourself while the AI works stops following it.
				if self.canvas.user_moved && self.settings.follow_agent && self.ai.busy() {
					self.settings.follow_agent = false;
					self.settings.save();
				}
				if self.settings.follow_agent && self.ai.busy() {
					let rect = ui.max_rect();
					let mut stop = false;
					egui::Area::new(egui::Id::new("follow-pill")).fixed_pos(egui::pos2(rect.center().x - 120.0, rect.top() + 48.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
						egui::Frame::popup(&ui.ctx().global_style()).inner_margin(egui::Margin::symmetric(12, 6)).corner_radius(14).show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.label(RichText::new(i::CROSSHAIR).font(theme::icon_font(13.0)).color(p.accent));
								ui.label(RichText::new("Following the AI").font(theme::semibold(12.5)));
								if ui.small_button("Stop").clicked() {
									stop = true;
								}
							});
						});
					});
					if stop {
						self.settings.follow_agent = false;
						self.settings.save();
					}
				}
				// Revision-compare bar.
				let mut close_compare = false;
				if let Some(c) = &mut d.compare {
					let rect = ui.max_rect();
					egui::Area::new(egui::Id::new("compare-bar")).fixed_pos(egui::pos2(rect.left() + 12.0, rect.bottom() - 52.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
						egui::Frame::popup(&ui.ctx().global_style()).inner_margin(egui::Margin::symmetric(12, 8)).corner_radius(6).show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.label(RichText::new(i::GIT_COMPARE_ARROWS).font(theme::icon_font(14.0)).color(p.accent));
								let name = c.other.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
								ui.label(RichText::new(format!("Comparing with {name} p{}", c.other_page)).font(theme::semibold(12.5)));
								ui.label(RichText::new("■ only here").color(Color32::from_rgb(230, 40, 40)).size(12.0));
								ui.label(RichText::new("■ only there").color(Color32::from_rgb(40, 90, 230)).size(12.0));
								if c.tex.is_some() {
									ui.label(RichText::new(format!("{:.1}% changed", c.changed * 100.0)).size(12.0).color(p.text2));
								}
								ui.checkbox(&mut c.show_overlay, "Overlay");
								if ui.button(format!("{} Close", i::X)).clicked() {
									close_compare = true;
								}
							});
						});
					});
				}
				if close_compare {
					d.compare = None;
				}
				// Tool hint bubble at the top of the canvas.
				if self.canvas.tool != Tool::Select && self.canvas.tool != Tool::Pan {
					let rect = ui.max_rect();
					let text = format!("{}  {} — {}", self.canvas.tool.icon(), self.canvas.tool.label(), self.canvas.tool.hint());
					let painter = ui.painter();
					let g = painter.layout_no_wrap(text, theme::medium(12.0), p.text);
					let r = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 22.0), g.size() + Vec2::new(22.0, 10.0));
					painter.rect_filled(r.translate(Vec2::new(0.0, 2.0)), 6.0, Color32::from_black_alpha(40));
					painter.rect_filled(r, 6.0, p.panel);
					painter.rect_stroke(r, 6.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
					painter.galley(r.min + Vec2::new(11.0, 5.0), g, p.text);
				}
			}
		});
	}

	// ── Document map ────────────────────────────────────────────────────

	fn graph_action(&mut self, a: GraphAction) {
		let active = self.active;
		match a {
			GraphAction::StartChat { instructions, fresh } => self.start_map_chat(instructions, fresh),
			GraphAction::Stop => self.ai.stop(),
			GraphAction::OpenChat => {
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
			GraphAction::Delete => self.dialog = Some(Dialog::DeleteMap),
			GraphAction::Focus(id) => {
				if let Some(d) = self.docs.get_mut(active) {
					d.map_go(&id);
				}
				self.show_map_tab();
			}
			GraphAction::Open(id) => self.open_on_sheet(&id),
		}
	}

	/// The Map tab, in view.
	fn show_map_tab(&mut self) {
		self.settings.right_panel = true;
		self.right.tab = 3;
	}

	/// Page view, at a node of the map (onto the trail).
	fn open_on_sheet(&mut self, id: &str) {
		if let Some(d) = self.docs.get_mut(self.active) {
			d.view_mode = ViewMode::Page;
			d.map_go(id);
		}
	}

	fn map_action(&mut self, a: crate::map_panel::MapAction) {
		use crate::map_panel::MapAction;
		let active = self.active;
		match a {
			MapAction::Go(id) => {
				if let Some(d) = self.docs.get_mut(active) {
					d.map_go(&id);
				}
			}
			MapAction::Step(k) => {
				if let Some(d) = self.docs.get_mut(active) {
					d.map_step(k);
				}
			}
			MapAction::OpenOnSheet(id) => self.open_on_sheet(&id),
			MapAction::ShowInGraph(id) => {
				self.set_view_mode(ViewMode::Graph);
				if let Some(d) = self.docs.get_mut(active) {
					d.map_go(&id);
				}
			}
			MapAction::Ask { page, region, title } => {
				let Some(path) = self.docs.get(active).map(|d| d.path.clone()) else { return };
				let region = region.map(|r| (r.x, r.y, r.w, r.h)).unwrap_or((0.0, 0.0, 1.0, 1.0));
				self.ai.ask_about(takeoff_agent::ViewAttachment { drawing: path, page, region }, &format!("What is {title}, and what does it connect to?"));
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
			MapAction::StartMapping => self.set_view_mode(ViewMode::Graph),
			MapAction::OpenChat => {
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
		}
	}

	/// Map the set: a new conversation in the AI tab, on the chat's model.
	fn start_map_chat(&mut self, instructions: String, fresh: bool) {
		let active = self.active;
		let Some(d) = self.docs.get_mut(active) else { return };
		if fresh {
			d.delete_map();
		}
		let resume = d.map.as_ref().is_some_and(|m| !m.sheets.is_empty());
		let mut text = String::from("Map this drawing set: build the document map.");
		if resume {
			text.push_str(" A map is already started: keep what is there and carry on with the sheets not mapped yet.");
		}
		if !instructions.is_empty() {
			text.push_str(&format!("\n\n{instructions}"));
		}
		let d = &self.docs[active];
		match self.ai.start_map_chat(text, d, &self.settings) {
			Ok(()) => {
				self.docs[active].map_status = None;
				self.settings.right_panel = true;
				self.right.tab = 2;
			}
			Err(e) => self.docs[active].map_status = Some((e, true)),
		}
	}

	/// Take the canvas to where the AI is looking (switching tabs if needed).
	fn follow_agent(&mut self) {
		let Some((path, page, region)) = self.ai.follow_request.take() else { return };
		if !self.settings.follow_agent {
			return;
		}
		if let Some(idx) = self.docs.iter().position(|d| d.path == path) {
			// A tab not shown yet doesn't know the canvas size; it's the same canvas.
			let canvas = self.docs.get(self.active).map(|d| d.canvas_rect).filter(|r| r.area() > 0.0);
			if let (Some(c), true) = (canvas, self.docs[idx].canvas_rect.area() <= 0.0) {
				self.docs[idx].canvas_rect = c;
			}
			self.active = idx;
			self.docs[idx].view_mode = ViewMode::Page;
			self.docs[idx].zoom_to(page, region);
		}
	}

	fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let active = self.active;
		egui::Panel::bottom("status").exact_size(30.0).frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 3)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			let Some(d) = self.docs.get_mut(active) else { return };
			ui.horizontal_centered(|ui| {
				ui.spacing_mut().item_spacing.x = 4.0;
				let n = d.page_count();
				if widgets::icon_button(ui, p, i::CHEVRON_LEFT, "Previous page (PgUp)", false).clicked() && d.page > 1 {
					let pg = d.page - 1;
					d.go_to_page(pg);
				}
				if self.page_input.is_empty() || !ui.memory(|m| m.has_focus(egui::Id::new("page-input"))) {
					self.page_input = d.page.to_string();
				}
				let r = ui.add(egui::TextEdit::singleline(&mut self.page_input).id(egui::Id::new("page-input")).desired_width(36.0).horizontal_align(egui::Align::Center));
				if r.lost_focus() {
					if let Ok(v) = self.page_input.trim().parse::<u32>() {
						d.go_to_page(v);
					}
					self.page_input.clear();
				}
				ui.label(RichText::new(format!("/ {n}")).color(p.text2));
				if widgets::icon_button(ui, p, i::CHEVRON_RIGHT, "Next page (PgDn)", false).clicked() && d.page < n {
					let pg = d.page + 1;
					d.go_to_page(pg);
				}
				let label = d.page_label(d.page);
				if !label.starts_with("Page ") {
					ui.label(RichText::new(label).font(theme::semibold(12.0)));
				}
				ui.add_space(10.0);
				ui.separator();
				// Zoom.
				if widgets::icon_button(ui, p, i::ZOOM_OUT, "Zoom out (⌘−)", false).clicked() {
					let c = d.canvas_rect.center();
					d.zoom_at(c, 1.0 / 1.25);
				}
				let pct = (d.zoom * 100.0 * 72.0 / 72.0).round();
				egui::ComboBox::from_id_salt("zoom").selected_text(format!("{pct}%")).width(76.0).show_ui(ui, |ui| {
					if ui.selectable_label(false, "Fit page").clicked() {
						d.fit_page();
					}
					if ui.selectable_label(false, "Fit width").clicked() {
						d.fit_width();
					}
					for z in [25.0, 50.0, 100.0, 200.0, 400.0, 800.0] {
						if ui.selectable_label(false, format!("{z}%")).clicked() {
							let c = d.canvas_rect.center();
							d.zoom_at(c, z / 100.0 / d.zoom);
						}
					}
				});
				if widgets::icon_button(ui, p, i::ZOOM_IN, "Zoom in (⌘=)", false).clicked() {
					let c = d.canvas_rect.center();
					d.zoom_at(c, 1.25);
				}
				if widgets::icon_button(ui, p, i::MAXIMIZE, "Fit page (⌘0)", false).clicked() {
					d.fit_page();
				}
				ui.separator();
				// Scale + cursor position.
				let (w_pt, h_pt) = d.page_size(d.page);
				let (scale, display) = {
					let doc = d.doc.lock();
					let at = self.canvas.cursor_page.unwrap_or(Point { x: 0.5, y: 0.5 });
					(doc.data.scale_at(d.page, at).cloned(), doc.data.display.clone())
				};
				match &scale {
					Some(s) => {
						ui.label(RichText::new(format!("{} {}", i::SCALING, describe_scale(s, w_pt))).size(12.0).color(p.text2));
					}
					None => {
						if ui.add(egui::Button::new(RichText::new(format!("{} No scale — set one", i::CIRCLE_ALERT)).size(12.0).color(p.warning)).frame(false)).clicked() {
							self.right.tab = 0;
							self.settings.right_panel = true;
						}
					}
				}
				if let (Some(c), Some(s)) = (self.canvas.cursor_page, &scale) {
					if let Some(upp) = takeoff_core::geometry::scale_units_per_px(s, w_pt, h_pt) {
						let (x, y) = (c.x * w_pt * upp, c.y * h_pt * upp);
						ui.label(RichText::new(format!("x {}  y {}", units::format_length(x, &s.unit, &display), units::format_length(y, &s.unit, &display))).size(11.5).color(p.muted).monospace());
					}
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					match &d.save_error {
						Some(e) => {
							ui.label(RichText::new(format!("{} Not saved", i::CIRCLE_ALERT)).color(p.danger)).on_hover_text(e);
						}
						None if d.is_dirty() => {
							ui.label(RichText::new("Saving…").size(12.0).color(p.muted));
						}
						None => {
							ui.label(RichText::new(format!("{} Saved", i::CHECK)).size(12.0).color(p.muted)).on_hover_text(format!("Takeoff saved next to the PDF:\n{}", takeoff_core::sidecar::sidecar_path(&d.path).display()));
						}
					}
					ui.separator();
					let (marks, meas, ann) = {
						let doc = d.doc.lock();
						(doc.data.marks.iter().filter(|m| m.page == d.page).count(), doc.data.measurements.iter().filter(|m| m.page == d.page).count(), doc.data.annotations.iter().filter(|a| a.page == d.page).count())
					};
					ui.label(RichText::new(format!("{marks} counts · {meas} measurements · {ann} markups on this page")).size(12.0).color(p.muted));
					if !d.selection.is_empty() {
						ui.separator();
						ui.label(RichText::new(format!("{} selected", d.selection.len())).size(12.0).color(p.selection));
					}
				});
			});
		});
	}

	fn start_screen(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let ctx = ui.ctx().clone();
		ui.vertical_centered(|ui| {
			ui.add_space((ui.available_height() * 0.16).max(30.0));
			let (r, _) = ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
			ui.painter().rect_filled(r, 10.0, p.accent_fill);
			ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, i::RULER, theme::icon_font(30.0), p.on_accent);
			ui.add_space(14.0);
			ui.label(RichText::new("Takeoff Desk").font(theme::bold(28.0)).color(p.text));
			ui.label(RichText::new("Quantity takeoff with an AI that can see your drawings.").size(14.0).color(p.text2));
			ui.add_space(22.0);
			ui.horizontal(|ui| {
				let w = 420.0;
				ui.add_space((ui.available_width() - w) / 2.0);
				ui.vertical(|ui| {
					ui.set_width(w);
					if ui.add_sized([w, 40.0], egui::Button::new(RichText::new(format!("{}   Open a drawing…", i::FOLDER_OPEN)).font(theme::semibold(14.0)).color(p.on_accent)).fill(p.accent_fill).corner_radius(6)).clicked() {
						self.open_dialog(&ctx);
					}
					ui.add_space(6.0);
					ui.label(RichText::new("or drop PDFs anywhere in this window").size(12.0).color(p.muted));
					ui.add_space(18.0);
					if !self.settings.recent.is_empty() {
						widgets::section_label(ui, p, "Recent");
						for r in self.settings.recent.clone().into_iter().take(8) {
							let exists = r.exists();
							let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
							let dir = r.parent().map(|d| d.display().to_string()).unwrap_or_default();
							let resp = egui::Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
								ui.set_width(w - 22.0);
								ui.horizontal(|ui| {
									ui.label(RichText::new(i::FILE_TEXT).font(theme::icon_font(16.0)).color(if exists { p.accent } else { p.muted }));
									ui.vertical(|ui| {
										ui.label(RichText::new(&name).font(theme::semibold(13.0)).color(if exists { p.text } else { p.muted }));
										ui.add(egui::Label::new(RichText::new(&dir).size(11.0).color(p.muted)).truncate());
									});
								});
							});
							let resp = resp.response.interact(egui::Sense::click());
							if resp.hovered() {
								ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
							}
							if resp.clicked() && exists {
								self.open(r, &ctx);
							}
							ui.add_space(4.0);
						}
					}
					ui.add_space(18.0);
					widgets::section_label(ui, p, "What you can do");
					for (icon, title, body) in [
						(i::HASH, "Count, measure, mark up", "Counts, lengths, areas with cutouts, volumes, angles, clouds, callouts — snapped to the drawing's own lines."),
						(i::SPARKLES, "Ask the drawing", "A local or cloud AI that sets scales, counts symbols, measures runs and reads title blocks — you review what it adds."),
						(i::FILES, "Work the PDF", "Split, merge, extract, rotate, insert and reorder pages; export a flattened, marked-up PDF and CSV quantities."),
					] {
						ui.horizontal(|ui| {
							ui.label(RichText::new(icon).font(theme::icon_font(16.0)).color(p.accent));
							ui.vertical(|ui| {
								ui.label(RichText::new(title).font(theme::semibold(13.0)));
								ui.label(RichText::new(body).size(12.0).color(p.text2));
							});
						});
						ui.add_space(6.0);
					}
				});
			});
		});
	}

	// ── Shortcuts ───────────────────────────────────────────────────────

	fn shortcuts(&mut self, ctx: &egui::Context) {
		let cmd = |k: Key| KeyboardShortcut::new(Modifiers::COMMAND, k);
		let cmd_shift = |k: Key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::O))) {
			self.open_dialog(ctx);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::J))) {
			self.toggle_ai();
			ctx.memory_mut(|m| m.request_focus(egui::Id::new("ai-input")));
		}
		if self.docs.is_empty() {
			return;
		}
		let typing = ctx.egui_wants_keyboard_input();
		let active = self.active;
		let Some(d) = self.docs.get_mut(active) else { return };
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::S))) {
			d.save_now();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::W))) {
			self.close_active();
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::G))) {
			let next = if d.view_mode == ViewMode::Graph { ViewMode::Page } else { ViewMode::Graph };
			self.set_view_mode(next);
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::F))) {
			if d.view_mode == ViewMode::Graph {
				ctx.memory_mut(|m| m.request_focus(egui::Id::new("graph-search")));
				return;
			}
			self.settings.left_panel = true;
			self.left.focus_search();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Num0))) {
			d.fit_page();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Equals)) || i.consume_shortcut(&cmd(Key::Plus))) {
			let c = d.canvas_rect.center();
			d.zoom_at(c, 1.25);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Minus))) {
			let c = d.canvas_rect.center();
			d.zoom_at(c, 0.8);
		}
		if typing {
			return;
		}
		// Back / Forward along the map trail (⌥← ⌥→, ⌘[ ⌘]).
		let (back, fwd) = ctx.input_mut(|i| {
			let alt = |k| egui::KeyboardShortcut::new(Modifiers::ALT, k);
			(i.consume_shortcut(&alt(Key::ArrowLeft)) || i.consume_shortcut(&cmd(Key::OpenBracket)), i.consume_shortcut(&alt(Key::ArrowRight)) || i.consume_shortcut(&cmd(Key::CloseBracket)))
		});
		if back {
			d.map_back();
		}
		if fwd {
			d.map_forward();
		}
		// The graph has no tools, selection or pages to step through.
		if d.view_mode == ViewMode::Graph {
			if ctx.input(|i| i.key_pressed(Key::Escape)) {
				d.graph.deselect();
			}
			return;
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::Z)) || i.consume_shortcut(&cmd(Key::Y))) {
			d.doc.lock().redo();
		} else if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Z))) {
			d.doc.lock().undo();
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::A))) {
			self.canvas.select_all(d);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::C))) {
			self.canvas.copy(d);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::X))) {
			self.canvas.copy(d);
			self.canvas.delete_selection(d);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::V))) {
			self.canvas.paste(d, false);
		}
		if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::D))) {
			self.canvas.copy(d);
			self.canvas.paste(d, true);
		}
		let (esc, del, pgup, pgdn, arrows, help, shift) = ctx.input(|i| {
			(
				i.key_pressed(Key::Escape),
				i.key_pressed(Key::Delete) || (i.key_pressed(Key::Backspace) && self.canvas.points.is_empty()),
				i.key_pressed(Key::PageUp),
				i.key_pressed(Key::PageDown),
				[i.key_pressed(Key::ArrowLeft), i.key_pressed(Key::ArrowRight), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)],
				i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?")),
				i.modifiers.shift,
			)
		});
		if esc {
			self.canvas.escape(d);
		}
		if del && !d.selection.is_empty() {
			self.canvas.delete_selection(d);
		}
		if pgup && d.page > 1 {
			let p = d.page - 1;
			d.go_to_page(p);
		}
		if pgdn && d.page < d.page_count() {
			let p = d.page + 1;
			d.go_to_page(p);
		}
		let step = if shift { 10.0 } else { 1.0 };
		if !d.selection.is_empty() {
			let [l, r, u, dn] = arrows;
			let (dx, dy) = ((r as i32 - l as i32) as f32 * step, (dn as i32 - u as i32) as f32 * step);
			if dx != 0.0 || dy != 0.0 {
				self.canvas.nudge(d, dx, dy);
			}
		} else if arrows[0] && d.page > 1 {
			let p = d.page - 1;
			d.go_to_page(p);
		} else if arrows[1] && d.page < d.page_count() {
			let p = d.page + 1;
			d.go_to_page(p);
		}
		if help {
			self.dialog = Some(Dialog::Shortcuts);
		}
		// Single-letter tool keys.
		let mods = ctx.input(|i| i.modifiers);
		if !mods.command && !mods.ctrl && !mods.alt {
			let typed: Vec<String> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None }).collect());
			for t in typed {
				let c = t.chars().next().unwrap_or(' ').to_ascii_uppercase();
				if c == 'S' {
					self.settings.snap = !self.settings.snap;
					self.toast(if self.settings.snap { "Snap on" } else { "Snap off" }, false);
					continue;
				}
				if let Some(tool) = Tool::all().find(|t| t.shortcut() == Some(c)) {
					self.canvas.set_tool(tool);
				}
			}
		}
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
			}
		}
	}

	// ── Dialogs ─────────────────────────────────────────────────────────

	fn dialogs(&mut self, ctx: &egui::Context, p: &Palette) {
		let Some(mut dialog) = self.dialog.take() else { return };
		let mut keep = true;
		let title = match &dialog {
			Dialog::Calibrate { .. } => "Calibrate scale",
			Dialog::Viewport { .. } => "New viewport",
			Dialog::Depth { .. } => "Depth for volume",
			Dialog::Split { .. } => "Split document",
			Dialog::Merge { .. } => "Merge PDFs",
			Dialog::Settings => "Preferences",
			Dialog::Shortcuts => "Keyboard shortcuts",
			Dialog::About => "About Takeoff Desk",
			Dialog::ConfirmPdfEdit { .. } => "Change the PDF file?",
			Dialog::DeleteMap => "Delete the document map?",
		};
		let modal = egui::Modal::new(egui::Id::new("dialog")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(18).corner_radius(8));
		let resp = modal.show(ctx, |ui| {
			ui.set_width(match dialog {
				Dialog::Shortcuts => 560.0,
				Dialog::Merge { .. } => 520.0,
				_ => 420.0,
			});
			ui.label(RichText::new(title).font(theme::semibold(16.0)));
			ui.add_space(10.0);
			match &mut dialog {
				Dialog::Calibrate { page, p1, p2, text, unit, error, all_pages } => {
					ui.label(RichText::new("How long is the line you drew, in real life?").color(p.text2));
					ui.add_space(4.0);
					let r = ui.add(egui::TextEdit::singleline(text).hint_text("e.g. 22'-6\"  or  12.5  or  3.2 m").desired_width(220.0));
					r.request_focus();
					ui.horizontal(|ui| {
						ui.label("Unit if not typed");
						for u in units_list() {
							ui.selectable_value(unit, u.to_string(), u);
						}
					});
					ui.checkbox(all_pages, "Use for all pages of the same size");
					if let Some(e) = error {
						ui.label(RichText::new(e.as_str()).color(p.danger));
					}
					ui.add_space(8.0);
					let enter = ui.input(|i| i.key_pressed(Key::Enter));
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Set scale").clicked() || enter {
							match parse_length(text, unit) {
								Some((v, u)) if v > 0.0 => {
									let (page, p1, p2, all) = (*page, *p1, *p2, *all_pages);
									if let Some(d) = self.docs.get_mut(self.active) {
										let sizes = d.page_sizes.clone();
										let here = d.page_size(page);
										d.doc.lock().edit("Calibrate", |t| {
											let pages: Vec<u32> = if all { sizes.iter().enumerate().filter(|(_, s)| **s == here).map(|(i, _)| i as u32 + 1).collect() } else { vec![page] };
											for pg in pages {
												t.page_scales.retain(|s| s.page != pg);
												t.page_scales.push(PageScale { page: pg, p1, p2, distance: v, unit: u.clone() });
											}
										});
									}
									self.canvas.set_tool(Tool::Select);
									keep = false;
								}
								_ => *error = Some("Type a length like 22'-6\", 12.5 or 3.2 m".into()),
							}
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Viewport { page, rect, name, preset } => {
					ui.label(RichText::new("Measurements inside this box use its own scale.").color(p.text2));
					ui.add(egui::TextEdit::singleline(name).desired_width(240.0));
					let presets: Vec<&ScalePreset> = ScalePreset::all().collect();
					egui::ComboBox::from_id_salt("vp-preset").selected_text(presets[*preset].label).width(240.0).show_ui(ui, |ui| {
						for (k, pr) in presets.iter().enumerate() {
							ui.selectable_value(preset, k, pr.label);
						}
					});
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Create viewport").clicked() {
							if let Some(d) = self.docs.get_mut(self.active) {
								let (w, _) = d.page_size(*page);
								let scale = presets[*preset].to_page_scale(*page, w);
								let vp = Viewport { id: new_id("vp"), page: *page, name: name.clone(), x: rect.0, y: rect.1, w: rect.2, h: rect.3, scale };
								d.doc.lock().edit("Viewport", |t| t.viewports.push(vp));
							}
							keep = false;
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Depth { id, value } => {
					ui.label(RichText::new("Depth or thickness, in the page's scale unit (e.g. 0.33 for a 4\" slab in feet).").color(p.text2));
					let r = ui.add(egui::TextEdit::singleline(value).desired_width(160.0));
					r.request_focus();
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Set depth").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
							let unit = self.docs.get(self.active).and_then(|d| d.doc.lock().data.scale_for(d.page).map(|s| s.unit.clone())).unwrap_or_else(|| "ft".into());
							if let Some((v, _)) = parse_length(value, &unit) {
								if let Some(d) = self.docs.get_mut(self.active) {
									let id = id.clone();
									d.doc.lock().edit("Depth", |t| {
										if let Some(m) = t.measurements.iter_mut().find(|m| m.id == id) {
											m.depth = Some(v);
										}
									});
								}
								keep = false;
							}
						}
						if widgets::secondary_button(ui, p, "Skip").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Split { mode, every, ranges, error } => {
					ui.horizontal(|ui| {
						ui.selectable_value(mode, 0, "Every N pages");
						ui.selectable_value(mode, 1, "Page ranges");
						ui.selectable_value(mode, 2, "By bookmarks");
					});
					ui.add_space(6.0);
					match *mode {
						0 => {
							ui.horizontal(|ui| {
								ui.label("Pages per file");
								ui.add(egui::DragValue::new(every).range(1..=10_000));
							});
						}
						1 => {
							ui.add(egui::TextEdit::singleline(ranges).hint_text("e.g. 1-12, 13-40, 41-").desired_width(260.0));
						}
						_ => {
							widgets::hint(ui, p, "One file per top-level bookmark, named after it.");
						}
					}
					if let Some(e) = error {
						ui.label(RichText::new(e.as_str()).color(p.danger));
					}
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Choose folder and split").clicked() {
							match self.run_split(*mode, *every, ranges) {
								Ok(n) => {
									self.toast(format!("Wrote {n} files"), false);
									keep = false;
								}
								Err(e) if e.is_empty() => {}
								Err(e) => *error = Some(e),
							}
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Merge { files, error } => {
					widgets::hint(ui, p, "Files are combined in this order. Takeoffs saved next to each file are merged too.");
					let mut remove = None;
					let mut swap = None;
					for (k, f) in files.iter().enumerate() {
						ui.horizontal(|ui| {
							ui.label(RichText::new(format!("{}.", k + 1)).color(p.muted));
							ui.add(egui::Label::new(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).truncate());
							ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
								if ui.small_button(i::X).clicked() {
									remove = Some(k);
								}
								if k + 1 < files.len() && ui.small_button(i::ARROW_DOWN).clicked() {
									swap = Some((k, k + 1));
								}
								if k > 0 && ui.small_button(i::ARROW_UP).clicked() {
									swap = Some((k, k - 1));
								}
							});
						});
					}
					if let Some(k) = remove {
						files.remove(k);
					}
					if let Some((a, b)) = swap {
						files.swap(a, b);
					}
					if ui.button(format!("{}  Add PDFs…", i::PLUS)).clicked() {
						if let Some(more) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_files() {
							files.extend(more);
						}
					}
					if let Some(e) = error {
						ui.label(RichText::new(e.as_str()).color(p.danger));
					}
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if ui.add_enabled(files.len() >= 2, egui::Button::new(RichText::new("Merge and open").color(p.on_accent).font(theme::semibold(13.0))).fill(p.accent_fill)).clicked() {
							match self.run_merge(files) {
								Ok(Some(path)) => {
									self.open(path, ctx);
									keep = false;
								}
								Ok(None) => {}
								Err(e) => *error = Some(e),
							}
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::ConfirmPdfEdit { action } => {
					ui.label(RichText::new("Page tools rewrite the PDF file itself. Your takeoff follows the pages (deleted pages drop their markups, rotated pages turn theirs).").color(p.text2));
					ui.label(RichText::new("A backup of the original is kept next to it the first time.").color(p.text2));
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if widgets::primary_button(ui, p, "Continue").clicked() {
							self.pdf_edits_confirmed = true;
							let a = action.clone();
							keep = false;
							self.dialog = None;
							self.page_action(a, ctx);
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::DeleteMap => {
					ui.label(RichText::new("The map of sheets, details and links is removed from this drawing. The AI stops using it, and mapping the set again starts from nothing.").color(p.text2));
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						if ui.add(egui::Button::new(RichText::new("Delete map").color(Color32::WHITE)).fill(p.danger)).clicked() {
							if let Some(d) = self.docs.get_mut(self.active) {
								d.delete_map();
							}
							keep = false;
						}
						if widgets::secondary_button(ui, p, "Cancel").clicked() {
							keep = false;
						}
					});
				}
				Dialog::Settings => {
					egui::Grid::new("prefs").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
						ui.label("Your name");
						if ui.text_edit_singleline(&mut self.settings.author).changed() {
							for d in &self.docs {
								d.doc.lock().author = self.settings.author.clone();
							}
						}
						ui.end_row();
						ui.label("Mouse wheel");
						ui.horizontal(|ui| {
							ui.selectable_value(&mut self.settings.wheel_zooms, true, "Zooms").on_hover_text("A trackpad always pans with two fingers; pinch to zoom");
							ui.selectable_value(&mut self.settings.wheel_zooms, false, "Scrolls (pinch / ⌘ to zoom)");
						});
						ui.end_row();
						ui.label("Snap to lines");
						ui.checkbox(&mut self.settings.snap, "On by default (S toggles, Alt bypasses)");
						ui.end_row();
						ui.label("llama-server");
						ui.add(egui::TextEdit::singleline(&mut self.settings.llama_server).hint_text("auto-detect").desired_width(240.0));
						ui.end_row();
					});
					ui.add_space(10.0);
					if widgets::primary_button(ui, p, "Done").clicked() {
						if !self.settings.llama_server.trim().is_empty() {
							std::env::set_var("TAKEOFF_LLAMA_SERVER", self.settings.llama_server.trim());
						}
						self.settings.save();
						keep = false;
					}
				}
				Dialog::Shortcuts => {
					egui::Grid::new("keys").num_columns(4).spacing([18.0, 6.0]).striped(true).show(ui, |ui| {
						let mut col = 0;
						for t in Tool::all() {
							if let Some(k) = t.shortcut() {
								ui.label(RichText::new(k.to_string()).monospace().font(theme::semibold(12.0)).color(p.accent));
								ui.label(t.label());
								col += 1;
								if col % 2 == 0 {
									ui.end_row();
								}
							}
						}
						ui.end_row();
						for (k, v) in [
							("Space", "Pan while held"),
							("S", "Toggle snapping"),
							("Shift", "Lock to 0/45/90°"),
							("Alt", "Bypass snapping"),
							("Enter", "Finish shape"),
							("Backspace", "Remove last point"),
							("Esc", "Cancel / Select tool"),
							("Del", "Delete selection"),
							("⌘Z / ⇧⌘Z", "Undo / Redo"),
							("⌘C ⌘V ⌘D", "Copy, paste, duplicate"),
							("⌘F", "Search text"),
							("⌘J", "Ask the AI"),
							("⌘G", "Page / Graph view"),
							("⌥← / ⌥→", "Back / forward along the map"),
							("PgUp / PgDn", "Previous / next page"),
							("⌘0", "Fit page"),
						] {
							ui.label(RichText::new(k).monospace().font(theme::semibold(12.0)).color(p.accent));
							ui.label(v);
							col += 1;
							if col % 2 == 0 {
								ui.end_row();
							}
						}
					});
					ui.add_space(10.0);
					if widgets::primary_button(ui, p, "Close").clicked() {
						keep = false;
					}
				}
				Dialog::About => {
					ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
					ui.label(RichText::new("Open-source takeoff with a native AI agent, by construction.live.").color(p.text2));
					ui.label(RichText::new("Apache License 2.0. Includes pdfium (BSD), Inter (OFL), Lucide icons (ISC).").size(12.0).color(p.muted));
					ui.hyperlink_to("construction.live", "https://construction.live");
					ui.add_space(10.0);
					if widgets::primary_button(ui, p, "Close").clicked() {
						keep = false;
					}
				}
			}
		});
		if resp.should_close() {
			keep = false;
		}
		if keep && self.dialog.is_none() {
			self.dialog = Some(dialog);
		}
	}

	fn toasts(&mut self, ctx: &egui::Context, p: &Palette) {
		let Some((msg, at, error)) = &self.toast else { return };
		let age = at.elapsed().as_secs_f32();
		if age > 4.0 {
			self.toast = None;
			return;
		}
		ctx.request_repaint_after(std::time::Duration::from_millis(100));
		egui::Area::new(egui::Id::new("toast")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -48.0]).order(egui::Order::Tooltip).show(ctx, |ui| {
			egui::Frame::popup(&ctx.global_style()).fill(if *error { p.danger } else { p.panel }).corner_radius(6).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
				ui.label(RichText::new(msg).color(if *error { Color32::WHITE } else { p.text }).size(13.0));
			});
		});
	}

	// ── Page tools ──────────────────────────────────────────────────────

	fn page_action(&mut self, action: PageAction, ctx: &egui::Context) {
		match action {
			PageAction::Split => {
				self.dialog = Some(Dialog::Split { mode: 0, every: 1, ranges: String::new(), error: None });
				return;
			}
			PageAction::Merge => {
				let files = self.docs.get(self.active).map(|d| vec![d.path.clone()]).unwrap_or_default();
				self.dialog = Some(Dialog::Merge { files, error: None });
				return;
			}
			PageAction::Extract { pages } => {
				let Some(d) = self.docs.get(self.active) else { return };
				let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
				let name = if pages.len() == 1 { format!("{stem} p{}.pdf", pages[0]) } else { format!("{stem} extract.pdf") };
				let Some(dst) = rfd::FileDialog::new().set_file_name(&name).add_filter("PDF", &["pdf"]).save_file() else { return };
				let src = d.path.clone();
				let data = d.doc.lock().data.clone();
				match takeoff_pdf::ops::extract(&src, &data, &pages, &dst) {
					Ok(()) => {
						self.toast(format!("Extracted {} page(s) to {}", pages.len(), dst.file_name().unwrap_or_default().to_string_lossy()), false);
					}
					Err(e) => self.toast(e.to_string(), true),
				}
				return;
			}
			_ => {}
		}
		if !self.pdf_edits_confirmed {
			self.dialog = Some(Dialog::ConfirmPdfEdit { action });
			return;
		}
		let Some(d) = self.docs.get_mut(self.active) else { return };
		let path = d.path.clone();
		if let Err(e) = takeoff_pdf::ops::backup_once(&path) {
			self.toast(format!("Couldn't back up the PDF: {e}"), true);
			return;
		}
		let n = d.page_count();
		let picked_file;
		let order: Vec<u32>;
		let edit = match &action {
			PageAction::Rotate { pages, quarter_turns } => takeoff_pdf::ops::Edit::Rotate(pages, *quarter_turns),
			PageAction::Delete { pages } => takeoff_pdf::ops::Edit::Delete(pages),
			PageAction::InsertBlank { at } => {
				let size = d.page_size(d.page);
				takeoff_pdf::ops::Edit::InsertBlank { at: *at, size_pt: (size.0 as f32, size.1 as f32) }
			}
			PageAction::InsertFile { at } => {
				let Some(other) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() else { return };
				picked_file = other;
				takeoff_pdf::ops::Edit::InsertFile { at: *at, other: &picked_file }
			}
			PageAction::Move { from, to } => {
				let mut o: Vec<u32> = (1..=n).filter(|p| p != from).collect();
				o.insert((*to as usize - 1).min(o.len()), *from);
				order = o;
				takeoff_pdf::ops::Edit::Reorder(&order)
			}
			_ => return,
		};
		let mut data = d.doc.lock().data.clone();
		let result = takeoff_pdf::ops::edit_in_place(&path, &mut data, edit).map_err(|e| e.to_string());
		if result.is_ok() {
			d.doc.lock().edit("Page edit", |t| *t = data);
		}
		match result {
			Ok(msg) => {
				d.save_now();
				d.reload_pdf();
				self.left.picked.clear();
				self.toast(msg, false);
			}
			Err(e) if e.is_empty() => {}
			Err(e) => self.toast(e, true),
		}
		let _ = ctx;
	}

	fn run_split(&mut self, mode: usize, every: u32, ranges: &str) -> Result<usize, String> {
		let d = self.docs.get(self.active).ok_or("No document")?;
		let n = d.page_count();
		let mode = match mode {
			0 => SplitMode::EveryN(every),
			1 => {
				let mut out = vec![];
				for part in ranges.split(',').map(str::trim).filter(|s| !s.is_empty()) {
					let pages = takeoff_core::pages::parse_page_ranges(part, n)?;
					out.push((*pages.iter().min().unwrap(), *pages.iter().max().unwrap()));
				}
				if out.is_empty() {
					return Err("Type at least one range".into());
				}
				SplitMode::Ranges(out)
			}
			_ => SplitMode::Bookmarks,
		};
		let Some(dir) = rfd::FileDialog::new().set_title("Folder for the split files").pick_folder() else { return Err(String::new()) };
		let data = d.doc.lock().data.clone();
		let written = takeoff_pdf::ops::split(&d.path, &data, &mode, &dir).map_err(|e| e.to_string())?;
		let _ = open::that(&dir);
		Ok(written.len())
	}

	fn run_merge(&mut self, files: &[PathBuf]) -> Result<Option<PathBuf>, String> {
		let Some(dst) = rfd::FileDialog::new().set_file_name("Merged.pdf").add_filter("PDF", &["pdf"]).save_file() else { return Ok(None) };
		let docs = &self.docs;
		takeoff_pdf::ops::merge(files, &dst, |f| docs.iter().find(|d| d.path == f).map(|d| d.doc.lock().data.clone())).map_err(|e| e.to_string())?;
		Ok(Some(dst))
	}

	// ── Exports ─────────────────────────────────────────────────────────

	fn export_pdf(&mut self) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{stem} (marked up).pdf")).add_filter("PDF", &["pdf"]).save_file() else { return };
		let data = d.doc.lock().data.clone();
		match takeoff_pdf::export::export_flattened(&d.path, &data, &dst) {
			Ok(n) => self.toast(format!("Exported {n} marked-up page(s)"), false),
			Err(e) => self.toast(e.to_string(), true),
		}
	}

	fn export_png(&mut self) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{stem} p{}.png", d.page)).add_filter("PNG", &["png"]).save_file() else { return };
		let data = d.doc.lock().data.clone();
		let page = d.page;
		let res = (|| -> Result<(), String> {
			let mut dr = takeoff_pdf::Drawing::open(&d.path).map_err(|e| e.to_string())?;
			dr.set_hidden_layers(&data.hidden_layers.iter().cloned().collect()).map_err(|e| e.to_string())?;
			let mut r = dr.render(page, 200.0, takeoff_pdf::FracRect::FULL, Some(8000)).map_err(|e| e.to_string())?;
			takeoff_pdf::burn::burn(&mut r.image, r.region, d.page_size(page), &data, page, &takeoff_pdf::burn::BurnOptions::default());
			r.image.save(&dst).map_err(|e| e.to_string())
		})();
		match res {
			Ok(()) => self.toast("Exported page image", false),
			Err(e) => self.toast(e, true),
		}
	}

	fn export_csv(&mut self, summary: bool) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let name = if summary { format!("{stem} quantities.csv") } else { format!("{stem} markups.csv") };
		let Some(dst) = rfd::FileDialog::new().set_file_name(name).add_filter("CSV", &["csv"]).save_file() else { return };
		let doc = d.doc.lock();
		let csv = if summary { takeoff_core::list::summary_csv(&takeoff_core::list::summary(&doc)) } else { takeoff_core::list::rows_csv(&doc, &takeoff_core::list::rows(&doc)) };
		drop(doc);
		match std::fs::write(&dst, csv) {
			Ok(()) => self.toast("Exported CSV", false),
			Err(e) => self.toast(e.to_string(), true),
		}
	}

	fn export_json(&mut self) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{stem}.takeoff.json")).add_filter("JSON", &["json"]).save_file() else { return };
		let data = d.doc.lock().data.clone();
		match takeoff_core::sidecar::save_to(&dst, &data) {
			Ok(()) => self.toast("Exported takeoff file", false),
			Err(e) => self.toast(e.to_string(), true),
		}
	}

	fn send_feedback(&mut self) {
		let url = updates::feedback_mailto(&self.settings.model);
		if open::that(&url).is_err() {
			self.toast("No email app found — write to rahul@construction.live", true);
		}
	}

	fn update_banner(&mut self, ui: &mut egui::Ui, p: &Palette) {
		let Some(rel) = self.updater.available(&self.settings.skipped_update) else { return };
		if self.updater.dismissed && !rel.critical {
			return;
		}
		let ctx = ui.ctx().clone();
		let state = self.updater.download_state();
		let fill = if rel.critical { Color32::from_rgb(0x7F, 0x1D, 0x1D) } else if p.dark { Color32::from_rgb(0x2A, 0x1C, 0x14) } else { Color32::from_rgb(0xFF, 0xF1, 0xEA) };
		egui::Panel::top("update-banner").frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, p.border))).show(ui, |ui| {
			ui.horizontal(|ui| {
				ui.label(RichText::new(if rel.critical { i::CIRCLE_ALERT } else { i::SPARKLES }).font(theme::icon_font(15.0)).color(if rel.critical { Color32::WHITE } else { p.accent }));
				let title = if rel.critical { format!("Important update: Takeoff Desk {}", rel.version) } else { format!("Takeoff Desk {} is available", rel.version) };
				ui.label(RichText::new(title).font(theme::semibold(13.0)).color(if rel.critical { Color32::WHITE } else { p.text }));
				ui.label(RichText::new(format!("you have {}", env!("CARGO_PKG_VERSION"))).size(12.0).color(if rel.critical { Color32::from_gray(220) } else { p.text2 }));
				let first = rel.notes.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches(['-', '*', '#', ' ']).to_string();
				if !first.is_empty() {
					ui.add(egui::Label::new(RichText::new(format!("· {first}")).size(12.0).color(if rel.critical { Color32::from_gray(230) } else { p.text2 })).truncate());
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if !rel.critical && ui.small_button("Skip this version").clicked() {
						self.settings.skipped_update = rel.version.clone();
						self.settings.save();
					}
					if !rel.critical && ui.small_button("Later").clicked() {
						self.updater.dismissed = true;
					}
					if !rel.notes.trim().is_empty() && ui.small_button("What's new").clicked() {
						self.notes_open = true;
					}
					match &state {
						DownloadState::Downloading { done, total } => {
							let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
							ui.add(egui::ProgressBar::new(frac).desired_width(160.0).text(format!("{:.0} / {:.0} MB", *done as f64 / 1e6, *total as f64 / 1e6)).fill(p.accent_fill));
						}
						DownloadState::Done(path) => {
							if ui.add(egui::Button::new(RichText::new(format!("{}  Open installer", i::EXTERNAL_LINK)).color(p.on_accent)).fill(p.accent_fill)).on_hover_text(path.display().to_string()).clicked() {
								let _ = open::that(path);
							}
							ui.label(RichText::new("Downloaded — quit Takeoff Desk, then install").size(12.0).color(p.text2));
						}
						DownloadState::Failed(e) => {
							if ui.button("Open download page").clicked() {
								let _ = open::that(if rel.page.is_empty() { updates::RELEASES_PAGE } else { &rel.page });
							}
							ui.label(RichText::new("Download failed").color(p.danger)).on_hover_text(e);
						}
						DownloadState::Idle => {
							if ui.add(egui::Button::new(RichText::new(format!("{}  Download & install", i::DOWNLOAD)).color(p.on_accent).font(theme::semibold(12.5))).fill(p.accent_fill).corner_radius(4)).clicked() {
								self.updater.download_and_open(&rel, &ctx);
							}
						}
					}
				});
			});
		});
		if self.notes_open {
			let mut open = true;
			egui::Window::new(format!("What's new in {}", rel.version)).open(&mut open).collapsible(false).resizable(true).default_width(460.0).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(&ctx, |ui| {
				if !rel.date.is_empty() {
					ui.label(RichText::new(&rel.date).color(p.muted).size(12.0));
				}
				egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| crate::ai_panel::markdown(ui, p, &rel.notes));
			});
			self.notes_open = open;
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
	pub fn debug_session(&mut self, path: &std::path::PathBuf) -> std::sync::Arc<parking_lot::Mutex<takeoff_agent::ChatSession>> {
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
		for tok in inch_str.split(|c: char| c == ' ' || c == '-').filter(|x| !x.is_empty()) {
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
