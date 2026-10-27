//! Every page's sheet number and title, from what the PDF and its document
//! map know: the map (read by the AI), page labels, the title block's own
//! text, bookmarks. For slip-sheeting and the set's sheet list.

use std::path::Path;

use takeoff_core::sheets::{sheet_number_in_runs, sheet_token, NumberSource, SheetKey, TextBit};

use crate::{Drawing, FracRect, PdfError};

/// Where title blocks keep the sheet number: the right strip, the bottom strip.
const TITLE_BLOCK: [FracRect; 2] = [FracRect { x: 0.75, y: 0.5, w: 0.25, h: 0.5 }, FracRect { x: 0.4, y: 0.85, w: 0.6, h: 0.15 }];

/// The sheets of the PDF at `path`.
pub fn index(path: &Path) -> Result<Vec<SheetKey>, PdfError> {
	index_drawing(&Drawing::open(path)?)
}

pub fn index_drawing(d: &Drawing) -> Result<Vec<SheetKey>, PdfError> {
	let map = takeoff_core::docmap::load(d.path());
	let outline = d.outline();
	let n = d.page_count();
	let mut out = Vec::with_capacity(n as usize);
	for page in 1..=n {
		let (w, h) = d.page_size_pt(page)?;
		let mut key = SheetKey { page, number: None, title: None, size_pt: (w as f64, h as f64), source: NumberSource::None };
		let sheet = map.as_ref().and_then(|m| m.sheet(page));
		if let Some(s) = sheet {
			key.title = Some(s.title.clone()).filter(|t| !t.trim().is_empty());
			if let Some(num) = s.number.clone().filter(|x| !x.trim().is_empty()) {
				key.number = Some(num);
				key.source = NumberSource::Map;
			}
		}
		// The first bookmark on the page: a number in it, and its words as a title.
		let mark = outline.iter().find(|b| b.page == Some(page)).map(|b| b.title.clone());
		if key.number.is_none() {
			if let Some(l) = d.page_label(page).ok().flatten().filter(|l| *l != page.to_string()).and_then(|l| sheet_token(&l)) {
				key.number = Some(l);
				key.source = NumberSource::Label;
			}
		}
		if key.number.is_none() {
			if let Some(num) = title_block_number(d, page) {
				key.number = Some(num);
				key.source = NumberSource::TitleBlock;
			}
		}
		if let Some(m) = &mark {
			let words: Vec<&str> = m.split_whitespace().collect();
			let found = words.iter().position(|w| sheet_token(w).is_some() || w.split_once('-').is_some_and(|(_, t)| sheet_token(t).is_some()));
			if let Some(i) = found {
				let num = sheet_token(words[i]).or_else(|| words[i].split_once('-').and_then(|(_, t)| sheet_token(t)));
				if key.number.is_none() {
					key.number = num.clone();
					key.source = NumberSource::Bookmark;
				}
				// "[1] T1.01 TITLE SHEET": the words after the number are the title.
				if key.title.is_none() && key.number.as_deref().map(takeoff_core::docmap::norm_sheet) == num.as_deref().map(takeoff_core::docmap::norm_sheet) {
					key.title = Some(words[i + 1..].join(" ").trim_start_matches(|c: char| !c.is_alphanumeric()).to_string()).filter(|t| !t.is_empty());
				}
			}
		}
		out.push(key);
	}
	Ok(out)
}

/// The sheet number in the page's title block text, if it has a text layer.
pub fn title_block_number(d: &Drawing, page: u32) -> Option<String> {
	let mut runs = Vec::new();
	for r in TITLE_BLOCK {
		for t in d.text_runs(page, Some(r), 1500).ok()? {
			let bit = TextBit { text: t.text, x: t.rect.0, y: t.rect.1, w: t.rect.2, h: t.rect.3 };
			if !runs.contains(&bit) {
				runs.push(bit);
			}
		}
	}
	sheet_number_in_runs(&runs)
}
