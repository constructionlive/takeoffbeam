//! Slip-sheeting (Bluebeam "replace with a new revision"): pages of a newer
//! revision take the place of the sheets they supersede, in the open PDF.
//! The takeoff stays on its pages (see `takeoff_core::slip`), the old pages
//! go to a history PDF beside the drawing (`plan.pdf.revisions/`, with their
//! takeoff as it was), and each replaced page gets a `sheetRevisions` entry
//! naming the items that sit on parts of the sheet that changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;
use takeoff_core::sheets::{match_sheets, SheetKey, SheetMatch};
use takeoff_core::slip::{aspect_differs, carry_page, items_on_changes};
use takeoff_core::{new_id, SheetRevision, TakeoffData};

use crate::{pdfium, Drawing, PdfError};

/// One page of a new revision, and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct SlipPage {
	pub file: PathBuf,
	/// 1-based page in `file`.
	pub page: u32,
	pub to: Placement,
	/// The sheet number it carries, for the record.
	pub sheet: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
	/// Replace this page (1-based) of the drawing.
	Replace(u32),
	/// A sheet the drawing didn't have: add it at the end.
	Append,
}

#[derive(Debug, Clone, Default)]
pub struct SlipOutcome {
	pub replaced: usize,
	pub added: usize,
	/// The history PDF with the old pages (None when nothing was replaced).
	pub history: Option<PathBuf>,
	/// Items on changed parts of the replaced sheets.
	pub review: usize,
	pub warnings: Vec<String>,
	pub slip: String,
}

impl SlipOutcome {
	pub fn message(&self) -> String {
		let mut parts = Vec::new();
		if self.replaced > 0 {
			parts.push(format!("Replaced {} sheet{}", self.replaced, if self.replaced == 1 { "" } else { "s" }));
		}
		if self.added > 0 {
			parts.push(format!("added {}", self.added));
		}
		if self.review > 0 {
			parts.push(format!("{} item{} to check", self.review, if self.review == 1 { "" } else { "s" }));
		}
		if parts.is_empty() { "Nothing to slip-sheet".into() } else { parts.join(", ") }
	}
}

/// The pages of new revision files, read and matched to the current sheets.
#[derive(Debug, Clone, Default)]
pub struct Proposal {
	pub incoming: Vec<(PathBuf, SheetKey)>,
	pub matches: Vec<SheetMatch>,
}

impl Proposal {
	/// What applying the matches as they stand does; unmatched pages are
	/// added (`add_new`) or skipped.
	pub fn pages(&self, add_new: bool) -> Vec<SlipPage> {
		self.matches
			.iter()
			.filter_map(|m| {
				let (file, key) = &self.incoming[m.incoming];
				let to = match m.target {
					Some(t) => Placement::Replace(t as u32 + 1),
					None if add_new => Placement::Append,
					None => return None,
				};
				Some(SlipPage { file: file.clone(), page: key.page, to, sheet: key.number.clone() })
			})
			.collect()
	}
}

/// Read the new files' sheets and match them to `current` (one drawing's sheets).
pub fn propose(current: &[SheetKey], files: &[PathBuf]) -> Result<Proposal, PdfError> {
	let mut incoming = Vec::new();
	for f in files {
		incoming.extend(crate::sheets::index(f)?.into_iter().map(|k| (f.clone(), k)));
	}
	let keys: Vec<SheetKey> = incoming.iter().map(|(_, k)| k.clone()).collect();
	Ok(Proposal { matches: match_sheets(current, &keys), incoming })
}

/// Where a drawing's earlier revisions are kept: `plan.pdf` → `plan.pdf.revisions/`.
pub fn revisions_dir(pdf: &Path) -> PathBuf {
	let mut name = pdf.file_name().map(|n| n.to_os_string()).unwrap_or_default();
	name.push(".revisions");
	pdf.with_file_name(name)
}

/// A revision entry's history PDF, resolved against the drawing's folder.
pub fn history_path(pdf: &Path, r: &SheetRevision) -> PathBuf {
	takeoff_core::set::resolve(pdf.parent().unwrap_or(Path::new("")), &r.file)
}

/// Swap the new pages into `path` (rewritten in place, atomically) and bring
/// `data` along. The caller saves the takeoff.
pub fn slip_sheet(path: &Path, data: &mut TakeoffData, pages: &[SlipPage]) -> Result<SlipOutcome, PdfError> {
	let mut targets: Vec<u32> = pages.iter().filter_map(|p| match p.to {
		Placement::Replace(t) => Some(t),
		Placement::Append => None,
	}).collect();
	let unique: HashSet<u32> = targets.iter().copied().collect();
	if unique.len() != targets.len() {
		return Err(PdfError::Io("two new pages replace the same sheet".into()));
	}
	targets.sort_unstable();
	let current = Drawing::open(path)?;
	let n = current.page_count();
	if let Some(t) = targets.iter().find(|t| **t == 0 || **t > n) {
		return Err(PdfError::PageOutOfRange(*t, n));
	}
	let old_sizes: HashMap<u32, (f64, f64)> = targets.iter().map(|t| Ok((*t, current.page_size_pt(*t).map(|(w, h)| (w as f64, h as f64))?))).collect::<Result<_, PdfError>>()?;
	drop(current);
	let slip = new_id("sl");
	let mut out = SlipOutcome { slip: slip.clone(), ..Default::default() };

	// The old pages, with their takeoff as it was, before anything changes.
	let history = if targets.is_empty() {
		None
	} else {
		let dir = revisions_dir(path);
		std::fs::create_dir_all(&dir).map_err(|e| PdfError::Io(e.to_string()))?;
		let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		// "2026-10-27 1403"
		let stamp: String = takeoff_core::time::iso_now().chars().take(16).filter(|c| *c != ':').map(|c| if c == 'T' { ' ' } else { c }).collect();
		let mut file = dir.join(format!("{stem} {stamp}.pdf"));
		for k in 2.. {
			if !file.exists() {
				break;
			}
			file = dir.join(format!("{stem} {stamp} ({k}).pdf"));
		}
		// Its own sidecar: the old pages' takeoff (their history paths would point nowhere from there).
		let old = TakeoffData { sheet_revisions: vec![], ..data.clone() };
		crate::ops::extract(path, &old, &targets, &file)?;
		Some(file)
	};

	// Swap the pages in.
	{
		let lib = pdfium()?;
		let mut doc = lib.load_pdf_from_file(path, None)?;
		let mut sources: HashMap<PathBuf, PdfDocument> = HashMap::new();
		for p in pages {
			if !sources.contains_key(&p.file) {
				sources.insert(p.file.clone(), lib.load_pdf_from_file(&p.file, None)?);
			}
			let src = &sources[&p.file];
			let count = src.pages().len() as u32;
			if p.page == 0 || p.page > count {
				return Err(PdfError::PageOutOfRange(p.page, count));
			}
			match p.to {
				Placement::Replace(t) => {
					doc.pages_mut().copy_page_from_document(src, (p.page - 1) as i32, (t - 1) as i32)?;
					doc.pages().get(t as i32)?.delete()?;
					out.replaced += 1;
				}
				Placement::Append => {
					let at = doc.pages().len();
					doc.pages_mut().copy_page_from_document(src, (p.page - 1) as i32, at)?;
					out.added += 1;
				}
			}
		}
		let tmp = path.with_extension("pdf.tmp");
		doc.save_to_file(&tmp)?;
		drop(doc);
		std::fs::rename(&tmp, path).map_err(|e| PdfError::Io(e.to_string()))?;
	}

	// The takeoff onto the new pages, and what to check on them.
	let new = Drawing::open(path)?;
	let old = history.as_ref().map(Drawing::open).transpose()?;
	let rel = history.as_ref().map(|h| h.strip_prefix(path.parent().unwrap_or(Path::new(""))).map(Path::to_path_buf).unwrap_or_else(|_| h.clone()).to_string_lossy().replace('\\', "/"));
	let now = takeoff_core::time::iso_now();
	for p in pages {
		let Placement::Replace(t) = p.to else { continue };
		let from = old_sizes[&t];
		let to = new.page_size_pt(t).map(|(w, h)| (w as f64, h as f64))?;
		carry_page(data, t, from, to);
		let warning = aspect_differs(from, to).then(|| format!("Page size changed ({} → {}): its items were scaled to fit; check them.", inches(from), inches(to)));
		if let Some(w) = &warning {
			out.warnings.push(format!("p{t}: {w}"));
		}
		let file_page = targets.iter().position(|x| *x == t).unwrap() as u32 + 1;
		let (changed, review) = match &old {
			Some(o) => match crate::compare::compare_pages(&new, t, o, file_page, 1600) {
				Ok(diff) => (Some(diff.changed as f64), items_on_changes(data, t, &diff.grid)),
				Err(_) => (None, vec![]),
			},
			None => (None, vec![]),
		};
		out.review += review.len();
		data.sheet_revisions.push(SheetRevision {
			page: t,
			sheet: p.sheet.clone(),
			file: rel.clone().unwrap_or_default(),
			file_page,
			replaced_at: now.clone(),
			from: p.file.file_name().map(|n| n.to_string_lossy().into_owned()),
			from_page: Some(p.page),
			slip: slip.clone(),
			changed,
			review,
			warning,
		});
	}
	out.history = history;
	Ok(out)
}

fn inches(s: (f64, f64)) -> String {
	format!("{:.1}×{:.1} in", s.0 / 72.0, s.1 / 72.0)
}
