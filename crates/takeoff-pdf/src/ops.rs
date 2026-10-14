//! Page tools that keep takeoffs in step: the PDF edit (see `tools`) plus the
//! matching change to the `.takeoff.json` sidecars, so markups follow their
//! pages through extract, split, merge, delete, rotate, insert and reorder.
//! Shared by the app and the CLI.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use takeoff_core::pages::{append_takeoff, delete_map, insert_map, remap_pages, reorder_map, rotate_page_items};
use takeoff_core::sidecar;
use takeoff_core::TakeoffData;

use crate::tools::{self, SplitMode};
use crate::PdfError;

fn load(p: &Path) -> TakeoffData {
	sidecar::load(p).unwrap_or_default()
}

fn save(p: &Path, d: &TakeoffData) -> Result<(), PdfError> {
	sidecar::save(p, d).map_err(|e| PdfError::Io(e.to_string()))
}

fn page_count(p: &Path) -> Result<u32, PdfError> {
	Ok(crate::Drawing::open(p)?.page_count())
}

/// New PDF with `pages` (in order); its sidecar gets those pages' takeoff.
pub fn extract(src: &Path, data: &TakeoffData, pages: &[u32], dst: &Path) -> Result<(), PdfError> {
	tools::extract_pages(src, pages, dst)?;
	let mut t = data.clone();
	remap_pages(&mut t, reorder_map(pages));
	save(dst, &t)
}

/// Split into files; each part's sidecar carries its pages.
pub fn split(src: &Path, data: &TakeoffData, mode: &SplitMode, out_dir: &Path) -> Result<Vec<(PathBuf, u32, u32)>, PdfError> {
	let written = tools::split(src, mode, out_dir)?;
	for (file, a, b) in &written {
		let (a, b) = (*a, *b);
		let mut t = data.clone();
		remap_pages(&mut t, move |p| (p >= a && p <= b).then(|| p - a + 1));
		save(file, &t)?;
	}
	Ok(written)
}

/// Merge files; the result's sidecar joins all their takeoffs.
/// `live` supplies an in-memory takeoff for a path (an open document) over its sidecar.
pub fn merge(inputs: &[PathBuf], dst: &Path, live: impl Fn(&Path) -> Option<TakeoffData>) -> Result<(), PdfError> {
	let starts = tools::merge(inputs, dst)?;
	let mut merged = TakeoffData::default();
	for (f, start) in inputs.iter().zip(&starts) {
		let t = live(f).unwrap_or_else(|| load(f));
		append_takeoff(&mut merged, &t, start - 1);
	}
	save(dst, &merged)
}

/// In-place edits: rewrite `path` and update `data` (the caller saves it).
pub enum Edit<'a> {
	Delete(&'a [u32]),
	Rotate(&'a [u32], u32),
	InsertBlank { at: u32, size_pt: (f32, f32) },
	InsertFile { at: u32, other: &'a Path },
	Reorder(&'a [u32]),
}

/// Keep a one-time backup "<name> (original).pdf" before the first in-place edit.
pub fn backup_once(path: &Path) -> std::io::Result<PathBuf> {
	let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
	let backup = path.with_file_name(format!("{stem} (original).pdf"));
	if !backup.exists() {
		std::fs::copy(path, &backup)?;
	}
	Ok(backup)
}

pub fn edit_in_place(path: &Path, data: &mut TakeoffData, edit: Edit) -> Result<String, PdfError> {
	let n = page_count(path)?;
	Ok(match edit {
		Edit::Delete(pages) => {
			tools::delete_pages(path, pages, path)?;
			let set: HashSet<u32> = pages.iter().copied().collect();
			remap_pages(data, delete_map(n, &set));
			format!("Deleted {} page(s)", pages.len())
		}
		Edit::Rotate(pages, q) => {
			tools::rotate_pages(path, pages, q, path)?;
			for p in pages {
				rotate_page_items(data, *p, q);
			}
			format!("Rotated {} page(s)", pages.len())
		}
		Edit::InsertBlank { at, size_pt } => {
			tools::insert_blank(path, at, 1, size_pt, path)?;
			remap_pages(data, insert_map(at, 1));
			"Inserted a blank page".into()
		}
		Edit::InsertFile { at, other } => {
			let added = tools::insert_from(path, at, other, path)?;
			remap_pages(data, insert_map(at, added));
			append_takeoff(data, &load(other), at.clamp(1, n + 1) - 1);
			format!("Inserted {added} page(s)")
		}
		Edit::Reorder(order) => {
			tools::reorder_pages(path, order, path)?;
			remap_pages(data, reorder_map(order));
			"Reordered pages".into()
		}
	})
}
