//! Bookmarks (the PDF outline), flattened with depth for a tree view.

use pdfium_render::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
	pub title: String,
	/// 1-based target page, when the bookmark points into this file.
	pub page: Option<u32>,
	pub depth: u32,
}

pub fn outline(doc: &PdfDocument) -> Vec<Bookmark> {
	let mut out = Vec::new();
	if let Some(root) = doc.bookmarks().root() {
		walk(root, 0, &mut out);
	}
	out
}

fn walk(b: PdfBookmark, depth: u32, out: &mut Vec<Bookmark>) {
	let mut cur = Some(b);
	let mut guard = 0;
	while let Some(bm) = cur {
		guard += 1;
		if guard > 100_000 || out.len() > 100_000 {
			return;
		}
		let page = bm.destination().and_then(|d| d.page_index().ok()).map(|i| i as u32 + 1);
		out.push(Bookmark { title: bm.title().unwrap_or_default(), page, depth });
		if let Some(child) = bm.first_child() {
			if depth < 32 {
				walk(child, depth + 1, out);
			}
		}
		cur = bm.next_sibling();
	}
}
