//! Bluebeam-style badge numbers in READING ORDER: page → rows top to bottom →
//! left to right, counted per category. Rows are greedy clusters (a mark joins
//! the row when its y is within ROW_TOL of the row's running mean), so a
//! slightly jittered row still reads as one row with continuous numbers.
//! Must match construction.live `buildSeqMap` exactly.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::model::{Mark, Measurement};

const ROW_TOL: f64 = 0.02;

struct Item<'a> {
	id: &'a str,
	category: Option<&'a str>,
	page: u32,
	x: f64,
	y: f64,
}

fn cmp_f(a: f64, b: f64) -> Ordering {
	a.total_cmp(&b)
}

fn reading_order<'a>(mut items: Vec<Item<'a>>) -> Vec<Item<'a>> {
	items.sort_by(|a, b| {
		a.page.cmp(&b.page).then(cmp_f(a.y, b.y)).then(cmp_f(a.x, b.x)).then(a.id.cmp(b.id))
	});
	let mut ordered = Vec::with_capacity(items.len());
	let mut row: Vec<Item<'a>> = Vec::new();
	let mut row_page = 0u32;
	let mut row_mean_y = 0.0;
	let flush = |row: &mut Vec<Item<'a>>, out: &mut Vec<Item<'a>>| {
		row.sort_by(|a, b| cmp_f(a.x, b.x).then(cmp_f(a.y, b.y)).then(a.id.cmp(b.id)));
		out.append(row);
	};
	for item in items {
		if row.is_empty() || item.page != row_page || item.y - row_mean_y > ROW_TOL {
			flush(&mut row, &mut ordered);
			row_page = item.page;
			row_mean_y = item.y;
			row.push(item);
		} else {
			row_mean_y = (row_mean_y * row.len() as f64 + item.y) / (row.len() as f64 + 1.0);
			row.push(item);
		}
	}
	flush(&mut row, &mut ordered);
	ordered
}

fn seq(ordered: Vec<Item<'_>>) -> HashMap<String, u32> {
	let mut counters: HashMap<Option<&str>, u32> = HashMap::new();
	let mut out = HashMap::with_capacity(ordered.len());
	for item in ordered {
		let n = counters.entry(item.category).or_insert(0);
		*n += 1;
		out.insert(item.id.to_string(), *n);
	}
	out
}

fn min_or_zero(values: impl Iterator<Item = f64>) -> f64 {
	values.reduce(f64::min).unwrap_or(0.0)
}

pub fn mark_seq(marks: &[Mark]) -> HashMap<String, u32> {
	seq(reading_order(
		marks
			.iter()
			.map(|m| Item { id: &m.id, category: m.category_id.as_deref(), page: m.page, x: m.x, y: m.y })
			.collect(),
	))
}

/// Measurements are anchored at the top-left corner of their bounding box.
pub fn measurement_seq(measurements: &[Measurement]) -> HashMap<String, u32> {
	seq(reading_order(
		measurements
			.iter()
			.map(|m| Item {
				id: &m.id,
				category: m.category_id.as_deref(),
				page: m.page,
				x: min_or_zero(m.points.iter().map(|p| p.x)),
				y: min_or_zero(m.points.iter().map(|p| p.y)),
			})
			.collect(),
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::model::{MarkStatus, Source};

	fn mark(id: &str, x: f64, y: f64) -> Mark {
		Mark {
			id: id.into(),
			category_id: Some("c1".into()),
			page: 1,
			x,
			y,
			w: 0.01,
			h: 0.01,
			source: Source::Ai,
			status: MarkStatus::Pending,
			tile: None,
			note: None,
			fields: None,
			meta: Default::default(),
		}
	}

	#[test]
	fn jittered_row_numbers_left_to_right() {
		// b sits slightly ABOVE a but in the same visual row, c is the next row.
		let marks = vec![mark("a", 0.1, 0.500), mark("b", 0.3, 0.495), mark("c", 0.05, 0.6)];
		let s = mark_seq(&marks);
		assert_eq!((s["a"], s["b"], s["c"]), (1, 2, 3));
	}
}
