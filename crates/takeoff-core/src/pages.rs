//! Keep a takeoff in step with page edits: when pages are deleted, moved,
//! rotated or merged in from another file, items follow their page.

use std::collections::HashSet;

use crate::model::*;

/// Renumber pages with `map(old) -> Some(new)`; items on pages mapped to None are dropped.
pub fn remap_pages(d: &mut TakeoffData, map: impl Fn(u32) -> Option<u32>) {
	d.marks.retain_mut(|m| map(m.page).map(|p| m.page = p).is_some());
	d.measurements.retain_mut(|m| map(m.page).map(|p| m.page = p).is_some());
	d.annotations.retain_mut(|a| map(a.page).map(|p| a.page = p).is_some());
	d.spaces.retain_mut(|s| map(s.page).map(|p| s.page = p).is_some());
	d.legends.retain_mut(|l| map(l.page).map(|p| l.page = p).is_some());
	d.page_scales.retain_mut(|s| map(s.page).map(|p| s.page = p).is_some());
	d.sheet_revisions.retain_mut(|r| map(r.page).map(|p| r.page = p).is_some());
	d.viewports.retain_mut(|v| {
		map(v.page)
			.map(|p| {
				v.page = p;
				v.scale.page = p;
			})
			.is_some()
	});
	for c in &mut d.categories {
		if let Some(r) = &mut c.legend_region {
			match map(r.page) {
				Some(p) => r.page = p,
				None => c.legend_region = None,
			}
		}
	}
}

/// The page map for "delete these pages" (1-based) in a document of `count` pages.
pub fn delete_map(count: u32, deleted: &HashSet<u32>) -> impl Fn(u32) -> Option<u32> {
	let mut table = vec![None; count as usize + 1];
	let mut next = 1;
	for p in 1..=count {
		if !deleted.contains(&p) {
			table[p as usize] = Some(next);
			next += 1;
		}
	}
	move |p| table.get(p as usize).copied().flatten()
}

/// The page map for "new order": `order[i]` is the OLD page shown at new position i+1.
pub fn reorder_map(order: &[u32]) -> impl Fn(u32) -> Option<u32> {
	let mut table = std::collections::HashMap::new();
	for (i, old) in order.iter().enumerate() {
		table.entry(*old).or_insert(i as u32 + 1);
	}
	move |p| table.get(&p).copied()
}

/// Insert `n` pages before `at` (1-based): later pages shift down.
pub fn insert_map(at: u32, n: u32) -> impl Fn(u32) -> Option<u32> {
	move |p| Some(if p >= at { p + n } else { p })
}

fn rotate_point(p: Point, quarter_turns_cw: u32) -> Point {
	match quarter_turns_cw % 4 {
		1 => Point { x: 1.0 - p.y, y: p.x },
		2 => Point { x: 1.0 - p.x, y: 1.0 - p.y },
		3 => Point { x: p.y, y: 1.0 - p.x },
		_ => p,
	}
}

/// The page itself turned clockwise by 90° × n: items turn with it.
pub fn rotate_page_items(d: &mut TakeoffData, page: u32, quarter_turns_cw: u32) {
	let q = quarter_turns_cw % 4;
	if q == 0 {
		return;
	}
	let rot = |p: &mut Point| *p = rotate_point(*p, q);
	for m in d.marks.iter_mut().filter(|m| m.page == page) {
		let a = rotate_point(Point { x: m.x, y: m.y }, q);
		let b = rotate_point(Point { x: m.x + m.w, y: m.y + m.h }, q);
		m.x = a.x.min(b.x);
		m.y = a.y.min(b.y);
		m.w = (a.x - b.x).abs();
		m.h = (a.y - b.y).abs();
	}
	for m in d.measurements.iter_mut().filter(|m| m.page == page) {
		m.points.iter_mut().for_each(rot);
		m.cutouts.iter_mut().flatten().for_each(rot);
	}
	for a in d.annotations.iter_mut().filter(|a| a.page == page) {
		a.points.iter_mut().for_each(rot);
	}
	for s in d.spaces.iter_mut().filter(|s| s.page == page) {
		s.points.iter_mut().for_each(rot);
	}
	for l in d.legends.iter_mut().filter(|l| l.page == page) {
		// The box turns with the page; its text stays upright.
		let a = rotate_point(Point { x: l.x, y: l.y }, q);
		let b = rotate_point(Point { x: l.x + l.w, y: l.y + l.h }, q);
		(l.x, l.y, l.w, l.h) = (a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs());
	}
	for s in d.page_scales.iter_mut().filter(|s| s.page == page) {
		rot(&mut s.p1);
		rot(&mut s.p2);
	}
	for v in d.viewports.iter_mut().filter(|v| v.page == page) {
		let a = rotate_point(Point { x: v.x, y: v.y }, q);
		let b = rotate_point(Point { x: v.x + v.w, y: v.y + v.h }, q);
		(v.x, v.y, v.w, v.h) = (a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs());
		rot(&mut v.scale.p1);
		rot(&mut v.scale.p2);
	}
}

/// Append another takeoff's items, its pages shifted by `offset` (merge files).
/// Categories with the same name are joined; ids are kept unless they clash.
pub fn append_takeoff(d: &mut TakeoffData, other: &TakeoffData, offset: u32) {
	let mut cat_map = std::collections::HashMap::new();
	for c in &other.categories {
		if let Some(existing) = d.categories.iter().find(|x| x.name == c.name) {
			cat_map.insert(c.id.clone(), existing.id.clone());
		} else {
			let other_id = c.id.clone();
			let mut c = c.clone();
			if d.categories.iter().any(|x| x.id == c.id) {
				c.id = new_id("c");
			}
			cat_map.insert(other_id, c.id.clone());
			d.categories.push(c);
		}
	}
	let fix_cat = |id: &Option<String>| id.as_ref().and_then(|i| cat_map.get(i).cloned());
	let mut taken: HashSet<String> = d.marks.iter().map(|m| m.id.clone()).chain(d.measurements.iter().map(|m| m.id.clone())).chain(d.annotations.iter().map(|a| a.id.clone())).collect();
	let mut fresh = |id: &str, prefix: &str| if taken.insert(id.to_string()) { id.to_string() } else { new_id(prefix) };
	for m in &other.marks {
		d.marks.push(Mark { id: fresh(&m.id, "m"), page: m.page + offset, category_id: fix_cat(&m.category_id), ..m.clone() });
	}
	for m in &other.measurements {
		d.measurements.push(Measurement { id: fresh(&m.id, "ms"), page: m.page + offset, category_id: fix_cat(&m.category_id), ..m.clone() });
	}
	for a in &other.annotations {
		d.annotations.push(Annotation { id: fresh(&a.id, "an"), page: a.page + offset, ..a.clone() });
	}
	for s in &other.spaces {
		d.spaces.push(Space { id: fresh(&s.id, "sp"), page: s.page + offset, ..s.clone() });
	}
	for l in &other.legends {
		d.legends.push(Legend { id: fresh(&l.id, "lg"), page: l.page + offset, category_ids: l.category_ids.iter().filter_map(|c| cat_map.get(c).cloned()).collect(), ..l.clone() });
	}
	for s in &other.page_scales {
		d.page_scales.push(PageScale { page: s.page + offset, ..s.clone() });
	}
	for v in &other.viewports {
		let mut v = v.clone();
		v.page += offset;
		v.scale.page += offset;
		d.viewports.push(v);
	}
}

/// "1-3, 5, 8-" → [1,2,3,5,8,…,count] (1-based, deduped, in the written order).
pub fn parse_page_ranges(spec: &str, count: u32) -> Result<Vec<u32>, String> {
	let mut out = Vec::new();
	let mut seen = HashSet::new();
	for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
		let (a, b) = match part.split_once('-') {
			Some((a, b)) => (a.trim(), b.trim()),
			None => (part, part),
		};
		let parse = |s: &str, default: u32| -> Result<u32, String> {
			if s.is_empty() {
				Ok(default)
			} else {
				s.parse::<u32>().map_err(|_| format!("'{s}' is not a page number"))
			}
		};
		let (a, b) = (parse(a, 1)?, parse(b, count)?);
		if a == 0 || b == 0 || a > count || b > count {
			return Err(format!("page range {part} is outside 1-{count}"));
		}
		let range: Box<dyn Iterator<Item = u32>> = if a <= b { Box::new(a..=b) } else { Box::new((b..=a).rev()) };
		for p in range {
			if seen.insert(p) {
				out.push(p);
			}
		}
	}
	if out.is_empty() {
		return Err("no pages selected".into());
	}
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn mark(page: u32) -> Mark {
		Mark { id: new_id("m"), category_id: None, page, x: 0.1, y: 0.2, w: 0.1, h: 0.05, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() }
	}

	#[test]
	fn delete_and_reorder() {
		let mut d = TakeoffData::default();
		d.marks = vec![mark(1), mark(2), mark(3)];
		remap_pages(&mut d, delete_map(3, &[2].into()));
		assert_eq!(d.marks.iter().map(|m| m.page).collect::<Vec<_>>(), vec![1, 2]);
		remap_pages(&mut d, reorder_map(&[2, 1]));
		assert_eq!(d.marks.iter().map(|m| m.page).collect::<Vec<_>>(), vec![2, 1]);
	}

	#[test]
	fn rotate_four_times_is_identity() {
		let mut d = TakeoffData::default();
		d.marks = vec![mark(1)];
		let before = d.marks[0].clone();
		rotate_page_items(&mut d, 1, 1);
		assert!((d.marks[0].x - 0.75).abs() < 1e-12 && (d.marks[0].w - 0.05).abs() < 1e-12);
		for _ in 0..3 {
			rotate_page_items(&mut d, 1, 1);
		}
		assert!((d.marks[0].x - before.x).abs() < 1e-12 && (d.marks[0].y - before.y).abs() < 1e-12);
	}

	#[test]
	fn ranges() {
		assert_eq!(parse_page_ranges("1-3, 5, 8-", 9).unwrap(), vec![1, 2, 3, 5, 8, 9]);
		assert_eq!(parse_page_ranges("3-1", 9).unwrap(), vec![3, 2, 1]);
		assert!(parse_page_ranges("0", 9).is_err());
		assert!(parse_page_ranges("12", 9).is_err());
	}
}
