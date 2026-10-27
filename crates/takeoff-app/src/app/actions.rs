//! File and page actions: page tools, split and merge, the exports, feedback and logs.

use super::*;

impl TakeoffApp {
	pub(super) fn page_action(&mut self, action: PageAction, ctx: &egui::Context) {
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
			PageAction::SlipSheet => {
				self.start_slip_sheet(false, ctx);
				return;
			}
			PageAction::CompareRevision { page } => {
				if let Some(d) = self.docs.get_mut(self.active) {
					d.go_to_page(page);
					if !d.compare_previous(ctx) {
						self.toast("The previous revision's file is missing", true);
					}
				}
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
		if let Err(e) = d.versions.before_edit(&path) {
			self.toast(format!("Couldn't keep a copy of the PDF for undo: {e}"), true);
			return;
		}
		let mut data = d.doc.lock().data.clone();
		let result = takeoff_pdf::ops::edit_in_place(&path, &mut data, edit).map_err(|e| e.to_string());
		if result.is_ok() {
			let state = d.versions.after_edit(&path);
			d.doc.lock().edit("Page edit", |t| {
				*t = data;
				t.pdf_state = state;
			});
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

	pub(super) fn run_split(&mut self, mode: usize, every: u32, ranges: &str) -> Result<usize, String> {
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

	pub(super) fn run_merge(&mut self, files: &[PathBuf]) -> Result<Option<PathBuf>, String> {
		let Some(dst) = rfd::FileDialog::new().set_file_name("Merged.pdf").add_filter("PDF", &["pdf"]).save_file() else { return Ok(None) };
		let docs = &self.docs;
		takeoff_pdf::ops::merge(files, &dst, |f| docs.iter().find(|d| d.path == f).map(|d| d.doc.lock().data.clone())).map_err(|e| e.to_string())?;
		Ok(Some(dst))
	}

	pub(super) fn export_pdf(&mut self) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{stem} (marked up).pdf")).add_filter("PDF", &["pdf"]).save_file() else { return };
		let data = d.doc.lock().data.clone();
		match takeoff_pdf::export::export_flattened(&d.path, &data, &dst) {
			Ok(n) => self.toast(format!("Exported {n} marked-up page(s)"), false),
			Err(e) => self.toast(e.to_string(), true),
		}
	}

	pub(super) fn export_png(&mut self) {
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

	pub(super) fn export_csv(&mut self, summary: bool) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let name = if summary { format!("{stem} quantities.csv") } else { format!("{stem} markups.csv") };
		let Some(dst) = rfd::FileDialog::new().set_file_name(name).add_filter("CSV", &["csv"]).save_file() else { return };
		let doc = d.doc.lock();
		// With spaces, the summary is per space (a Space column first).
		let csv = match summary {
			true if !doc.data.spaces.is_empty() => takeoff_core::list::space_summary_csv(&takeoff_core::list::summary_by_space(&doc)),
			true => takeoff_core::list::summary_csv(&takeoff_core::list::summary(&doc)),
			false => takeoff_core::list::rows_csv(&doc, &takeoff_core::list::rows(&doc)),
		};
		drop(doc);
		match std::fs::write(&dst, csv) {
			Ok(()) => self.toast("Exported CSV", false),
			Err(e) => self.toast(e.to_string(), true),
		}
	}

	/// Excel: the markups list with a Summary sheet, or the summary alone.
	pub(super) fn export_xlsx(&mut self, summary: bool) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let name = if summary { format!("{stem} quantities.xlsx") } else { format!("{stem} markups.xlsx") };
		let Some(dst) = rfd::FileDialog::new().set_file_name(name).add_filter("Excel workbook", &["xlsx"]).save_file() else { return };
		let doc = d.doc.lock();
		let xlsx = if summary { takeoff_core::list::summary_xlsx(&takeoff_core::list::summary(&doc)) } else { takeoff_core::list::rows_xlsx(&doc, &takeoff_core::list::rows(&doc)) };
		drop(doc);
		match xlsx.and_then(|b| std::fs::write(&dst, b).map_err(|e| e.to_string())) {
			Ok(()) => self.toast("Exported Excel workbook", false),
			Err(e) => self.toast(e, true),
		}
	}

	pub(super) fn export_json(&mut self) {
		let Some(d) = self.docs.get(self.active) else { return };
		let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{stem}.takeoff.json")).add_filter("JSON", &["json"]).save_file() else { return };
		let data = d.doc.lock().data.clone();
		match takeoff_core::sidecar::save_to(&dst, &data) {
			Ok(()) => self.toast("Exported takeoff file", false),
			Err(e) => self.toast(e.to_string(), true),
		}
	}

	pub(super) fn send_feedback(&mut self) {
		let url = updates::feedback_mailto(&self.settings.model);
		if open::that(&url).is_err() {
			self.toast("No email app found — write to rahul@construction.live", true);
		}
	}

	pub(super) fn show_logs(&mut self) {
		let dir = crate::crash::log_dir();
		let _ = std::fs::create_dir_all(&dir);
		if open::that(&dir).is_err() {
			self.toast(format!("Logs are in {}", dir.display()), false);
		}
	}
}
