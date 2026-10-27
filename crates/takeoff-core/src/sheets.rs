//! Sheet numbers: read them off a title block (or a page label, a bookmark,
//! the document map), and match the pages of a new revision to the sheets
//! they replace (slip-sheeting). Text comes in from the PDF side; this only
//! reasons about it.

use crate::docmap::norm_sheet;

/// A run of text on a page, its box in page fractions.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBit {
	pub text: String,
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

/// Where a page's sheet number came from, most trusted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NumberSource {
	Map,
	Label,
	TitleBlock,
	Bookmark,
	None,
}

impl NumberSource {
	pub fn label(self) -> &'static str {
		match self {
			NumberSource::Map => "document map",
			NumberSource::Label => "page label",
			NumberSource::TitleBlock => "title block",
			NumberSource::Bookmark => "bookmark",
			NumberSource::None => "no number",
		}
	}
}

/// One page as far as matching goes.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetKey {
	pub page: u32,
	pub number: Option<String>,
	pub title: Option<String>,
	/// Page size in PDF points, as displayed.
	pub size_pt: (f64, f64),
	pub source: NumberSource,
}

impl SheetKey {
	/// "A-201 · Second floor plan", "A-201", or "p3".
	pub fn name(&self) -> String {
		match (&self.number, self.title.as_deref().map(str::trim).filter(|t| !t.is_empty())) {
			(Some(n), Some(t)) => format!("{n} · {t}"),
			(Some(n), None) => n.clone(),
			(None, Some(t)) => format!("p{} · {t}", self.page),
			(None, None) => format!("p{}", self.page),
		}
	}
}

/// Letter runs that start tokens shaped like sheet numbers but are not:
/// revisions, notes, units, phone and job numbers.
const NOT_DISCIPLINES: [&str; 26] = ["REV", "R", "NO", "NOS", "NUM", "NTS", "SHT", "DWG", "PH", "TEL", "FAX", "FT", "IN", "LF", "SF", "SQ", "EA", "TYP", "MIN", "MAX", "OF", "AT", "BY", "JOB", "PROJ", "ISSUE"];

/// Discipline letters (US National CAD Standard and common variants).
const DISCIPLINES: [&str; 24] = ["A", "AD", "AS", "C", "CS", "D", "E", "ED", "EP", "FP", "G", "GN", "I", "ID", "L", "M", "P", "PL", "Q", "S", "SD", "T", "TC", "X"];

/// The sheet number in a token, when it looks like one: "A-201", "A2.01",
/// "S101", "FP-1.1", "(M-3)". Dates, scales, job numbers and plain numbers don't.
pub fn sheet_token(tok: &str) -> Option<String> {
	let t = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric());
	if t.len() < 2 || t.len() > 12 {
		return None;
	}
	let letters: String = t.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
	if letters.is_empty() || letters.len() > 4 || NOT_DISCIPLINES.contains(&letters.to_ascii_uppercase().as_str()) {
		return None;
	}
	let rest = &t[letters.len()..];
	let rest = rest.strip_prefix(['-', '.', '_', ' ']).unwrap_or(rest);
	if !rest.starts_with(|c: char| c.is_ascii_digit()) {
		return None;
	}
	// Digits with at most two separators, then up to two letters ("A-201a", "S1.01B").
	let body = rest.trim_end_matches(|c: char| c.is_ascii_alphabetic());
	if rest.len() - body.len() > 2 || body.ends_with(['.', '-']) {
		return None;
	}
	let digits = body.chars().filter(char::is_ascii_digit).count();
	let seps = body.chars().filter(|c| matches!(c, '.' | '-')).count();
	if digits == 0 || digits > 6 || seps > 2 || body.chars().any(|c| !c.is_ascii_digit() && c != '.' && c != '-') || body.contains("..") || body.contains("--") {
		return None;
	}
	Some(t.to_ascii_uppercase())
}

/// Every sheet-number-shaped token in a line; "5538-A2.01" gives "A2.01".
fn tokens(line: &str) -> Vec<String> {
	let mut out = Vec::new();
	for w in line.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '/' || c == '|') {
		if let Some(t) = sheet_token(w) {
			out.push(t);
		} else if let Some((_, tail)) = w.split_once('-') {
			// A job-number prefix glued on.
			if let Some(t) = sheet_token(tail) {
				out.push(t);
			}
		}
	}
	out
}

/// A label that says "the sheet number is next to me".
fn is_number_label(s: &str) -> bool {
	let u: String = s.to_ascii_uppercase().chars().filter(|c| c.is_ascii_alphabetic() || *c == ' ').collect();
	let u = u.split_whitespace().collect::<Vec<_>>().join(" ");
	["SHEET NO", "SHEET NUMBER", "SHEET", "SHT NO", "DWG NO", "DRAWING NO", "DRAWING NUMBER", "DRAWING", "SHEET ID"].iter().any(|l| u == *l) || u.starts_with("SHEET NO") || u.starts_with("DRAWING NO") || u.starts_with("DWG NO")
}

fn discipline_bonus(n: &str) -> f64 {
	let letters: String = n.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
	if DISCIPLINES.contains(&letters.as_str()) { 1.0 } else { 0.0 }
}

/// The sheet number in plain title-block text (no positions): the token right
/// after a "SHEET NO." label, else the first with a discipline prefix, else the first.
pub fn sheet_number_in_text(text: &str) -> Option<String> {
	let words: Vec<&str> = text.split_whitespace().collect();
	for i in 0..words.len() {
		for n in 1..=3.min(words.len() - i) {
			let label = words[i..i + n].join(" ");
			if is_number_label(&label) && (n >= 2 || words[i + n - 1].ends_with([':', '.'])) {
				if let Some(t) = words[i + n..].iter().take(3).find_map(|w| tokens(w).into_iter().next()) {
					return Some(t);
				}
			}
		}
	}
	let all: Vec<String> = text.lines().flat_map(tokens).collect();
	all.iter().find(|t| discipline_bonus(t) > 0.0).or(all.first()).cloned()
}

/// The sheet number among a title block's text runs: next to a "SHEET NO."
/// label, in big type, on its own, towards the bottom-right corner (where
/// title blocks keep it), with a discipline prefix. Detail callouts naming
/// other sheets are small, so they lose.
pub fn sheet_number_in_runs(runs: &[TextBit]) -> Option<String> {
	let max_h = runs.iter().map(|r| r.h).fold(0.0_f64, f64::max);
	if max_h <= 0.0 {
		return None;
	}
	let labels: Vec<&TextBit> = runs.iter().filter(|r| is_number_label(&r.text)).collect();
	let mut best: Option<(f64, String)> = None;
	for r in runs {
		let toks = tokens(&r.text);
		let alone = toks.len() == 1 && sheet_token(r.text.trim()).is_some();
		for t in toks {
			let near_label = labels.iter().any(|l| {
				let (dx, dy) = (r.x - l.x, r.y - l.y);
				// Below the label (within a few lines) or to its right on the same line.
				(dx > -0.04 && dx < 0.12 && dy > -0.005 && dy < 0.05) || (dx > 0.0 && dx < 0.2 && dy.abs() < l.h.max(0.004) * 1.5)
			});
			let score = if near_label { 6.0 } else { 0.0 } + 4.0 * r.h / max_h + if alone { 2.0 } else { 0.0 } + (r.x + r.y) / 2.0 + discipline_bonus(&t);
			if best.as_ref().is_none_or(|(s, _)| score > *s) {
				best = Some((score, t));
			}
		}
	}
	best.map(|(_, t)| t)
}

/// How a new page found the sheet it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchBy {
	/// Same sheet number.
	Number,
	/// Same position (no number to go by).
	Order,
	/// Picked by hand in the review.
	Manual,
}

impl MatchBy {
	pub fn label(self) -> &'static str {
		match self {
			MatchBy::Number => "sheet number",
			MatchBy::Order => "page order",
			MatchBy::Manual => "picked",
		}
	}
}

/// For each incoming page (by index), the current sheet it replaces (by index), if any.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetMatch {
	pub incoming: usize,
	pub target: Option<usize>,
	pub by: Option<MatchBy>,
}

/// Match a new revision's pages to the current sheets: by sheet number first
/// (normalized: "A-201" = "A201"), then, for pages without a number, by
/// position. A page whose number isn't in the set is a new sheet (no match).
/// Each current sheet is replaced at most once.
pub fn match_sheets(current: &[SheetKey], incoming: &[SheetKey]) -> Vec<SheetMatch> {
	let mut used = vec![false; current.len()];
	let mut out: Vec<SheetMatch> = (0..incoming.len()).map(|i| SheetMatch { incoming: i, target: None, by: None }).collect();
	for (i, s) in incoming.iter().enumerate() {
		let Some(n) = s.number.as_deref().map(norm_sheet).filter(|n| !n.is_empty()) else { continue };
		if let Some(j) = (0..current.len()).find(|j| !used[*j] && current[*j].number.as_deref().is_some_and(|m| norm_sheet(m) == n)) {
			used[j] = true;
			out[i].target = Some(j);
			out[i].by = Some(MatchBy::Number);
		}
	}
	for (i, s) in incoming.iter().enumerate() {
		if out[i].target.is_some() || i >= current.len() || used[i] {
			continue;
		}
		if s.number.is_none() || current[i].number.is_none() {
			used[i] = true;
			out[i].target = Some(i);
			out[i].by = Some(MatchBy::Order);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn key(page: u32, number: Option<&str>) -> SheetKey {
		SheetKey { page, number: number.map(str::to_string), title: None, size_pt: (2592.0, 1728.0), source: NumberSource::TitleBlock }
	}

	fn bit(text: &str, x: f64, y: f64, h: f64) -> TextBit {
		TextBit { text: text.into(), x, y, w: 0.01 * text.len() as f64 / 5.0, h }
	}

	#[test]
	fn tokens_that_are_sheet_numbers() {
		for (t, want) in [("A-201", "A-201"), ("a2.01", "A2.01"), ("S101", "S101"), ("FP-1.1", "FP-1.1"), ("(M-3)", "M-3"), ("A-201a", "A-201A"), ("E0.1:", "E0.1"), ("ID-101", "ID-101")] {
			assert_eq!(sheet_token(t).as_deref(), Some(want), "{t}");
		}
		for t in ["10/12/2026", "2024-118", "1/4\"", "REV3", "NO.5", "5599.2", "NTS", "A", "12", "SCALE", "A-", "TEL-5551234", "B..2", "PH1"] {
			assert_eq!(sheet_token(t), None, "{t}");
		}
		assert_eq!(tokens("5538-A2.01 FLOOR PLAN"), vec!["A2.01".to_string()]);
	}

	#[test]
	fn number_in_messy_title_block_text() {
		let t = "PROJECT NO. 2024-118\nDATE 10/12/2026 SCALE: 1/4\" = 1'-0\"\nREV 3  ISSUED FOR CONSTRUCTION\nSHEET NO.\nA-201\nSEE S-101 FOR FRAMING";
		assert_eq!(sheet_number_in_text(t).as_deref(), Some("A-201"));
		assert_eq!(sheet_number_in_text("Drawing No: m-3.02 Job No.: 5599.2").as_deref(), Some("M-3.02"));
		assert_eq!(sheet_number_in_text("JOB 5599.2 ISSUE 6/4/26 NTS E-401 POWER PLAN").as_deref(), Some("E-401"));
		assert_eq!(sheet_number_in_text("no numbers here 12 34"), None);
	}

	#[test]
	fn number_in_title_block_runs() {
		// Like a real 36×24 sheet: a detail callout naming another sheet in
		// small type, the job number in big type, the sheet number under its label.
		let runs = vec![
			bit("5", 0.856, 0.78, 0.004),
			bit("A8.03", 0.854, 0.788, 0.003),
			bit("Sheet Title:", 0.903, 0.813, 0.0035),
			bit("INTERIOR DETAIL", 0.913, 0.833, 0.0136),
			bit("Job No.:", 0.903, 0.902, 0.0034),
			bit("5599.2", 0.92, 0.903, 0.0164),
			bit("Sheet No.:", 0.903, 0.933, 0.0035),
			bit("A8.06", 0.922, 0.945, 0.0162),
			bit("Issue Date 6/4/26", 0.97, 0.971, 0.004),
			bit("PER 'FINISH SCHEDULE', SHEET A0.01.", 0.796, 0.938, 0.0048),
		];
		assert_eq!(sheet_number_in_runs(&runs).as_deref(), Some("A8.06"));
		// No label at all: the big standalone token wins over references in notes.
		let runs = vec![bit("SEE A-501 FOR DETAILS", 0.80, 0.90, 0.004), bit("A-101", 0.93, 0.95, 0.012), bit("FIRST FLOOR PLAN", 0.86, 0.93, 0.008)];
		assert_eq!(sheet_number_in_runs(&runs).as_deref(), Some("A-101"));
		assert_eq!(sheet_number_in_runs(&[bit("NOTES", 0.9, 0.9, 0.01)]), None);
	}

	#[test]
	fn matching_by_number_then_order() {
		let current = vec![key(1, Some("A-101")), key(2, Some("A-102")), key(3, Some("A-201")), key(4, None)];
		// A reissue in another order, one number written differently, one new sheet, one unnumbered page.
		let incoming = vec![key(1, Some("A201")), key(2, Some("a-101")), key(3, Some("A-301")), key(4, None)];
		let m = match_sheets(&current, &incoming);
		assert_eq!(m[0].target, Some(2));
		assert_eq!(m[0].by, Some(MatchBy::Number));
		assert_eq!(m[1].target, Some(0));
		assert_eq!(m[2].target, None, "A-301 is a new sheet");
		assert_eq!((m[3].target, m[3].by), (Some(3), Some(MatchBy::Order)));
		// No numbers anywhere: page order.
		let m = match_sheets(&[key(1, None), key(2, None)], &[key(1, None), key(2, None), key(3, None)]);
		assert_eq!(m.iter().map(|m| m.target).collect::<Vec<_>>(), vec![Some(0), Some(1), None]);
		// The same number twice in the set: each sheet is replaced once.
		let m = match_sheets(&[key(1, Some("S-1")), key(2, Some("S-1"))], &[key(1, Some("S-1")), key(2, Some("S-1"))]);
		assert_eq!(m.iter().map(|m| m.target).collect::<Vec<_>>(), vec![Some(0), Some(1)]);
	}
}
