//! Markup legends: the rows a legend lists, computed live from the takeoff
//! (never stored), and where everything sits in its box. The canvas, burned
//! renders and the flattened PDF all draw from the same layout.
//!
//! Positions are page fractions: x of the width, y of the height. Text sizes
//! are fractions of the page height.

use std::collections::BTreeMap;

use crate::doc::Document;
use crate::list::summary_where;
use crate::model::*;
use crate::units::group_thousands;

/// What a row's swatch looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Swatch {
	/// A square, like a count mark.
	Count,
	/// A line in the item's line style.
	Length(Option<LineStyle>),
	/// A tinted block in the item's hatch.
	Area(Option<Hatch>),
	/// An outlined block in the markup's colour.
	Markup(AnnotationKind),
}

/// One line of a legend.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendRow {
	pub category_id: Option<String>,
	pub name: String,
	pub color: String,
	/// The item's fill opacity (1 = as is).
	pub opacity: f64,
	pub swatch: Swatch,
	pub count: usize,
	/// Total with units ("1,234.50 ft²"); empty for markups.
	pub quantity: String,
	pub cost: Option<f64>,
}

/// Which page a legend's scope keeps (None = every page).
fn scope_page(l: &Legend) -> Option<u32> {
	match l.scope {
		LegendScope::Document => None,
		LegendScope::Page | LegendScope::Unknown => Some(l.page),
	}
}

/// The rows of a legend: each item with something in scope (hidden items
/// left out), then markups by subject when asked for.
pub fn rows(doc: &Document, l: &Legend) -> Vec<LegendRow> {
	let d = &doc.data;
	let page = scope_page(l);
	let in_scope = |p: u32| page.is_none_or(|w| w == p);
	let mut out = Vec::new();
	for line in summary_where(doc, |_, p| in_scope(p)) {
		if line.items == 0 {
			continue;
		}
		let cat = line.category_id.as_deref().and_then(|c| d.category(c));
		if cat.is_some_and(|c| c.hidden) || (!l.category_ids.is_empty() && !line.category_id.as_ref().is_some_and(|c| l.category_ids.contains(c))) {
			continue;
		}
		let swatch = match line.kind {
			CategoryKind::Count => Swatch::Count,
			CategoryKind::Length => Swatch::Length(cat.and_then(|c| c.line_style)),
			CategoryKind::Area => Swatch::Area(cat.and_then(|c| c.hatch)),
		};
		out.push(LegendRow { category_id: line.category_id.clone(), name: line.name.clone(), color: line.color.clone(), opacity: cat.and_then(|c| c.opacity).unwrap_or(1.0), swatch, count: line.items, quantity: line.total_text.clone(), cost: line.cost });
	}
	if l.markups {
		let mut by_subject: BTreeMap<String, LegendRow> = BTreeMap::new();
		for a in d.annotations.iter().filter(|a| in_scope(a.page)) {
			let name = a.subject.clone().unwrap_or_else(|| a.kind.label().to_string());
			by_subject
				.entry(name.clone())
				.or_insert_with(|| LegendRow { category_id: None, name, color: a.style.stroke.clone(), opacity: a.style.opacity, swatch: Swatch::Markup(a.kind), count: 0, quantity: String::new(), cost: None })
				.count += 1;
		}
		out.extend(by_subject.into_values());
	}
	out
}

/// The rows of a legend from the takeoff alone (exports, renders): page
/// sizes come from its `pages`.
pub fn rows_for(data: &TakeoffData, l: &Legend) -> Vec<LegendRow> {
	let sizes = data.pages.iter().map(|p| (p.width_px * 72.0 / 200.0, p.height_px * 72.0 / 200.0)).collect();
	rows(&Document::new(data.clone(), sizes, ""), l)
}

/// A piece of text in a legend. `at` is the top-left of its line, or the
/// top-right when `right` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendText {
	pub at: Point,
	/// Fraction of the page height.
	pub size: f64,
	pub text: String,
	pub right: bool,
	pub bold: bool,
	/// Header and "no items" text, drawn quieter.
	pub muted: bool,
}

/// Where everything in a legend sits.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendLayout {
	pub texts: Vec<LegendText>,
	/// Swatch boxes (x, y, w, h) with the index of their row.
	pub swatches: Vec<((f64, f64, f64, f64), usize)>,
	/// Rules under the title and the header.
	pub rules: Vec<(Point, Point)>,
}

/// Width of `text` in page-width fractions at `size` (page-height
/// fraction) on a page `aspect` (height / width) tall: an estimate, for
/// laying out columns before any font is at hand.
pub fn text_width(text: &str, size: f64, aspect: f64) -> f64 {
	text.chars().count() as f64 * 0.56 * size * aspect
}

fn cost_text(c: Option<f64>) -> String {
	c.map(|c| format!("${}", group_thousands(&format!("{c:.2}")))).unwrap_or_default()
}

/// Lay a legend's rows out in its box. The text shrinks to fit the box:
/// resizing the legend resizes its text.
pub fn layout(l: &Legend, rows: &[LegendRow], aspect: f64) -> LegendLayout {
	let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
	let n = rows.len().max(1) as f64;
	let rh = l.h / (1.5 + 1.0 + n + 0.3);
	let pad = (l.w * 0.035).min(rh * 0.6 * aspect);
	let (sh, sw) = (rh * 0.56, rh * 0.56 * 1.8 * aspect);
	let gap = pad;
	let cols = &l.columns;
	let head_count = "Count";
	let head_qty = "Quantity";
	let head_cost = "Cost";
	let mut numeric: Vec<(&str, Vec<String>)> = Vec::new();
	if cols.count {
		numeric.push((head_count, rows.iter().map(|r| group_thousands(&r.count.to_string())).collect()));
	}
	if cols.quantity {
		numeric.push((head_qty, rows.iter().map(|r| r.quantity.clone()).collect()));
	}
	if cols.cost {
		numeric.push((head_cost, rows.iter().map(|r| cost_text(r.cost)).collect()));
	}
	let chars = |s: &str| s.chars().count();
	let name_chars = rows.iter().map(|r| chars(&r.name)).max().unwrap_or(0).max(4);
	let col_chars: Vec<usize> = numeric.iter().map(|(h, v)| v.iter().map(|s| chars(s)).max().unwrap_or(0).max(chars(h))).collect();
	let total_chars = name_chars + col_chars.iter().sum::<usize>() + 2 * col_chars.len();
	let avail = (l.w - 2.0 * pad - sw - gap).max(l.w * 0.2);
	let size = (rh * 0.62).min(avail / (total_chars as f64 * 0.56 * aspect));
	let title = l.title();
	let title_size = (rh * 0.9).min(size * 1.4).min((l.w - 2.0 * pad) / (chars(title).max(1) as f64 * 0.58 * aspect)).max(size);
	let (x0, x1) = (l.x + pad, l.x + l.w - pad);
	let mut out = LegendLayout { texts: vec![], swatches: vec![], rules: vec![] };
	let text = |at: Point, size: f64, text: String, right: bool, bold: bool, muted: bool| LegendText { at, size, text, right, bold, muted };
	out.texts.push(text(Point { x: x0, y: l.y + (rh * 1.5 - title_size) / 2.0 }, title_size, title.to_string(), false, true, false));
	out.rules.push((Point { x: x0, y: l.y + rh * 1.5 }, Point { x: x1, y: l.y + rh * 1.5 }));
	// Numeric columns, right-aligned from the right edge inwards.
	let mut rights = Vec::new();
	let mut r = x1;
	for c in col_chars.iter().rev() {
		rights.push(r);
		r -= (*c as f64 + 2.0) * 0.56 * size * aspect;
	}
	rights.reverse();
	let name_x = x0 + sw + gap;
	let head_y = l.y + rh * 1.5 + (rh - size) / 2.0;
	out.texts.push(text(Point { x: name_x, y: head_y }, size, "Item".into(), false, false, true));
	for ((h, _), rx) in numeric.iter().zip(&rights) {
		out.texts.push(text(Point { x: *rx, y: head_y }, size, h.to_string(), true, false, true));
	}
	let body = l.y + rh * 2.5;
	out.rules.push((Point { x: x0, y: body }, Point { x: x1, y: body }));
	if rows.is_empty() {
		out.texts.push(text(Point { x: name_x, y: body + rh * 0.15 + (rh - size) / 2.0 }, size, "No items".into(), false, false, true));
	}
	for (k, row) in rows.iter().enumerate() {
		let top = body + rh * 0.15 + k as f64 * rh;
		// Counts get a square, like their marks.
		let w = if row.swatch == Swatch::Count { sh * aspect } else { sw };
		out.swatches.push(((x0, top + (rh - sh) / 2.0, w, sh), k));
		out.texts.push(text(Point { x: name_x, y: top + (rh - size) / 2.0 }, size, row.name.clone(), false, false, false));
		for ((_, vals), rx) in numeric.iter().zip(&rights) {
			out.texts.push(text(Point { x: *rx, y: top + (rh - size) / 2.0 }, size, vals[k].clone(), true, true, false));
		}
	}
	out
}

/// Hatch lines inside a swatch box (x, y, w, h), ~`count` lines across.
pub fn hatch_lines(b: (f64, f64, f64, f64), hatch: Hatch, aspect: f64, count: usize) -> Vec<(Point, Point)> {
	// Work in page-height units so 45° is 45° on paper: X = x / aspect.
	let aspect = if aspect > 0.0 { aspect } else { 1.0 };
	let (x0, y0, x1, y1) = (b.0 / aspect, b.1, (b.0 + b.2) / aspect, b.1 + b.3);
	let gap = ((x1 - x0) + (y1 - y0)) / count.max(1) as f64;
	let pt = |x: f64, y: f64| Point { x: x * aspect, y };
	let mut out = Vec::new();
	let diag = |out: &mut Vec<(Point, Point)>, flip: bool| {
		// y = x - c (or y = c - x), clipped to the box.
		let mut c = if flip { x0 + y0 } else { x0 - y1 };
		let end = if flip { x1 + y1 } else { x1 - y0 };
		c += gap / 2.0;
		while c < end {
			let (a, b) = if flip { (x0.max(c - y1), x1.min(c - y0)) } else { (x0.max(y0 + c), x1.min(y1 + c)) };
			if b > a {
				out.push(if flip { (pt(a, c - a), pt(b, c - b)) } else { (pt(a, a - c), pt(b, b - c)) });
			}
			c += gap;
		}
	};
	match hatch {
		Hatch::Diagonal | Hatch::Dots => diag(&mut out, false),
		Hatch::Cross => {
			diag(&mut out, false);
			diag(&mut out, true);
		}
		Hatch::Grid => {
			let mut x = x0 + gap / 2.0;
			while x < x1 {
				out.push((pt(x, y0), pt(x, y1)));
				x += gap;
			}
			let mut y = y0 + gap / 2.0;
			while y < y1 {
				out.push((pt(x0, y), pt(x1, y)));
				y += gap;
			}
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn doc() -> Document {
		let mut d = TakeoffData::default();
		d.categories.push(Category::new("c1", "Duplex", "#4CC2FF", CategoryKind::Count));
		let mut wall = Category::new("c2", "Wall", "#F87171", CategoryKind::Length);
		wall.line_style = Some(LineStyle::Dashed);
		wall.unit_cost = Some(2.0);
		d.categories.push(wall);
		let mut hidden = Category::new("c3", "Hidden", "#000000", CategoryKind::Count);
		hidden.hidden = true;
		d.categories.push(hidden);
		d.categories.push(Category::new("c4", "Empty", "#00FF00", CategoryKind::Area));
		for (i, (page, cat)) in [(1, "c1"), (1, "c1"), (2, "c1"), (1, "c3")].iter().enumerate() {
			d.marks.push(Mark { id: format!("m{i}"), category_id: Some(cat.to_string()), page: *page, x: 0.1 * i as f64, y: 0.1, w: 0.01, h: 0.01, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() });
		}
		let mut m = Measurement::new("w1", 1, MeasurementType::Length, vec![Point { x: 0.0, y: 0.5 }, Point { x: 0.5, y: 0.5 }]);
		m.category_id = Some("c2".into());
		d.measurements.push(m);
		d.page_scales.push(PageScale { page: 1, p1: Point { x: 0.0, y: 0.0 }, p2: Point { x: 0.1, y: 0.0 }, distance: 10.0, unit: "ft".into() });
		d.annotations.push(Annotation { id: "a1".into(), page: 1, kind: AnnotationKind::Cloud, points: vec![Point { x: 0.1, y: 0.1 }, Point { x: 0.2, y: 0.2 }, Point { x: 0.1, y: 0.2 }], text: None, style: Default::default(), subject: Some("RFI".into()), note: None, source: None, meta: Default::default() });
		d.annotations.push(Annotation { id: "a2".into(), page: 2, kind: AnnotationKind::Text, points: vec![Point { x: 0.1, y: 0.1 }, Point { x: 0.2, y: 0.2 }], text: None, style: Default::default(), subject: None, note: None, source: None, meta: Default::default() });
		Document::new(d, vec![(1000.0, 500.0), (1000.0, 500.0)], "me")
	}

	#[test]
	fn rows_follow_scope_items_and_markups() {
		let doc = doc();
		let mut l = Legend::new("lg", 1, 0.7, 0.7, 0.25, 0.2);
		let r = rows(&doc, &l);
		let names: Vec<&str> = r.iter().map(|r| r.name.as_str()).collect();
		assert_eq!(names, ["Duplex", "Wall"], "page 1, hidden and empty items left out");
		assert_eq!(r[0].count, 2);
		assert_eq!(r[0].swatch, Swatch::Count);
		assert_eq!(r[1].swatch, Swatch::Length(Some(LineStyle::Dashed)));
		assert_eq!(r[1].quantity, "50.00 ft");
		assert_eq!(r[1].cost, Some(100.0));
		l.scope = LegendScope::Document;
		assert_eq!(rows(&doc, &l)[0].count, 3, "every page");
		l.category_ids = vec!["c2".into()];
		assert_eq!(rows(&doc, &l).len(), 1);
		l.category_ids.clear();
		l.markups = true;
		let r = rows(&doc, &l);
		let names: Vec<&str> = r.iter().map(|r| r.name.as_str()).collect();
		assert_eq!(names, ["Duplex", "Wall", "RFI", "Text Box"]);
		assert_eq!(r[2].swatch, Swatch::Markup(AnnotationKind::Cloud));
		assert_eq!(r[2].quantity, "");
	}

	#[test]
	fn rows_update_with_the_takeoff() {
		let mut doc = doc();
		let l = Legend::new("lg", 1, 0.7, 0.7, 0.25, 0.2);
		assert_eq!(rows(&doc, &l)[0].count, 2);
		doc.edit("move", |d| d.marks[2].page = 1);
		assert_eq!(rows(&doc, &l)[0].count, 3);
		doc.edit("scale", |d| d.page_scales[0].distance = 20.0);
		assert_eq!(rows(&doc, &l)[1].quantity, "100.00 ft");
	}

	#[test]
	fn rows_from_the_data_alone_match() {
		let doc = doc();
		let l = Legend::new("lg", 1, 0.7, 0.7, 0.25, 0.2);
		assert_eq!(rows_for(&doc.data, &l), rows(&doc, &l));
	}

	#[test]
	fn layout_fits_the_box() {
		let doc = doc();
		let mut l = Legend::new("lg", 1, 0.6, 0.6, 0.3, 0.25);
		l.columns.cost = true;
		l.title = Some("Electrical".into());
		let r = rows(&doc, &l);
		let lay = layout(&l, &r, 0.5);
		assert_eq!(lay.swatches.len(), 2);
		assert_eq!(lay.texts[0].text, "Electrical");
		assert!(lay.texts.iter().any(|t| t.text == "$100.00" && t.right));
		for t in &lay.texts {
			let (x0, x1) = if t.right { (t.at.x - text_width(&t.text, t.size, 0.5), t.at.x) } else { (t.at.x, t.at.x + text_width(&t.text, t.size, 0.5)) };
			assert!(x0 >= l.x - 1e-9 && x1 <= l.x + l.w + 1e-9, "{} fits across", t.text);
			assert!(t.at.y >= l.y && t.at.y + t.size <= l.y + l.h + 1e-9, "{} fits down", t.text);
		}
		// Bigger box, bigger text.
		let small = layout(&l, &r, 0.5).texts[2].size;
		l.w *= 2.0;
		l.h *= 2.0;
		assert!(layout(&l, &r, 0.5).texts[2].size > small * 1.9);
		// No rows: a "No items" line.
		assert!(layout(&l, &[], 0.5).texts.iter().any(|t| t.text == "No items"));
	}

	#[test]
	fn hatch_lines_stay_in_their_box() {
		let b = (0.1, 0.2, 0.05, 0.02);
		for h in [Hatch::Diagonal, Hatch::Cross, Hatch::Grid] {
			let lines = hatch_lines(b, h, 0.5, 6);
			assert!(!lines.is_empty());
			for (a, z) in lines {
				for p in [a, z] {
					assert!(p.x >= b.0 - 1e-9 && p.x <= b.0 + b.2 + 1e-9 && p.y >= b.1 - 1e-9 && p.y <= b.1 + b.3 + 1e-9, "{h:?} {p:?}");
				}
			}
		}
	}
}
