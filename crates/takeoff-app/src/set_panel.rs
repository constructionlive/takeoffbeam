//! An open set: the Set tab (every sheet of every drawing in the set; a
//! click opens that drawing's tab at the sheet) and the project takeoff
//! window (the summary and the markups of all drawings together). Each
//! drawing's own `.takeoff.json` stays the truth: open drawings are read
//! live, the others from their sidecars.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::{self, RichText, Sense, Vec2};
use egui_extras::{Column, TableBuilder};
use takeoff_core::list::{rows, MarkupRow};
use takeoff_core::set::{self, ProjectLine, SetFile};
use takeoff_core::sheets::SheetKey;
use takeoff_core::Document;

use crate::docstate::OpenDoc;
use crate::icons as i;
use crate::theme::{self, hex, Palette};
use crate::widgets;

/// One drawing of the set, as the Set tab lists it.
pub struct SetDoc {
	pub path: PathBuf,
	pub name: String,
	/// None while being read.
	pub sheets: Option<Result<Vec<SheetKey>, String>>,
	/// Pages with an earlier revision kept (slip-sheeted).
	pub revised: HashSet<u32>,
	/// The open drawing's PDF version the list was read from.
	state: u64,
}

type Read = (usize, Result<Vec<SheetKey>, String>, HashSet<u32>);

pub struct OpenSet {
	pub path: PathBuf,
	pub file: SetFile,
	pub docs: Vec<SetDoc>,
	tx: Sender<Read>,
	rx: Receiver<Read>,
	folded: HashSet<usize>,
	pub summary_open: bool,
	summary_tab: usize,
	/// The project summary and markups, and what they were built from.
	cache: Option<(Vec<Stamp>, Vec<ProjectLine>, Vec<(usize, MarkupRow)>)>,
	closed: HashMap<PathBuf, (Option<SystemTime>, Document)>,
}

/// What a document's numbers came from: an open one's revision, or the sidecar's time.
#[derive(Debug, Clone, PartialEq)]
enum Stamp {
	Open(u64),
	File(Option<SystemTime>),
}

pub enum SetAction {
	/// Open (or switch to) a drawing at a page.
	OpenSheet(PathBuf, u32),
	AddDrawings,
	SlipSheet,
	Summary,
	Close,
	ExportSummary(Vec<String>, Vec<ProjectLine>),
}

impl OpenSet {
	pub fn open(path: PathBuf) -> Result<Self, String> {
		let file = set::load(&path).map_err(|e| e.to_string())?;
		let (tx, rx) = unbounded();
		let mut s = Self { path, file, docs: vec![], tx, rx, folded: HashSet::new(), summary_open: false, summary_tab: 0, cache: None, closed: HashMap::new() };
		s.reload_docs();
		Ok(s)
	}

	pub fn name(&self) -> String {
		self.file.name.clone()
	}

	/// The documents from the set file; read every one's sheets again.
	pub fn reload_docs(&mut self) {
		let paths = self.file.paths(&self.path);
		self.docs = paths.into_iter().enumerate().map(|(k, path)| SetDoc { path, name: self.file.doc_name(k), sheets: None, revised: HashSet::new(), state: 0 }).collect();
		for k in 0..self.docs.len() {
			self.refresh(k);
		}
		self.cache = None;
	}

	/// Read one document's sheets again (after a slip-sheet or a page edit).
	pub fn refresh(&mut self, k: usize) {
		let Some(d) = self.docs.get_mut(k) else { return };
		d.sheets = None;
		let (path, tx) = (d.path.clone(), self.tx.clone());
		std::thread::Builder::new()
			.name("takeoff-set-index".into())
			.spawn(move || {
				let sheets = if path.exists() { takeoff_pdf::sheets::index(&path).map_err(|e| e.to_string()) } else { Err("file not found".into()) };
				let revised = takeoff_core::sidecar::load(&path).map(|t| t.sheet_revisions.iter().map(|r| r.page).collect()).unwrap_or_default();
				let _ = tx.send((k, sheets, revised));
			})
			.expect("spawn set reader");
	}

	/// Read an open drawing's sheets again when its PDF changed (a slip-sheet, a page edit, their undo).
	pub fn follow(&mut self, open: &[OpenDoc]) {
		for k in 0..self.docs.len() {
			let Some(o) = open.iter().find(|o| o.path == self.docs[k].path) else { continue };
			if o.versions.on_disk != self.docs[k].state {
				self.docs[k].state = o.versions.on_disk;
				self.refresh(k);
			}
		}
	}

	/// Add drawings to the set and save it.
	pub fn add(&mut self, pdfs: &[PathBuf]) -> Result<(), String> {
		for p in pdfs {
			self.file.add(&self.path, p);
		}
		set::save(&self.path, &self.file).map_err(|e| e.to_string())?;
		self.reload_docs();
		Ok(())
	}

	pub fn poll(&mut self, ctx: &egui::Context) {
		let mut got = false;
		while let Ok((k, sheets, revised)) = self.rx.try_recv() {
			if let Some(d) = self.docs.get_mut(k) {
				d.sheets = Some(sheets);
				d.revised = revised;
			}
			got = true;
		}
		if got {
			ctx.request_repaint();
		}
		if self.docs.iter().any(|d| d.sheets.is_none()) {
			ctx.request_repaint_after(std::time::Duration::from_millis(150));
		}
	}

	pub fn paths(&self) -> Vec<PathBuf> {
		self.docs.iter().map(|d| d.path.clone()).collect()
	}

	pub fn names(&self) -> Vec<String> {
		self.docs.iter().map(|d| d.name.clone()).collect()
	}

	/// The Set tab: the set's drawings, each with its sheets.
	pub fn ui(&mut self, ui: &mut egui::Ui, open: &[OpenDoc], active: Option<&Path>, p: &Palette) -> Option<SetAction> {
		let mut action = None;
		ui.horizontal(|ui| {
			ui.label(RichText::new(i::FILES).font(theme::icon_font(14.0)).color(p.accent));
			ui.add(egui::Label::new(RichText::new(self.name()).font(theme::semibold(13.0))).truncate()).on_hover_text(self.path.display().to_string());
		});
		ui.horizontal_wrapped(|ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			if widgets::icon_button(ui, p, i::SIGMA, "Project takeoff: every drawing's quantities and markups together", self.summary_open).clicked() {
				action = Some(SetAction::Summary);
			}
			if widgets::icon_button(ui, p, i::REFRESH_CW, "Slip-sheet a new revision into the set…", false).clicked() {
				action = Some(SetAction::SlipSheet);
			}
			if widgets::icon_button(ui, p, i::FILE_PLUS, "Add drawings to the set…", false).clicked() {
				action = Some(SetAction::AddDrawings);
			}
			if widgets::icon_button(ui, p, i::X, "Close the set (its drawings stay open)", false).clicked() {
				action = Some(SetAction::Close);
			}
		});
		let total: usize = self.docs.iter().map(|d| d.sheets.as_ref().and_then(|s| s.as_ref().ok()).map_or(0, Vec::len)).sum();
		widgets::hint(ui, p, &format!("{} drawing{} · {total} sheets · click a sheet to open it", self.docs.len(), if self.docs.len() == 1 { "" } else { "s" }));
		ui.add_space(4.0);
		let width = ui.available_width();
		egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
			ui.set_max_width(width);
			for (k, d) in self.docs.iter().enumerate() {
				let doc = open.iter().find(|o| o.path == d.path);
				let folded = self.folded.contains(&k);
				let head = ui.horizontal(|ui| {
					ui.label(RichText::new(if folded { i::CHEVRON_RIGHT } else { i::CHEVRON_DOWN }).font(theme::icon_font(11.0)).color(p.muted));
					ui.label(RichText::new(i::FILE_TEXT).font(theme::icon_font(12.0)).color(if doc.is_some() { p.accent } else { p.muted }));
					let count = d.sheets.as_ref().and_then(|s| s.as_ref().ok()).map(|s| s.len().to_string()).unwrap_or_default();
					ui.add(egui::Label::new(RichText::new(d.name.to_uppercase()).font(theme::semibold(10.5)).color(p.text2)).truncate());
					ui.label(RichText::new(count).size(11.0).color(p.muted));
				});
				let head = ui.interact(head.response.rect, ui.id().with(("set-doc", k)), Sense::click());
				if head.clicked() {
					if folded {
						self.folded.remove(&k);
					} else {
						self.folded.insert(k);
					}
				}
				head.on_hover_text(d.path.display().to_string());
				if folded {
					continue;
				}
				// An open drawing's history is live (undo takes it away again).
				let live_revised: Option<HashSet<u32>> = doc.map(|o| o.doc.lock().data.sheet_revisions.iter().map(|r| r.page).collect());
				match &d.sheets {
					None => {
						ui.horizontal(|ui| {
							ui.add_space(18.0);
							ui.spinner();
						});
					}
					Some(Err(e)) => {
						ui.label(RichText::new(format!("   {e}")).color(p.danger).size(11.5));
					}
					Some(Ok(sheets)) => {
						for s in sheets {
							let current = active == Some(d.path.as_path()) && doc.is_some_and(|o| o.page == s.page);
							let label = match (&s.number, s.title.as_deref().map(str::trim).filter(|t| !t.is_empty())) {
								(Some(n), Some(t)) => format!("{n}  {t}"),
								(Some(n), None) => n.clone(),
								(None, Some(t)) => format!("p{}  {t}", s.page),
								(None, None) => format!("Page {}", s.page),
							};
							let revised = live_revised.as_ref().unwrap_or(&d.revised).contains(&s.page);
							let (rect, resp) = ui.allocate_exact_size(Vec2::new(width - 4.0, 22.0), Sense::click());
							let painter = ui.painter();
							if current {
								painter.rect_filled(rect, 4.0, p.hover);
								painter.rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(2.5, rect.height())), 1.0, p.accent);
							} else if resp.hovered() {
								painter.rect_filled(rect, 4.0, p.hover);
							}
							let mut job = egui::text::LayoutJob::simple_singleline(label, if current { theme::semibold(12.0) } else { theme::medium(12.0) }, if current { p.text } else { p.text2 });
							job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 48.0).max(20.0));
							let g = painter.layout_job(job);
							painter.galley(rect.left_center() + Vec2::new(18.0, -g.size().y / 2.0), g, p.text);
							if revised {
								painter.text(rect.right_center() - Vec2::new(6.0, 0.0), egui::Align2::RIGHT_CENTER, i::HISTORY, theme::icon_font(11.0), p.muted);
							}
							// Its name for screen readers and tests.
							let name = format!("{} {}", d.name, s.number.clone().unwrap_or_else(|| format!("p{}", s.page)));
							resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
							let resp = resp.on_hover_text(format!("{} · page {}{}", d.name, s.page, if revised { " · revised (Compare with the previous revision)" } else { "" }));
							if resp.clicked() {
								action = Some(SetAction::OpenSheet(d.path.clone(), s.page));
							}
						}
					}
				}
				ui.add_space(6.0);
			}
		});
		action
	}

	/// Each document's takeoff, live when it's open, else from its sidecar.
	fn stamps(&self, open: &[OpenDoc]) -> Vec<Stamp> {
		self.docs
			.iter()
			.map(|d| match open.iter().find(|o| o.path == d.path) {
				Some(o) => Stamp::Open(o.doc.lock().revision()),
				None => Stamp::File(std::fs::metadata(takeoff_core::sidecar::sidecar_path(&d.path)).and_then(|m| m.modified()).ok()),
			})
			.collect()
	}

	fn rebuild(&mut self, open: &[OpenDoc]) {
		let stamps = self.stamps(open);
		if self.cache.as_ref().is_some_and(|(s, _, _)| *s == stamps) {
			return;
		}
		let mut owned: Vec<Document> = Vec::new();
		for (d, st) in self.docs.iter().zip(&stamps) {
			match (open.iter().find(|o| o.path == d.path), st) {
				(Some(o), _) => {
					let g = o.doc.lock();
					owned.push(Document::new(g.data.clone(), g.page_sizes_pt.clone(), ""));
				}
				(None, Stamp::File(t)) => {
					let fresh = self.closed.get(&d.path).is_some_and(|(at, _)| at == t);
					if !fresh {
						self.closed.insert(d.path.clone(), (*t, set::sidecar_document(&d.path)));
					}
					let doc = &self.closed[&d.path].1;
					owned.push(Document::new(doc.data.clone(), doc.page_sizes_pt.clone(), ""));
				}
				(None, Stamp::Open(_)) => owned.push(Document::new(Default::default(), vec![], "")),
			}
		}
		let refs: Vec<&Document> = owned.iter().collect();
		let lines = set::project_summary(&refs);
		let markups = owned.iter().enumerate().flat_map(|(k, d)| rows(d).into_iter().map(move |r| (k, r))).collect();
		self.cache = Some((stamps, lines, markups));
	}

	/// The project takeoff window.
	pub fn summary_ui(&mut self, ctx: &egui::Context, open: &[OpenDoc], p: &Palette) -> Option<SetAction> {
		if !self.summary_open {
			return None;
		}
		self.rebuild(open);
		let mut action = None;
		let mut keep = true;
		let names = self.names();
		egui::Window::new(format!("Project takeoff · {}", self.name())).id(egui::Id::new("project-takeoff")).open(&mut keep).default_size([760.0, 460.0]).resizable(true).collapsible(false).show(ctx, |ui| {
			ui.horizontal(|ui| {
				widgets::tabs(ui, p, &[(i::SIGMA, "Summary"), (i::LIST, "Markups")], &mut self.summary_tab);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if self.summary_tab == 0 && ui.button(format!("{}  Export CSV", i::FILE_SPREADSHEET)).clicked() {
						action = Some(SetAction::ExportSummary(names.clone(), self.cache.as_ref().map(|c| c.1.clone()).unwrap_or_default()));
					}
				});
			});
			ui.separator();
			let Some((_, lines, markups)) = &self.cache else { return };
			if self.summary_tab == 0 {
				if lines.is_empty() {
					widgets::empty_state(ui, p, i::SIGMA, "No takeoff yet", "Quantities from every drawing in the set add up here.");
					return;
				}
				let n = names.len();
				TableBuilder::new(ui).striped(true).resizable(true).column(Column::initial(200.0).at_least(120.0)).columns(Column::initial(96.0).at_least(60.0), n).column(Column::initial(110.0).at_least(70.0)).column(Column::remainder()).header(22.0, |mut h| {
					h.col(|ui| {
						ui.label(RichText::new("Item").font(theme::semibold(12.0)));
					});
					for name in &names {
						h.col(|ui| {
							ui.add(egui::Label::new(RichText::new(name).font(theme::semibold(12.0))).truncate());
						});
					}
					h.col(|ui| {
						ui.label(RichText::new("Total").font(theme::semibold(12.0)));
					});
					h.col(|ui| {
						ui.label(RichText::new("Cost").font(theme::semibold(12.0)));
					});
				}).body(|mut body| {
					for l in lines {
						body.row(22.0, |mut row| {
							row.col(|ui| {
								ui.horizontal(|ui| {
									widgets::swatch(ui, hex(&l.color), 10.0);
									ui.add(egui::Label::new(RichText::new(&l.name).size(12.5)).truncate());
								});
							});
							for d in &l.per_doc {
								row.col(|ui| {
									ui.label(RichText::new(d.as_ref().map(|d| d.total_text.clone()).unwrap_or_else(|| "—".into())).size(12.5).color(if d.is_some() { p.text } else { p.muted }));
								});
							}
							row.col(|ui| {
								let txt = total_text(l);
								let r = ui.label(RichText::new(txt).font(theme::semibold(12.5)));
								if let Some(w) = &l.warning {
									r.on_hover_text(w);
								}
							});
							row.col(|ui| {
								ui.label(RichText::new(l.cost.map(|c| format!("{c:.2}")).unwrap_or_default()).size(12.5).color(p.text2));
							});
						});
					}
				});
			} else {
				if markups.is_empty() {
					widgets::empty_state(ui, p, i::LIST, "No markups yet", "Every drawing's markups list together.");
					return;
				}
				TableBuilder::new(ui).striped(true).resizable(true).sense(Sense::click()).column(Column::initial(140.0)).column(Column::initial(48.0)).column(Column::initial(80.0)).column(Column::initial(180.0)).column(Column::initial(110.0)).column(Column::remainder()).header(22.0, |mut h| {
					for t in ["Drawing", "Page", "Type", "Subject", "Quantity", "Status"] {
						h.col(|ui| {
							ui.label(RichText::new(t).font(theme::semibold(12.0)));
						});
					}
				}).body(|body| {
					body.rows(22.0, markups.len(), |mut row| {
						let (k, r) = &markups[row.index()];
						row.col(|ui| {
							ui.add(egui::Label::new(RichText::new(&names[*k]).size(12.5)).truncate());
						});
						row.col(|ui| {
							ui.label(RichText::new(r.page.to_string()).size(12.5));
						});
						row.col(|ui| {
							ui.label(RichText::new(r.kind.label()).size(12.5).color(p.text2));
						});
						row.col(|ui| {
							ui.horizontal(|ui| {
								widgets::swatch(ui, hex(&r.color), 10.0);
								ui.add(egui::Label::new(RichText::new(&r.subject).size(12.5)).truncate());
							});
						});
						row.col(|ui| {
							ui.label(RichText::new(&r.quantity).size(12.5));
						});
						row.col(|ui| {
							ui.label(RichText::new(&r.status).size(12.5).color(p.text2));
						});
						if row.response().clicked() {
							action = Some(SetAction::OpenSheet(self.docs[*k].path.clone(), r.page));
						}
					});
				});
			}
		});
		if !keep {
			self.summary_open = false;
		}
		action
	}
}

fn total_text(l: &ProjectLine) -> String {
	if l.unit.is_empty() {
		return "—".into();
	}
	let disp = takeoff_core::DisplaySettings::default();
	takeoff_core::units::group_thousands(&takeoff_core::units::format_quantity(l.total, &l.unit, &disp))
}
