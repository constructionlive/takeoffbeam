//! Text search with hit rectangles in page fractions (for highlights and
//! "Search" panel results), and positioned text for the agent's read_text.

use pdfium_render::prelude::*;

use crate::vectors::PageAffine;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
	pub page: u32,
	/// Union of the hit's segments (x, y, w, h) in page fractions.
	pub rect: (f64, f64, f64, f64),
	/// The surrounding words, for the results list.
	pub context: String,
}

pub fn search_page(page: &PdfPage, page_no: u32, query: &str, match_case: bool, whole_word: bool, limit: usize) -> Vec<Hit> {
	let Ok(text) = page.text() else { return vec![] };
	let Ok(affine) = PageAffine::for_page(page) else { return vec![] };
	let opts = PdfSearchOptions::new().match_case(match_case).match_whole_word(whole_word);
	let Ok(search) = text.search(query, &opts) else { return vec![] };
	let mut hits = Vec::new();
	for segments in search.iter(PdfSearchDirection::SearchForward) {
		let mut rect: Option<(f64, f64, f64, f64)> = None;
		for seg in segments.iter() {
			let b = seg.bounds();
			let p1 = affine.apply(b.left().value as f64, b.top().value as f64);
			let p2 = affine.apply(b.right().value as f64, b.bottom().value as f64);
			let (x0, y0, x1, y1) = (p1.x.min(p2.x), p1.y.min(p2.y), p1.x.max(p2.x), p1.y.max(p2.y));
			rect = Some(match rect {
				None => (x0, y0, x1, y1),
				Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
			});
		}
		if let Some((x0, y0, x1, y1)) = rect {
			let pad_w = (x1 - x0) * 1.5 + 0.02;
			let ctx_rect = (x0 - pad_w, y0, (x1 - x0) + 2.0 * pad_w, y1 - y0);
			hits.push(Hit { page: page_no, rect: (x0, y0, x1 - x0, y1 - y0), context: context_text(page, &text, ctx_rect) });
		}
		if hits.len() >= limit {
			break;
		}
	}
	hits
}

fn context_text(page: &PdfPage, text: &PdfPageText, r: (f64, f64, f64, f64)) -> String {
	let (w, h) = (page.width().value, page.height().value);
	// Context is best-effort and assumes an unrotated page.
	let rect = PdfRect::new_from_values(
		h * (1.0 - (r.1 + r.3) as f32),
		w * r.0.max(0.0) as f32,
		h * (1.0 - r.1 as f32),
		w * (r.0 + r.2).min(1.0) as f32,
	);
	text.inside_rect(rect).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A run of text with where it sits, for the agent (title blocks, schedules, notes).
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
	pub text: String,
	pub rect: (f64, f64, f64, f64),
}

/// Text grouped into lines with positions, optionally only inside `region` (fractions).
pub fn text_runs(page: &PdfPage, region: Option<(f64, f64, f64, f64)>, limit: usize) -> Vec<TextRun> {
	let Ok(text) = page.text() else { return vec![] };
	let Ok(affine) = PageAffine::for_page(page) else { return vec![] };
	let mut out = Vec::new();
	for seg in text.segments().iter() {
		let b = seg.bounds();
		let p1 = affine.apply(b.left().value as f64, b.top().value as f64);
		let p2 = affine.apply(b.right().value as f64, b.bottom().value as f64);
		let rect = (p1.x.min(p2.x), p1.y.min(p2.y), (p1.x - p2.x).abs(), (p1.y - p2.y).abs());
		if let Some((rx, ry, rw, rh)) = region {
			let (cx, cy) = (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
			if cx < rx || cx > rx + rw || cy < ry || cy > ry + rh {
				continue;
			}
		}
		let t = seg.text();
		let t = t.trim();
		if t.is_empty() {
			continue;
		}
		out.push(TextRun { text: t.to_string(), rect });
		if out.len() >= limit {
			break;
		}
	}
	out
}
