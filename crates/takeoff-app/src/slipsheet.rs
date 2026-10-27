//! Slip-sheet review: the pages of a newer revision, each matched to the
//! sheet it replaces (by sheet number, else page order), for the user to
//! check, change or skip before the swap. Works on one drawing or on every
//! drawing of a set. Reading the sheets and applying run off the UI thread.

use std::path::PathBuf;

use crossbeam_channel::{unbounded, Receiver};
use eframe::egui::{self, Color32, RichText};
use takeoff_core::sheets::{match_sheets, MatchBy, SheetKey};
use takeoff_core::slip::aspect_differs;
use takeoff_core::TakeoffData;
use takeoff_pdf::slip::{Placement, SlipOutcome, SlipPage};

use crate::docstate::OpenDoc;
use crate::icons as i;
use crate::theme::{self, Palette};
use crate::widgets;

/// A sheet a new page can replace.
#[derive(Debug, Clone)]
pub struct Target {
	pub doc: usize,
	pub key: SheetKey,
}

/// What happens to one new page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
	Skip,
	Replace(usize),
	/// Add it at the end of this document.
	Append(usize),
}

#[derive(Debug, Clone)]
pub struct Row {
	pub file: PathBuf,
	pub key: SheetKey,
	pub choice: Choice,
	pub by: Option<MatchBy>,
}

type Indexed = Result<(Vec<Target>, Vec<(PathBuf, SheetKey)>), String>;

/// What one document's swap came to: its new takeoff, or why it failed.
pub type Applied = (PathBuf, Result<(TakeoffData, SlipOutcome), String>);

enum Stage {
	Reading(Receiver<Indexed>),
	Review,
	Applying(Receiver<Vec<Applied>>),
}

pub struct SlipReview {
	/// The drawings whose sheets can be replaced (one, or the set's).
	pub docs: Vec<PathBuf>,
	pub doc_names: Vec<String>,
	pub files: Vec<PathBuf>,
	pub targets: Vec<Target>,
	pub rows: Vec<Row>,
	pub error: Option<String>,
	stage: Stage,
}

pub enum ReviewEvent {
	Cancel,
	/// Swap these pages in, per document.
	Apply(Vec<(PathBuf, Vec<SlipPage>)>),
}

impl SlipReview {
	/// Start reading the drawings' sheets and the new files'.
	pub fn new(docs: Vec<PathBuf>, doc_names: Vec<String>, files: Vec<PathBuf>) -> Self {
		let (tx, rx) = unbounded();
		let (d, f) = (docs.clone(), files.clone());
		std::thread::Builder::new()
			.name("takeoff-slip-index".into())
			.spawn(move || {
				let run = || -> Indexed {
					let mut targets = Vec::new();
					for (k, p) in d.iter().enumerate() {
						let keys = takeoff_pdf::sheets::index(p).map_err(|e| format!("{}: {e}", p.display()))?;
						targets.extend(keys.into_iter().map(|key| Target { doc: k, key }));
					}
					let mut incoming = Vec::new();
					for p in &f {
						let keys = takeoff_pdf::sheets::index(p).map_err(|e| format!("{}: {e}", p.display()))?;
						incoming.extend(keys.into_iter().map(|k| (p.clone(), k)));
					}
					Ok((targets, incoming))
				};
				let _ = tx.send(run());
			})
			.expect("spawn slip-sheet reader");
		Self { docs, doc_names, files, targets: vec![], rows: vec![], error: None, stage: Stage::Reading(rx) }
	}

	pub fn reading(&self) -> bool {
		matches!(self.stage, Stage::Reading(_))
	}

	pub fn applying(&self) -> bool {
		matches!(self.stage, Stage::Applying(_))
	}

	/// Sheets read: match them. Returns the swaps once applied.
	pub fn poll(&mut self) -> Option<Vec<Applied>> {
		match &self.stage {
			Stage::Reading(rx) => {
				let r = rx.try_recv().ok()?;
				self.stage = Stage::Review;
				match r {
					Ok((targets, incoming)) => {
						let keys: Vec<SheetKey> = targets.iter().map(|t| t.key.clone()).collect();
						let inc: Vec<SheetKey> = incoming.iter().map(|(_, k)| k.clone()).collect();
						let m = match_sheets(&keys, &inc);
						self.rows = incoming.into_iter().zip(m).map(|((file, key), m)| Row { file, key, choice: m.target.map(Choice::Replace).unwrap_or(Choice::Skip), by: m.by }).collect();
						self.targets = targets;
					}
					Err(e) => self.error = Some(e),
				}
				None
			}
			Stage::Applying(rx) => {
				let r = rx.try_recv().ok()?;
				self.stage = Stage::Review;
				Some(r)
			}
			Stage::Review => None,
		}
	}

	/// The swaps the rows describe, per document.
	pub fn plan(&self) -> Vec<(PathBuf, Vec<SlipPage>)> {
		let mut out: Vec<(PathBuf, Vec<SlipPage>)> = self.docs.iter().map(|d| (d.clone(), vec![])).collect();
		for r in &self.rows {
			let (doc, to) = match r.choice {
				Choice::Skip => continue,
				Choice::Replace(t) => (self.targets[t].doc, Placement::Replace(self.targets[t].key.page)),
				Choice::Append(d) => (d, Placement::Append),
			};
			out[doc].1.push(SlipPage { file: r.file.clone(), page: r.key.page, to, sheet: r.key.number.clone() });
		}
		out.retain(|(_, p)| !p.is_empty());
		out
	}

	/// Two new pages on one sheet.
	fn clash(&self) -> Option<String> {
		let mut seen = std::collections::HashSet::new();
		for r in &self.rows {
			if let Choice::Replace(t) = r.choice {
				if !seen.insert(t) {
					return Some(format!("Two pages replace {}", self.target_name(t)));
				}
			}
		}
		None
	}

	fn target_name(&self, t: usize) -> String {
		let tg = &self.targets[t];
		let mut page = format!("p{}", tg.key.page);
		if let Some(n) = &tg.key.number {
			page.push_str(&format!(" · {n}"));
		}
		if let Some(title) = tg.key.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
			page.push_str(&format!(" {title}"));
		}
		if self.docs.len() > 1 { format!("{} · {page}", self.doc_names[tg.doc]) } else { page }
	}

	fn choice_text(&self, c: Choice) -> String {
		match c {
			Choice::Skip => "Skip".into(),
			Choice::Replace(t) => format!("Replace {}", self.target_name(t)),
			Choice::Append(d) if self.docs.len() > 1 => format!("Add to {} as a new sheet", self.doc_names[d]),
			Choice::Append(_) => "Add as a new sheet".into(),
		}
	}

	/// Swap the pages in, off the UI thread. Each document's PDF is kept
	/// first (for undo) and its takeoff taken as it is now.
	pub fn start_apply(&mut self, jobs: Vec<(PathBuf, Vec<SlipPage>, TakeoffData)>) {
		let (tx, rx) = unbounded();
		std::thread::Builder::new()
			.name("takeoff-slip-apply".into())
			.spawn(move || {
				let out = jobs
					.into_iter()
					.map(|(path, pages, mut data)| {
						let r = takeoff_pdf::slip::slip_sheet(&path, &mut data, &pages).map(|o| (data, o)).map_err(|e| e.to_string());
						(path, r)
					})
					.collect();
				let _ = tx.send(out);
			})
			.expect("spawn slip-sheet");
		self.stage = Stage::Applying(rx);
	}

	pub fn ui(&mut self, ctx: &egui::Context, p: &Palette) -> Option<ReviewEvent> {
		let mut event = None;
		if !matches!(self.stage, Stage::Review) {
			ctx.request_repaint_after(std::time::Duration::from_millis(100));
		}
		let modal = egui::Modal::new(egui::Id::new("slip-sheet")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(18).corner_radius(8));
		let resp = modal.show(ctx, |ui| {
			ui.set_width(720.0);
			ui.label(RichText::new("Slip-sheet a new revision").font(theme::semibold(16.0)));
			ui.add_space(6.0);
			let names: Vec<String> = self.files.iter().map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
			widgets::hint(ui, p, &format!("New pages from {}. Each replaces the sheet with the same number; your takeoff and markups move onto it, and the old page is kept for Compare.", names.join(", ")));
			ui.add_space(8.0);
			match &self.stage {
				Stage::Reading(_) => {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.label(RichText::new("Reading sheet numbers…").color(p.text2));
					});
				}
				Stage::Applying(_) => {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.label(RichText::new("Replacing sheets and checking what changed…").color(p.text2));
					});
				}
				Stage::Review => self.rows_ui(ui, p),
			}
			if let Some(e) = &self.error {
				ui.label(RichText::new(e.as_str()).color(p.danger));
			}
			ui.add_space(10.0);
			ui.horizontal(|ui| {
				let clash = self.clash();
				let plan = self.plan();
				let n: usize = plan.iter().map(|(_, p)| p.len()).sum();
				let ready = matches!(self.stage, Stage::Review) && clash.is_none() && n > 0;
				ui.add_enabled_ui(ready, |ui| {
					if widgets::primary_button(ui, p, "Apply slip-sheet").clicked() {
						event = Some(ReviewEvent::Apply(plan));
					}
				});
				if widgets::secondary_button(ui, p, "Cancel").clicked() && !self.applying() {
					event = Some(ReviewEvent::Cancel);
				}
				if let Some(c) = clash {
					ui.label(RichText::new(c).color(p.danger).size(12.0));
				} else if matches!(self.stage, Stage::Review) {
					ui.label(RichText::new(format!("{n} page{} to swap in · one undo puts everything back", if n == 1 { "" } else { "s" })).color(p.muted).size(12.0));
				}
			});
		});
		if resp.should_close() && event.is_none() && !self.applying() {
			event = Some(ReviewEvent::Cancel);
		}
		event
	}

	fn rows_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
		if self.rows.is_empty() {
			ui.label(RichText::new("The new files have no pages.").color(p.text2));
			return;
		}
		let choices: Vec<Choice> = std::iter::once(Choice::Skip).chain((0..self.targets.len()).map(Choice::Replace)).chain((0..self.docs.len()).map(Choice::Append)).collect();
		let texts: Vec<String> = choices.iter().map(|c| self.choice_text(*c)).collect();
		let multi_file = self.files.len() > 1;
		egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
			egui::Grid::new("slip-rows").num_columns(3).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
				ui.label(RichText::new("New page").font(theme::semibold(12.0)).color(p.muted));
				ui.label(RichText::new("Goes to").font(theme::semibold(12.0)).color(p.muted));
				ui.label(RichText::new("Matched by").font(theme::semibold(12.0)).color(p.muted));
				ui.end_row();
				for k in 0..self.rows.len() {
					let r = &self.rows[k];
					let file = if multi_file { format!("{} ", r.file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) } else { String::new() };
					ui.add(egui::Label::new(RichText::new(format!("{file}p{}  {}", r.key.page, r.key.number.clone().unwrap_or_else(|| "—".into()))).size(12.5)).truncate()).on_hover_text(format!("{}\nsheet number from the {}", r.key.name(), r.key.source.label()));
					let mut choice = r.choice;
					let current = texts[choices.iter().position(|c| *c == choice).unwrap_or(0)].clone();
					egui::ComboBox::from_id_salt(("slip-choice", k)).width(380.0).selected_text(current).show_ui(ui, |ui| {
						for (c, t) in choices.iter().zip(&texts) {
							ui.selectable_value(&mut choice, *c, t);
						}
					});
					let by = self.rows[k].by;
					let warn = match choice {
						Choice::Replace(t) => aspect_differs(self.targets[t].key.size_pt, self.rows[k].key.size_pt),
						_ => false,
					};
					ui.horizontal(|ui| {
						match (choice, by) {
							(Choice::Skip, _) => ui.label(RichText::new("—").color(p.muted).size(12.0)),
							(_, Some(b)) => ui.label(RichText::new(b.label()).color(if b == MatchBy::Number { p.text2 } else { Color32::from_rgb(0xD9, 0x8E, 0x04) }).size(12.0)),
							(_, None) => ui.label(RichText::new("new").color(p.text2).size(12.0)),
						};
						if warn {
							ui.label(RichText::new(i::CIRCLE_ALERT).font(theme::icon_font(13.0)).color(Color32::from_rgb(0xD9, 0x8E, 0x04))).on_hover_text("Different paper shape: the takeoff is scaled to fit the new page. Check it after.");
						}
					});
					ui.end_row();
					if choice != self.rows[k].choice {
						self.rows[k].choice = choice;
						self.rows[k].by = if matches!(choice, Choice::Replace(_)) { Some(MatchBy::Manual) } else { None };
					}
				}
			});
		});
	}
}

/// Keep the drawing's PDF for undo and take its takeoff as it is, before a swap.
pub fn prepare(d: &mut OpenDoc) -> Result<TakeoffData, String> {
	d.versions.before_edit(&d.path).map_err(|e| format!("Couldn't keep a copy of {} for undo: {e}", d.name))?;
	Ok(d.doc.lock().data.clone())
}

/// A swap is done: the new takeoff in as one undo step, the PDF reloaded.
pub fn finish(d: &mut OpenDoc, data: TakeoffData) {
	let state = d.versions.after_edit(&d.path);
	d.doc.lock().edit("Slip-sheet", |t| {
		*t = data;
		t.pdf_state = state;
	});
	d.compare = None;
	d.save_now();
	d.reload_pdf();
}
