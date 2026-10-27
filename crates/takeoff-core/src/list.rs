//! The Markups List: one row per item with Bluebeam-style columns, plus the
//! per-item summary (counts, totals, cost) and CSV and Excel export of both.

use std::collections::BTreeMap;

use crate::doc::{Document, ItemRef};
use crate::model::*;
use crate::time::date_from_ms;
use crate::units::{format_quantity, group_thousands};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowKind {
	Count,
	Length,
	Perimeter,
	Area,
	Angle,
	Annotation(AnnotationKindKey),
}

/// `AnnotationKind` with an order, for sorting rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnnotationKindKey(pub u8);

impl RowKind {
	pub fn label(self) -> &'static str {
		match self {
			RowKind::Count => "Count",
			RowKind::Length => "Length",
			RowKind::Perimeter => "Perimeter",
			RowKind::Area => "Area",
			RowKind::Angle => "Angle",
			RowKind::Annotation(k) => AnnotationKind::ALL[k.0 as usize].label(),
		}
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarkupRow {
	pub item: ItemRef,
	pub kind: RowKind,
	pub page: u32,
	/// Category name, or the annotation's subject.
	pub subject: String,
	pub category_id: Option<String>,
	pub color: String,
	/// Badge number within the category (marks and measurements).
	pub number: Option<u32>,
	pub label: String,
	pub value: Option<f64>,
	pub unit: String,
	pub quantity: String,
	/// Areas: perimeter; with depth: volume.
	pub extra: String,
	pub author: String,
	pub date: String,
	pub modified_ms: i64,
	/// Pending/Confirmed for AI marks, else the review status.
	pub status: String,
	pub layer: String,
	pub note: String,
	pub source: &'static str,
	pub fields: BTreeMap<String, String>,
	pub locked: bool,
	pub hidden: bool,
	/// The deepest space it is in, and that space's full path ("Level 2 › Unit 204").
	pub space_id: Option<String>,
	pub space: String,
}

const UNGROUPED_COLOR: &str = "#22D3EE";

pub fn rows(doc: &Document) -> Vec<MarkupRow> {
	let d = &doc.data;
	let disp = &d.display;
	let mark_seq = crate::numbering::mark_seq(&d.marks);
	let meas_seq = crate::numbering::measurement_seq(&d.measurements);
	let cat = |id: &Option<String>| id.as_deref().and_then(|i| d.category(i));
	let spaces = crate::spaces::Spaces::new(d);
	let names: std::collections::HashMap<&str, String> = d.spaces.iter().map(|s| (s.id.as_str(), spaces.path_name(&s.id))).collect();
	let space_of = |s: Option<&Space>| (s.map(|s| s.id.clone()), s.and_then(|s| names.get(s.id.as_str()).cloned()).unwrap_or_default());
	let mut out = Vec::with_capacity(d.marks.len() + d.measurements.len() + d.annotations.len());

	for m in &d.marks {
		let c = cat(&m.category_id);
		let (space_id, space) = space_of(spaces.of_mark(m));
		out.push(MarkupRow {
			item: ItemRef::Mark(m.id.clone()),
			kind: RowKind::Count,
			page: m.page,
			subject: c.map(|c| c.name.clone()).unwrap_or_else(|| "Count".into()),
			category_id: m.category_id.clone(),
			color: c.map(|c| c.color.clone()).unwrap_or_else(|| UNGROUPED_COLOR.into()),
			number: mark_seq.get(&m.id).copied(),
			label: String::new(),
			value: Some(1.0),
			unit: "ea".into(),
			quantity: "1 ea".into(),
			extra: String::new(),
			author: m.meta.author.clone().unwrap_or_else(|| if m.source == Source::Ai { "AI".into() } else { String::new() }),
			date: m.meta.created_at.map(date_from_ms).unwrap_or_default(),
			modified_ms: m.meta.modified_at.unwrap_or(0),
			status: m.meta.review.clone().unwrap_or_else(|| match m.status {
				MarkStatus::Pending => "Pending".into(),
				MarkStatus::Confirmed => "Confirmed".into(),
			}),
			layer: m.meta.layer.clone().unwrap_or_default(),
			note: m.note.clone().unwrap_or_default(),
			source: if m.source == Source::Ai { "AI" } else { "User" },
			fields: m.fields.clone().unwrap_or_default(),
			locked: m.meta.locked,
			hidden: c.is_some_and(|c| c.hidden),
			space_id,
			space,
		});
	}
	// `probe_*` are the agent's transient check shapes, not takeoff items.
	for m in d.measurements.iter().filter(|m| !m.id.starts_with("probe_")) {
		let c = cat(&m.category_id);
		let q = doc.quantity(m);
		let (space_id, space) = space_of(spaces.of_measurement(m));
		let kind = match m.kind {
			MeasurementType::Length if m.closed => RowKind::Perimeter,
			MeasurementType::Length => RowKind::Length,
			MeasurementType::Area => RowKind::Area,
			MeasurementType::Angle => RowKind::Angle,
		};
		let extra = q
			.as_ref()
			.map(|q| {
				let mut parts = vec![];
				if let Some(p) = q.perimeter {
					parts.push(format!("P {}", format_quantity(p, &q.base_unit, disp)));
				}
				if let Some(v) = q.volume {
					parts.push(format!("V {}", format_quantity(v, &format!("{}³", q.base_unit), disp)));
				}
				parts.join(" · ")
			})
			.unwrap_or_default();
		out.push(MarkupRow {
			item: ItemRef::Measurement(m.id.clone()),
			kind,
			page: m.page,
			subject: c.map(|c| c.name.clone()).unwrap_or_else(|| kind.label().into()),
			category_id: m.category_id.clone(),
			color: m.color.clone().or_else(|| c.map(|c| c.color.clone())).unwrap_or_else(|| UNGROUPED_COLOR.into()),
			number: meas_seq.get(&m.id).copied(),
			label: m.label.clone().unwrap_or_default(),
			value: q.as_ref().map(|q| q.value),
			unit: q.as_ref().map(|q| q.unit.clone()).unwrap_or_default(),
			quantity: q.as_ref().map(|q| format_quantity(q.value, &q.unit, disp)).unwrap_or_else(|| "no scale".into()),
			extra,
			author: m.meta.author.clone().unwrap_or_else(|| if m.source == Some(Source::Ai) { "AI".into() } else { String::new() }),
			date: m.meta.created_at.map(date_from_ms).unwrap_or_default(),
			modified_ms: m.meta.modified_at.unwrap_or(0),
			status: m.meta.review.clone().unwrap_or_default(),
			layer: m.meta.layer.clone().unwrap_or_default(),
			note: m.note.clone().unwrap_or_default(),
			source: if m.source == Some(Source::Ai) { "AI" } else { "User" },
			fields: m.fields.clone().unwrap_or_default(),
			locked: m.meta.locked,
			hidden: c.is_some_and(|c| c.hidden),
			space_id,
			space,
		});
	}
	for a in &d.annotations {
		let k = AnnotationKind::ALL.iter().position(|k| *k == a.kind).unwrap_or(0) as u8;
		let (space_id, space) = space_of(spaces.of_annotation(a));
		out.push(MarkupRow {
			item: ItemRef::Annotation(a.id.clone()),
			kind: RowKind::Annotation(AnnotationKindKey(k)),
			page: a.page,
			subject: a.subject.clone().unwrap_or_else(|| a.kind.label().into()),
			category_id: None,
			color: a.style.stroke.clone(),
			number: None,
			label: a.text.clone().unwrap_or_default(),
			value: None,
			unit: String::new(),
			quantity: String::new(),
			extra: String::new(),
			author: a.meta.author.clone().unwrap_or_else(|| if a.source == Some(Source::Ai) { "AI".into() } else { String::new() }),
			date: a.meta.created_at.map(date_from_ms).unwrap_or_default(),
			modified_ms: a.meta.modified_at.unwrap_or(0),
			status: a.meta.review.clone().unwrap_or_default(),
			layer: a.meta.layer.clone().unwrap_or_default(),
			note: a.note.clone().unwrap_or_default(),
			source: if a.source == Some(Source::Ai) { "AI" } else { "User" },
			fields: BTreeMap::new(),
			locked: a.meta.locked,
			hidden: false,
			space_id,
			space,
		});
	}
	out
}

/// One line of the takeoff summary: everything in one item (category).
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryLine {
	pub category_id: Option<String>,
	pub name: String,
	pub color: String,
	pub kind: CategoryKind,
	pub items: usize,
	pub pending: usize,
	/// Count for count items, summed value for length/area.
	pub total: f64,
	pub unit: String,
	pub total_text: String,
	/// Areas: summed volume when depths are set.
	pub volume: Option<f64>,
	pub unit_cost: Option<f64>,
	pub cost: Option<f64>,
	/// Mixed units or unscaled pages → the total is partial.
	pub warning: Option<String>,
}

pub fn summary(doc: &Document) -> Vec<SummaryLine> {
	summary_where(doc, |_, _| true)
}

/// The summary of only the marks and measurements `keep(id, page)` lets through.
pub fn summary_where(doc: &Document, keep: impl Fn(&str, u32) -> bool) -> Vec<SummaryLine> {
	let d = &doc.data;
	let disp = &d.display;
	let mut out: Vec<SummaryLine> = Vec::new();
	let mut keys: Vec<Option<&Category>> = d.categories.iter().map(Some).collect();
	keys.push(None);
	for c in keys {
		let cid = c.map(|c| c.id.clone());
		let marks: Vec<&Mark> = d.marks.iter().filter(|m| m.category_id == cid && keep(&m.id, m.page)).collect();
		let meas: Vec<&Measurement> =
			d.measurements.iter().filter(|m| m.category_id == cid && m.kind != MeasurementType::Angle && keep(&m.id, m.page)).collect();
		if c.is_none() && marks.is_empty() && meas.is_empty() {
			continue;
		}
		let kind = c.map(|c| c.kind()).unwrap_or(if marks.is_empty() { CategoryKind::Length } else { CategoryKind::Count });
		let (mut total, mut unit, mut warning, mut volume) = (0.0, String::new(), None, None::<f64>);
		if kind == CategoryKind::Count {
			total = marks.len() as f64;
			unit = "ea".into();
		} else {
			let mut unscaled = 0;
			for m in &meas {
				match doc.quantity(m) {
					Some(q) => {
						if unit.is_empty() {
							unit = q.unit.clone();
						}
						if crate::geometry::unit_key(&q.unit) != crate::geometry::unit_key(&unit) {
							warning = Some("mixed units".into());
							continue;
						}
						total += q.value;
						if let Some(v) = q.volume {
							*volume.get_or_insert(0.0) += v;
						}
					}
					None => unscaled += 1,
				}
			}
			if unscaled > 0 {
				warning = Some(format!("{unscaled} not scaled"));
			}
		}
		let items = if kind == CategoryKind::Count { marks.len() } else { meas.len() };
		let unit_cost = c.and_then(|c| c.unit_cost);
		out.push(SummaryLine {
			category_id: cid,
			name: c.map(|c| c.name.clone()).unwrap_or_else(|| "Ungrouped".into()),
			color: c.map(|c| c.color.clone()).unwrap_or_else(|| UNGROUPED_COLOR.into()),
			kind,
			items,
			pending: marks.iter().filter(|m| m.status == MarkStatus::Pending).count(),
			total,
			total_text: if unit.is_empty() { "—".into() } else { group_thousands(&format_quantity(total, &unit, disp)) },
			unit,
			volume,
			unit_cost,
			cost: unit_cost.map(|u| u * total),
			warning,
		});
	}
	out
}

/// One exported cell: CSV writes it as text, Excel as a typed cell.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
	Text(String),
	Num(f64),
	/// Two decimals in CSV, `#,##0.00` in Excel.
	Money(f64),
	Empty,
}

impl Cell {
	fn text(s: impl Into<String>) -> Self {
		let s = s.into();
		if s.is_empty() { Cell::Empty } else { Cell::Text(s) }
	}
	fn csv(&self) -> String {
		match self {
			Cell::Text(s) => s.clone(),
			Cell::Num(v) => format!("{v}"),
			Cell::Money(v) => format!("{v:.2}"),
			Cell::Empty => String::new(),
		}
	}
}

const ROW_HEADER: [&str; 16] = ["Page", "Space", "Type", "Subject", "#", "Label", "Quantity", "Value", "Unit", "Extra", "Author", "Date", "Status", "Layer", "Source", "Note"];
const SUMMARY_HEADER: [&str; 9] = ["Item", "Type", "Count", "Quantity", "Unit", "Volume", "Unit cost", "Cost", "Note"];

/// The Markups List as a header and cells, custom columns included: what both exports write.
fn row_table(doc: &Document, rows: &[MarkupRow]) -> (Vec<String>, Vec<Vec<Cell>>) {
	let custom = &doc.data.custom_fields;
	let mut header: Vec<String> = ROW_HEADER.iter().map(|s| s.to_string()).collect();
	header.extend(custom.iter().map(|f| f.name.clone()));
	let body = rows
		.iter()
		.map(|r| {
			let mut cells = vec![
				Cell::Num(r.page as f64),
				Cell::text(r.space.clone()),
				Cell::text(r.kind.label()),
				Cell::text(r.subject.clone()),
				r.number.map(|n| Cell::Num(n as f64)).unwrap_or(Cell::Empty),
				Cell::text(r.label.clone()),
				Cell::text(r.quantity.clone()),
				r.value.map(Cell::Num).unwrap_or(Cell::Empty),
				Cell::text(r.unit.clone()),
				Cell::text(r.extra.clone()),
				Cell::text(r.author.clone()),
				Cell::text(r.date.clone()),
				Cell::text(r.status.clone()),
				Cell::text(r.layer.clone()),
				Cell::text(r.source),
				Cell::text(r.note.clone()),
			];
			cells.extend(custom.iter().map(|f| Cell::text(r.fields.get(&f.id).cloned().unwrap_or_default())));
			cells
		})
		.collect();
	(header, body)
}

fn summary_table(lines: &[SummaryLine]) -> (Vec<String>, Vec<Vec<Cell>>) {
	let body = lines
		.iter()
		.map(|l| {
			vec![
				Cell::text(l.name.clone()),
				Cell::text(format!("{:?}", l.kind)),
				Cell::Num(l.items as f64),
				Cell::Num(l.total),
				Cell::text(l.unit.clone()),
				l.volume.map(Cell::Num).unwrap_or(Cell::Empty),
				l.unit_cost.map(Cell::Money).unwrap_or(Cell::Empty),
				l.cost.map(Cell::Money).unwrap_or(Cell::Empty),
				Cell::text(l.warning.clone().unwrap_or_default()),
			]
		})
		.collect();
	(SUMMARY_HEADER.iter().map(|s| s.to_string()).collect(), body)
}

/// The summary for one space (or for what is in no space).
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceSummary {
	pub space_id: Option<String>,
	/// Full path, or "No space".
	pub name: String,
	pub color: String,
	pub page: Option<u32>,
	pub depth: usize,
	/// Only the items with something in this space.
	pub lines: Vec<SummaryLine>,
}

/// The summary per space: every quantity counts once, in its deepest space,
/// so the groups add up to the whole summary. Spaces in tree order; empty
/// ones left out; "No space" last.
pub fn summary_by_space(doc: &Document) -> Vec<SpaceSummary> {
	let d = &doc.data;
	let spaces = crate::spaces::Spaces::new(d);
	let of = spaces.attribute(d);
	let mut out = Vec::new();
	let used = |l: &SummaryLine| l.items > 0;
	for (s, depth) in spaces.tree() {
		let lines: Vec<SummaryLine> = summary_where(doc, |id, _| of.get(id) == Some(&s.id)).into_iter().filter(used).collect();
		if !lines.is_empty() {
			out.push(SpaceSummary { space_id: Some(s.id.clone()), name: spaces.path_name(&s.id), color: s.color.clone(), page: Some(s.page), depth, lines });
		}
	}
	let lines: Vec<SummaryLine> = summary_where(doc, |id, _| !of.contains_key(id)).into_iter().filter(used).collect();
	if !lines.is_empty() {
		out.push(SpaceSummary { space_id: None, name: "No space".into(), color: UNGROUPED_COLOR.into(), page: None, depth: 0, lines });
	}
	out
}

fn csv_cell(s: &str) -> String {
	if s.contains([',', '"', '\n', '\r']) {
		format!("\"{}\"", s.replace('"', "\"\""))
	} else {
		s.to_string()
	}
}

fn csv_line(cells: &[String]) -> String {
	cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",")
}

fn table_csv((header, body): (Vec<String>, Vec<Vec<Cell>>)) -> String {
	let mut out = csv_line(&header);
	for cells in body {
		out.push('\n');
		out.push_str(&csv_line(&cells.iter().map(Cell::csv).collect::<Vec<_>>()));
	}
	out.push('\n');
	out
}

/// Markups List as CSV, custom columns included.
pub fn rows_csv(doc: &Document, rows: &[MarkupRow]) -> String {
	table_csv(row_table(doc, rows))
}

pub fn summary_csv(lines: &[SummaryLine]) -> String {
	table_csv(summary_table(lines))
}

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> String {
	format!("couldn't write the workbook: {e}")
}

/// One sheet: bold header row frozen at the top, then the cells, typed. `total` adds a bold
/// last row with that label and the sum of the money in column `total.1`.
fn xlsx_sheet(wb: &mut rust_xlsxwriter::Workbook, name: &str, (header, body): (Vec<String>, Vec<Vec<Cell>>), total: Option<(&str, u16)>) -> Result<(), String> {
	use rust_xlsxwriter::Format;
	let bold = Format::new().set_bold();
	let money = Format::new().set_num_format("#,##0.00");
	let ws = wb.add_worksheet().set_name(name).map_err(xlsx_err)?;
	for (c, h) in header.iter().enumerate() {
		ws.write_string_with_format(0, c as u16, h, &bold).map_err(xlsx_err)?;
	}
	let mut sum = None::<f64>;
	for (r, cells) in body.iter().enumerate() {
		let r = r as u32 + 1;
		for (c, cell) in cells.iter().enumerate() {
			let c = c as u16;
			match cell {
				Cell::Text(s) => ws.write_string(r, c, s).map(|_| ()),
				Cell::Num(v) => ws.write_number(r, c, *v).map(|_| ()),
				Cell::Money(v) => {
					if total.is_some_and(|t| t.1 == c) {
						*sum.get_or_insert(0.0) += v;
					}
					ws.write_number_with_format(r, c, *v, &money).map(|_| ())
				}
				Cell::Empty => Ok(()),
			}
			.map_err(xlsx_err)?;
		}
	}
	if let (Some((label, col)), Some(sum)) = (total, sum) {
		let r = body.len() as u32 + 1;
		ws.write_string_with_format(r, 0, label, &bold).map_err(xlsx_err)?;
		ws.write_number_with_format(r, col, sum, &money.set_bold()).map_err(xlsx_err)?;
	}
	ws.set_freeze_panes(1, 0).map_err(xlsx_err)?;
	ws.autofit();
	Ok(())
}

/// Markups List as an Excel workbook: a *Markups* sheet with the CSV's columns, a *Summary*
/// sheet with the quantity summary's, and with spaces a *By space* sheet. Pages, numbers,
/// values, counts and costs are numbers.
pub fn rows_xlsx(doc: &Document, rows: &[MarkupRow]) -> Result<Vec<u8>, String> {
	let mut wb = rust_xlsxwriter::Workbook::new();
	xlsx_sheet(&mut wb, "Markups", row_table(doc, rows), None)?;
	xlsx_sheet(&mut wb, "Summary", summary_table(&summary(doc)), Some(("Total", 7)))?;
	if !doc.data.spaces.is_empty() {
		xlsx_sheet(&mut wb, "By space", space_summary_table(&summary_by_space(doc)), Some(("Total", 8)))?;
	}
	wb.save_to_buffer().map_err(xlsx_err)
}

/// Quantity summary as an Excel workbook, one sheet, with a total cost row when items have costs.
pub fn summary_xlsx(lines: &[SummaryLine]) -> Result<Vec<u8>, String> {
	let mut wb = rust_xlsxwriter::Workbook::new();
	xlsx_sheet(&mut wb, "Summary", summary_table(lines), Some(("Total", 7)))?;
	wb.save_to_buffer().map_err(xlsx_err)
}

/// The summary per space as a table: a Space column, then the summary's columns.
fn space_summary_table(groups: &[SpaceSummary]) -> (Vec<String>, Vec<Vec<Cell>>) {
	let (mut header, _) = summary_table(&[]);
	header.insert(0, "Space".into());
	let body = groups
		.iter()
		.flat_map(|g| summary_table(&g.lines).1.into_iter().map(|mut cells| {
			cells.insert(0, Cell::text(g.name.clone()));
			cells
		}))
		.collect();
	(header, body)
}

/// The summary per space as CSV: a Space column, then the summary's columns.
pub fn space_summary_csv(groups: &[SpaceSummary]) -> String {
	table_csv(space_summary_table(groups))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn summary_and_csv() {
		let mut data = TakeoffData::default();
		let mut c = Category::new("c1", "Duplex, 20A", "#4CC2FF", CategoryKind::Count);
		c.unit_cost = Some(12.5);
		data.categories.push(c);
		for i in 0..3 {
			data.marks.push(Mark {
				id: format!("m{i}"),
				category_id: Some("c1".into()),
				page: 1,
				x: 0.1 * i as f64,
				y: 0.1,
				w: 0.01,
				h: 0.01,
				source: Source::Ai,
				status: MarkStatus::Pending,
				tile: None,
				note: None,
				fields: None,
				meta: Default::default(),
			});
		}
		let doc = Document::new(data, vec![(1000.0, 500.0)], "me");
		let s = summary(&doc);
		assert_eq!(s[0].items, 3);
		assert_eq!(s[0].cost, Some(37.5));
		let csv = rows_csv(&doc, &rows(&doc));
		assert!(csv.contains("\"Duplex, 20A\""));
		assert_eq!(csv.lines().count(), 4);
	}

	fn doc_with_costs() -> Document {
		let mut data = TakeoffData::default();
		let mut c = Category::new("c1", "Duplex, 20A", "#4CC2FF", CategoryKind::Count);
		c.unit_cost = Some(12.5);
		data.categories.push(c);
		data.custom_fields.push(FieldDef { id: "f1".into(), name: "Circuit".into() });
		for i in 0..2 {
			data.marks.push(Mark {
				id: format!("m{i}"),
				category_id: Some("c1".into()),
				page: 2,
				x: 0.1 * i as f64,
				y: 0.1,
				w: 0.01,
				h: 0.01,
				source: Source::User,
				status: MarkStatus::Confirmed,
				tile: None,
				note: Some("by the door".into()),
				fields: Some([("f1".to_string(), "L1-3".to_string())].into()),
				meta: Default::default(),
			});
		}
		Document::new(data, vec![(1000.0, 500.0), (1000.0, 500.0)], "me")
	}

	#[test]
	fn xlsx_matches_csv_with_typed_cells() {
		use calamine::{Data, Reader, Xlsx};
		let doc = doc_with_costs();
		let buf = rows_xlsx(&doc, &rows(&doc)).unwrap();
		let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(buf)).unwrap();
		assert_eq!(wb.sheet_names(), vec!["Markups".to_string(), "Summary".to_string()]);
		let m = wb.worksheet_range("Markups").unwrap();
		// Same header as the CSV, custom column last.
		let csv = rows_csv(&doc, &rows(&doc));
		let header: Vec<String> = (0..m.width()).map(|c| m.get((0, c)).unwrap().to_string()).collect();
		assert_eq!(header.join(","), csv.lines().next().unwrap());
		assert_eq!(m.height(), 3);
		assert_eq!(m.get((1, 0)), Some(&Data::Float(2.0)));
		assert_eq!(m.get((1, 1)), Some(&Data::Empty), "in no space");
		assert_eq!(m.get((1, 3)), Some(&Data::String("Duplex, 20A".into())));
		assert_eq!(m.get((1, 4)), Some(&Data::Float(1.0)));
		assert_eq!(m.get((2, 4)), Some(&Data::Float(2.0)));
		assert_eq!(m.get((1, 7)), Some(&Data::Float(1.0)));
		assert_eq!(m.get((1, 16)), Some(&Data::String("L1-3".into())));
		let s = wb.worksheet_range("Summary").unwrap();
		assert_eq!(s.get((1, 2)), Some(&Data::Float(2.0)));
		assert_eq!(s.get((1, 6)), Some(&Data::Float(12.5)));
		assert_eq!(s.get((1, 7)), Some(&Data::Float(25.0)));
		// Total cost row.
		assert_eq!(s.get((2, 0)), Some(&Data::String("Total".into())));
		assert_eq!(s.get((2, 7)), Some(&Data::Float(25.0)));
	}

	#[test]
	fn summary_xlsx_without_costs_has_no_total() {
		use calamine::{Data, Reader, Xlsx};
		let mut doc = doc_with_costs();
		doc.data.categories[0].unit_cost = None;
		let buf = summary_xlsx(&summary(&doc)).unwrap();
		let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(buf)).unwrap();
		let s = wb.worksheet_range("Summary").unwrap();
		assert_eq!(s.height(), 2);
		assert_eq!(s.get((0, 7)), Some(&Data::String("Cost".into())));
		assert_eq!(s.get((1, 3)), Some(&Data::Float(2.0)));
		assert_eq!(s.get((1, 7)), Some(&Data::Empty));
	}

	#[test]
	fn spaces_in_rows_csv_and_summary() {
		let mut data = TakeoffData::default();
		data.categories.push(Category::new("c1", "Duplex", "#4CC2FF", CategoryKind::Count));
		let sq = |x0: f64, y0: f64, x1: f64, y1: f64| vec![Point { x: x0, y: y0 }, Point { x: x1, y: y0 }, Point { x: x1, y: y1 }, Point { x: x0, y: y1 }];
		data.spaces.push(Space { id: "u".into(), page: 1, name: "Unit 204".into(), color: "#34D399".into(), points: sq(0.0, 0.0, 0.5, 0.5), hidden: false, meta: Default::default() });
		data.spaces.push(Space { id: "k".into(), page: 1, name: "Kitchen".into(), color: "#FBBF24".into(), points: sq(0.0, 0.0, 0.2, 0.2), hidden: true, meta: Default::default() });
		for (i, x) in [0.1, 0.3, 0.35, 0.8].iter().enumerate() {
			data.marks.push(Mark { id: format!("m{i}"), category_id: Some("c1".into()), page: 1, x: *x, y: 0.1, w: 0.01, h: 0.01, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() });
		}
		let doc = Document::new(data, vec![(1000.0, 1000.0)], "me");
		let r = rows(&doc);
		assert_eq!(r[0].space, "Unit 204 › Kitchen", "hidden spaces still count");
		assert_eq!(r[0].space_id.as_deref(), Some("k"));
		assert_eq!(r[1].space, "Unit 204");
		assert_eq!(r[3].space, "");
		let csv = rows_csv(&doc, &r);
		assert!(csv.starts_with("Page,Space,Type"));
		assert!(csv.lines().nth(1).unwrap().starts_with("1,Unit 204 › Kitchen,Count"));
		let groups = summary_by_space(&doc);
		let names: Vec<(&str, usize)> = groups.iter().map(|g| (g.name.as_str(), g.lines[0].items)).collect();
		assert_eq!(names, [("Unit 204", 2), ("Unit 204 › Kitchen", 1), ("No space", 1)]);
		assert_eq!(groups.iter().map(|g| g.lines[0].items).sum::<usize>(), summary(&doc)[0].items, "groups add up");
		let csv = space_summary_csv(&groups);
		assert!(csv.starts_with("Space,Item,"));
		assert_eq!(csv.lines().count(), 4);
		use calamine::{Data, Reader, Xlsx};
		let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(rows_xlsx(&doc, &r).unwrap())).unwrap();
		assert_eq!(wb.sheet_names(), vec!["Markups".to_string(), "Summary".to_string(), "By space".to_string()]);
		let m = wb.worksheet_range("Markups").unwrap();
		assert_eq!(m.get((1, 1)), Some(&Data::String("Unit 204 › Kitchen".into())));
		let b = wb.worksheet_range("By space").unwrap();
		assert_eq!(b.height(), 4);
		assert_eq!(b.get((1, 0)), Some(&Data::String("Unit 204".into())));
		assert_eq!(b.get((1, 3)), Some(&Data::Float(2.0)));
	}
}
