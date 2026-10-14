//! Page tools (Bluebeam "Document" menu): extract, delete, split, merge,
//! rotate, reorder, insert blank, insert from file. Each writes a new file
//! (atomically when it replaces the source), and returns the page map so the
//! caller can keep the takeoff in step (see `takeoff_core::pages`).

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use crate::{pdfium, PdfError};

fn load(path: &Path) -> Result<PdfDocument<'static>, PdfError> {
	Ok(pdfium()?.load_pdf_from_file(path, None)?)
}

fn page_count(doc: &PdfDocument) -> u32 {
	doc.pages().len() as u32
}

/// Save next to `dst` then rename, so a failed save never leaves half a file
/// (and saving over the source while it is open is safe).
fn save_atomic(doc: &PdfDocument, dst: &Path) -> Result<(), PdfError> {
	let tmp = dst.with_extension("pdf.tmp");
	doc.save_to_file(&tmp)?;
	std::fs::rename(&tmp, dst).map_err(|e| PdfError::Io(e.to_string()))?;
	Ok(())
}

/// Copy `pages` (1-based, in the given order) of `src` into a new PDF.
pub fn extract_pages(src: &Path, pages: &[u32], dst: &Path) -> Result<(), PdfError> {
	let source = load(src)?;
	let n = page_count(&source);
	let mut out = pdfium()?.create_new_pdf()?;
	for (i, p) in pages.iter().enumerate() {
		if *p == 0 || *p > n {
			return Err(PdfError::PageOutOfRange(*p, n));
		}
		let idx = (*p - 1) as i32;
		out.pages_mut().copy_page_range_from_document(&source, idx..=idx, i as i32)?;
	}
	save_atomic(&out, dst)
}

/// A copy of `src` without `pages`.
pub fn delete_pages(src: &Path, pages: &[u32], dst: &Path) -> Result<(), PdfError> {
	let source = load(src)?;
	let n = page_count(&source);
	let del: std::collections::HashSet<u32> = pages.iter().copied().collect();
	let keep: Vec<u32> = (1..=n).filter(|p| !del.contains(p)).collect();
	if keep.is_empty() {
		return Err(PdfError::Io("cannot delete every page".into()));
	}
	extract_pages(src, &keep, dst)
}

/// `order[i]` = the old page that becomes page i+1 (may drop or repeat pages).
pub fn reorder_pages(src: &Path, order: &[u32], dst: &Path) -> Result<(), PdfError> {
	extract_pages(src, order, dst)
}

#[derive(Debug, Clone)]
pub enum SplitMode {
	/// One file per `n` pages.
	EveryN(u32),
	/// One file per range (1-based, inclusive).
	Ranges(Vec<(u32, u32)>),
	/// One file per top-level bookmark, named after it.
	Bookmarks,
}

/// Split into several files in `out_dir`, named `{stem}_{part}.pdf` (or the
/// bookmark title). Returns the files written, with their page ranges.
pub fn split(src: &Path, mode: &SplitMode, out_dir: &Path) -> Result<Vec<(PathBuf, u32, u32)>, PdfError> {
	let source = load(src)?;
	let n = page_count(&source);
	let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "split".into());
	let ranges: Vec<(String, u32, u32)> = match mode {
		SplitMode::EveryN(k) => {
			let k = (*k).max(1);
			(0..n.div_ceil(k)).map(|i| (format!("{stem}_{:03}", i + 1), i * k + 1, ((i + 1) * k).min(n))).collect()
		}
		SplitMode::Ranges(r) => r.iter().enumerate().map(|(i, (a, b))| (format!("{stem}_{:03}", i + 1), *a, *b)).collect(),
		SplitMode::Bookmarks => {
			let starts: Vec<(String, u32)> = crate::outline::outline(&source)
				.into_iter()
				.filter(|b| b.depth == 0)
				.filter_map(|b| b.page.map(|p| (b.title, p)))
				.collect();
			if starts.is_empty() {
				return Err(PdfError::Io("this PDF has no bookmarks to split by".into()));
			}
			let mut out = Vec::new();
			for (i, (title, start)) in starts.iter().enumerate() {
				let end = starts.get(i + 1).map(|(_, s)| s.saturating_sub(1)).unwrap_or(n).max(*start);
				out.push((sanitize_file_name(title), *start, end));
			}
			out
		}
	};
	std::fs::create_dir_all(out_dir).map_err(|e| PdfError::Io(e.to_string()))?;
	let mut written = Vec::new();
	for (name, a, b) in ranges {
		if a == 0 || b > n || a > b {
			return Err(PdfError::PageOutOfRange(b, n));
		}
		let mut out = pdfium()?.create_new_pdf()?;
		out.pages_mut().copy_page_range_from_document(&source, (a - 1) as i32..=(b - 1) as i32, 0)?;
		let path = unique_path(&out_dir.join(format!("{name}.pdf")));
		save_atomic(&out, &path)?;
		written.push((path, a, b));
	}
	Ok(written)
}

fn sanitize_file_name(s: &str) -> String {
	let cleaned: String = s.chars().map(|c| if c.is_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
	let t = cleaned.trim().trim_matches('.');
	if t.is_empty() { "part".into() } else { t.chars().take(80).collect() }
}

fn unique_path(p: &Path) -> PathBuf {
	if !p.exists() {
		return p.to_path_buf();
	}
	let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
	for i in 2.. {
		let c = p.with_file_name(format!("{stem} ({i}).pdf"));
		if !c.exists() {
			return c;
		}
	}
	unreachable!()
}

/// Concatenate files into one. Returns each input's first page in the result.
pub fn merge(inputs: &[PathBuf], dst: &Path) -> Result<Vec<u32>, PdfError> {
	let mut out = pdfium()?.create_new_pdf()?;
	let mut starts = Vec::new();
	for input in inputs {
		let doc = load(input)?;
		starts.push(page_count(&out) + 1);
		let at = out.pages().len();
		let n = doc.pages().len();
		if n > 0 {
			out.pages_mut().copy_page_range_from_document(&doc, 0..=n - 1, at)?;
		}
	}
	save_atomic(&out, dst)?;
	Ok(starts)
}

/// Insert all pages of `other` before page `at` (1-based; count+1 = append).
pub fn insert_from(src: &Path, at: u32, other: &Path, dst: &Path) -> Result<u32, PdfError> {
	let mut doc = load(src)?;
	let o = load(other)?;
	let n = o.pages().len();
	let at = at.clamp(1, page_count(&doc) + 1);
	if n > 0 {
		doc.pages_mut().copy_page_range_from_document(&o, 0..=n - 1, (at - 1) as i32)?;
	}
	save_atomic(&doc, dst)?;
	Ok(n as u32)
}

/// Insert `count` blank pages of `size_pt` before page `at`.
pub fn insert_blank(src: &Path, at: u32, count: u32, size_pt: (f32, f32), dst: &Path) -> Result<(), PdfError> {
	let mut doc = load(src)?;
	let at = at.clamp(1, page_count(&doc) + 1);
	for i in 0..count {
		let size = PdfPagePaperSize::Custom(PdfPoints::new(size_pt.0), PdfPoints::new(size_pt.1));
		doc.pages_mut().create_page_at_index(size, (at - 1 + i) as i32)?;
	}
	save_atomic(&doc, dst)
}

/// Turn pages clockwise by 90° × `quarter_turns`.
pub fn rotate_pages(src: &Path, pages: &[u32], quarter_turns: u32, dst: &Path) -> Result<(), PdfError> {
	let doc = load(src)?;
	let n = page_count(&doc);
	for p in pages {
		if *p == 0 || *p > n {
			return Err(PdfError::PageOutOfRange(*p, n));
		}
		let mut page = doc.pages().get((*p - 1) as i32)?;
		let current = match page.rotation()? {
			PdfPageRenderRotation::None => 0,
			PdfPageRenderRotation::Degrees90 => 1,
			PdfPageRenderRotation::Degrees180 => 2,
			PdfPageRenderRotation::Degrees270 => 3,
		};
		page.set_rotation(match (current + quarter_turns) % 4 {
			1 => PdfPageRenderRotation::Degrees90,
			2 => PdfPageRenderRotation::Degrees180,
			3 => PdfPageRenderRotation::Degrees270,
			_ => PdfPageRenderRotation::None,
		});
	}
	save_atomic(&doc, dst)
}
