//! The app's side of sets and slip-sheets: opening and making set files,
//! the Set tab's actions, the slip-sheet review from start to undoable swap.

use std::path::PathBuf;

use eframe::egui;
use takeoff_core::set::{SetFile, SET_SUFFIX};

use super::TakeoffApp;
use crate::icons as i;
use crate::set_panel::{OpenSet, SetAction};
use crate::slipsheet::{self, ReviewEvent, SlipReview};
use crate::theme::Palette;

impl TakeoffApp {
	/// Open a set file: the Set tab lists its drawings, the first one opens.
	pub fn open_set(&mut self, path: PathBuf, ctx: &egui::Context) {
		match OpenSet::open(path) {
			Ok(s) => {
				if self.docs.is_empty() {
					if let Some(first) = s.paths().into_iter().find(|p| p.exists()) {
						self.open(first, ctx);
					}
				}
				self.set = Some(s);
				self.settings.left_panel = true;
				self.left.tab = 4;
			}
			Err(e) => self.toast(format!("Couldn't open the set: {e}"), true),
		}
	}

	/// A new set from PDFs the user picks, saved where they say.
	fn new_set(&mut self, ctx: &egui::Context) {
		let Some(pdfs) = rfd::FileDialog::new().add_filter("PDF drawings", &["pdf"]).set_title("Drawings for the set (Architectural, Structural, MEP…)").pick_files() else { return };
		let dir = pdfs[0].parent().map(PathBuf::from).unwrap_or_default();
		let Some(mut dst) = rfd::FileDialog::new().set_directory(&dir).set_file_name(format!("Project{SET_SUFFIX}")).add_filter("Takeoff set", &["json"]).save_file() else { return };
		if !takeoff_core::set::is_set_file(&dst) {
			let stem = dst.file_name().map(|n| n.to_string_lossy().trim_end_matches(".json").to_string()).unwrap_or_default();
			dst = dst.with_file_name(format!("{stem}{SET_SUFFIX}"));
		}
		let name = dst.file_name().map(|n| n.to_string_lossy().trim_end_matches(SET_SUFFIX).to_string()).unwrap_or_default();
		let mut set = SetFile::new(name);
		for p in &pdfs {
			set.add(&dst, p);
		}
		match takeoff_core::set::save(&dst, &set) {
			Ok(()) => self.open_set(dst, ctx),
			Err(e) => self.toast(format!("Couldn't save the set: {e}"), true),
		}
	}

	/// File menu entries for sets.
	pub(super) fn set_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
		ui.separator();
		if ui.button(format!("{}  New set…", i::FILES)).on_hover_text("Group several drawings (Architectural, Structural, MEP…) into one project").clicked() {
			ui.close();
			self.new_set(ctx);
		}
		if ui.button(format!("{}  Open set…", i::FOLDER_OPEN)).clicked() {
			ui.close();
			if let Some(f) = rfd::FileDialog::new().add_filter("Takeoff set", &["json"]).set_title("Open a set").pick_file() {
				self.open_set(f, ctx);
			}
		}
		if self.set.is_some() {
			if ui.button(format!("{}  Project takeoff…", i::SIGMA)).clicked() {
				ui.close();
				self.set_action(SetAction::Summary, ctx);
			}
			if ui.button(format!("{}  Close set", i::X)).clicked() {
				ui.close();
				self.set_action(SetAction::Close, ctx);
			}
		}
	}

	pub(super) fn set_action(&mut self, a: SetAction, ctx: &egui::Context) {
		match a {
			SetAction::OpenSheet(path, page) => {
				if !path.exists() {
					self.toast(format!("{} is missing", path.display()), true);
					return;
				}
				self.open(path, ctx);
				if let Some(d) = self.docs.get_mut(self.active) {
					d.view_mode = crate::docstate::ViewMode::Page;
					if d.loaded {
						d.go_to_page(page);
					} else {
						// Just opened: its pages aren't known yet (the page is kept in range on load).
						d.page = page;
					}
				}
			}
			SetAction::AddDrawings => {
				let Some(pdfs) = rfd::FileDialog::new().add_filter("PDF drawings", &["pdf"]).set_title("Add drawings to the set").pick_files() else { return };
				if let Some(Err(e)) = self.set.as_mut().map(|s| s.add(&pdfs)) {
					self.toast(format!("Couldn't save the set: {e}"), true);
				}
			}
			SetAction::SlipSheet => self.start_slip_sheet(true, ctx),
			SetAction::Summary => {
				if let Some(s) = &mut self.set {
					s.summary_open = !s.summary_open;
				}
			}
			SetAction::Close => {
				self.set = None;
				self.left.tab = 0;
			}
			SetAction::ExportSummary(names, lines) => {
				let name = self.set.as_ref().map(|s| s.name()).unwrap_or_default();
				let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{name} quantities.csv")).add_filter("CSV", &["csv"]).save_file() else { return };
				match std::fs::write(&dst, takeoff_core::set::project_csv(&names, &lines)) {
					Ok(()) => self.toast("Exported CSV", false),
					Err(e) => self.toast(e.to_string(), true),
				}
			}
		}
	}

	/// Pick the new revision's PDFs and review the matches: against the set's
	/// drawings (`whole_set`), else the drawing on screen.
	pub(super) fn start_slip_sheet(&mut self, whole_set: bool, ctx: &egui::Context) {
		let Some(files) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_title("Choose the new revision (one PDF or several)").pick_files() else { return };
		self.review_slip_sheet(whole_set, files, ctx);
	}

	/// The review for these new files.
	pub fn review_slip_sheet(&mut self, whole_set: bool, files: Vec<PathBuf>, _ctx: &egui::Context) {
		let (docs, names) = match (&self.set, whole_set) {
			(Some(s), true) => (s.paths(), s.names()),
			_ => match self.docs.get(self.active) {
				Some(d) => (vec![d.path.clone()], vec![d.name.clone()]),
				None => return,
			},
		};
		self.slip = Some(SlipReview::new(docs, names, files));
	}

	/// Each frame: the set's sheet lists, the slip-sheet's progress, messages from documents.
	pub(super) fn poll_sets(&mut self, ctx: &egui::Context) {
		if let Some(s) = &mut self.set {
			s.poll(ctx);
			s.follow(&self.docs);
		}
		let notices: Vec<(String, bool)> = self.docs.iter_mut().filter_map(|d| d.notice.take()).collect();
		for (n, err) in notices {
			self.toast(n, err);
		}
		let Some(done) = self.slip.as_mut().and_then(|r| r.poll()) else { return };
		let mut msgs = Vec::new();
		let mut errors = Vec::new();
		for (path, r) in done {
			match r {
				Ok((data, out)) => {
					if let Some(d) = self.docs.iter_mut().find(|d| d.path == path) {
						slipsheet::finish(d, data);
					}
					let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
					msgs.push(if self.set.is_some() { format!("{name}: {}", out.message()) } else { out.message() });
					errors.extend(out.warnings.iter().map(|w| format!("{name} {w}")));
				}
				Err(e) => errors.push(format!("{}: {e}", path.display())),
			}
		}
		if errors.is_empty() {
			self.slip = None;
			self.toast(msgs.join(" · "), false);
		} else {
			if let Some(r) = &mut self.slip {
				r.error = Some(errors.join("\n"));
			}
			if !msgs.is_empty() {
				self.toast(msgs.join(" · "), false);
			}
		}
	}

	/// The slip-sheet review and the project takeoff window.
	pub(super) fn set_windows(&mut self, ctx: &egui::Context, p: &Palette) {
		if let Some(s) = &mut self.set {
			if let Some(a) = s.summary_ui(ctx, &self.docs, p) {
				self.set_action(a, ctx);
			}
		}
		let Some(ev) = self.slip.as_mut().and_then(|r| r.ui(ctx, p)) else { return };
		match ev {
			ReviewEvent::Cancel => self.slip = None,
			ReviewEvent::Apply(plan) => {
				let mut jobs = Vec::new();
				for (path, pages) in plan {
					self.open(path.clone(), ctx);
					let Some(d) = self.docs.iter_mut().find(|d| d.path == path) else { continue };
					if let Err(e) = takeoff_pdf::ops::backup_once(&path) {
						self.toast(format!("Couldn't back up the PDF: {e}"), true);
						return;
					}
					match slipsheet::prepare(d) {
						Ok(data) => jobs.push((path, pages, data)),
						Err(e) => {
							self.toast(e, true);
							return;
						}
					}
				}
				if let Some(r) = &mut self.slip {
					r.error = None;
					r.start_apply(jobs);
				}
			}
		}
	}

	// ── Test / automation hooks ─────────────────────────────────────────

	#[doc(hidden)]
	pub fn debug_set(&mut self) -> Option<&mut OpenSet> {
		self.set.as_mut()
	}

	#[doc(hidden)]
	pub fn debug_slip(&mut self) -> Option<&mut SlipReview> {
		self.slip.as_mut()
	}

	#[doc(hidden)]
	pub fn debug_docs(&self) -> Vec<PathBuf> {
		self.docs.iter().map(|d| d.path.clone()).collect()
	}
}
