//! Page layout (View → Page layout): one page at a time, continuous, or
//! two-page spreads. View only: the takeoff stays per page, in page
//! fractions, exactly as in single-page view.
//!
//! Every page has a box in layout space: PDF points, rows of one or two
//! pages stacked top to bottom with a gap, each row centred on x = 0. The
//! screen is `origin + layout × zoom`. Built from the page sizes alone (the
//! cheap size lookup), so an 859-page set never loads a page to lay out.

use std::ops::RangeInclusive;

use eframe::egui::{pos2, vec2, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PageLayout {
	/// Pages follow one another as you scroll.
	pub continuous: bool,
	/// Two pages side by side.
	pub spreads: bool,
	/// With spreads: page 1 on its own, like a book's cover.
	pub cover: bool,
}

impl PageLayout {
	pub const SINGLE: Self = Self { continuous: false, spreads: false, cover: false };
	pub const CONTINUOUS: Self = Self { continuous: true, spreads: false, cover: false };
	pub const SPREADS: Self = Self { continuous: false, spreads: true, cover: false };
	pub const SPREADS_CONTINUOUS: Self = Self { continuous: true, spreads: true, cover: false };

	/// More than one page can be on screen.
	pub fn multi(self) -> bool {
		self.continuous || self.spreads
	}

	/// The same arrangement, keeping the cover choice.
	pub fn with(self, continuous: bool, spreads: bool) -> Self {
		Self { continuous, spreads, cover: self.cover }
	}

	/// Same mode, ignoring the cover choice (which only matters with spreads).
	pub fn is(self, other: Self) -> bool {
		self.continuous == other.continuous && self.spreads == other.spreads
	}
}

struct Row {
	first: u32,
	last: u32,
	rect: Rect,
}

/// Where every page sits in layout space.
pub struct Arrangement {
	pub layout: PageLayout,
	/// Between rows and between the pages of a spread, in points.
	pub gap: f32,
	rects: Vec<Rect>,
	rows: Vec<Row>,
	row_of: Vec<usize>,
}

impl Default for Arrangement {
	fn default() -> Self {
		Self::new(&[], PageLayout::SINGLE)
	}
}

impl Arrangement {
	pub fn new(sizes: &[(f64, f64)], layout: PageLayout) -> Self {
		let widest = sizes.iter().map(|s| s.0 as f32).fold(0.0, f32::max);
		let gap = (widest * 0.04).clamp(12.0, 96.0);
		let n = sizes.len();
		let (mut rects, mut rows, mut row_of) = (Vec::with_capacity(n), Vec::new(), Vec::with_capacity(n));
		let (mut y, mut i) = (0.0f32, 0usize);
		while i < n {
			let take = if !layout.spreads || (layout.cover && i == 0) { 1 } else { 2.min(n - i) };
			let pages = &sizes[i..i + take];
			let w = pages.iter().map(|s| s.0 as f32).sum::<f32>() + gap * (take - 1) as f32;
			let h = pages.iter().map(|s| s.1 as f32).fold(0.0, f32::max);
			let mut x = -w / 2.0;
			for s in pages {
				rects.push(Rect::from_min_size(pos2(x, y + (h - s.1 as f32) / 2.0), vec2(s.0 as f32, s.1 as f32)));
				row_of.push(rows.len());
				x += s.0 as f32 + gap;
			}
			rows.push(Row { first: i as u32 + 1, last: (i + take) as u32, rect: Rect::from_min_size(pos2(-w / 2.0, y), vec2(w, h)) });
			y += h + gap;
			i += take;
		}
		Self { layout, gap, rects, rows, row_of }
	}

	pub fn page_count(&self) -> u32 {
		self.rects.len() as u32
	}

	/// A page's box (letter-size at the origin for a page not known yet).
	pub fn rect(&self, page: u32) -> Rect {
		self.rects.get(page.wrapping_sub(1) as usize).copied().unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(612.0, 792.0)))
	}

	/// Which row a page is in.
	pub fn row(&self, page: u32) -> usize {
		self.row_of.get(page.wrapping_sub(1) as usize).copied().unwrap_or(0)
	}

	/// The box around a page's row (the page itself without spreads).
	pub fn row_rect(&self, page: u32) -> Rect {
		match self.row_of.get(page.wrapping_sub(1) as usize) {
			Some(r) => self.rows[*r].rect,
			None => self.rect(page),
		}
	}

	/// Where the drawn frame starts: the origin when continuous, else the
	/// top-left of `current`'s row (one row on screen at a time).
	pub fn base(&self, current: u32) -> Vec2 {
		if self.layout.continuous { Vec2::ZERO } else { self.row_rect(current).min.to_vec2() }
	}

	/// The pages this layout puts on screen around `current`: all of them
	/// when continuous, else its row.
	pub fn shown(&self, current: u32) -> RangeInclusive<u32> {
		if self.layout.continuous && !self.rows.is_empty() {
			return 1..=self.page_count();
		}
		match self.row_of.get(current.wrapping_sub(1) as usize) {
			Some(r) => self.rows[*r].first..=self.rows[*r].last,
			None => current..=current,
		}
	}

	/// The rows a continuous layout has between `top` and `bottom`.
	fn rows_between(&self, top: f32, bottom: f32) -> impl Iterator<Item = &Row> {
		let from = self.rows.partition_point(|r| r.rect.bottom() < top);
		self.rows[from..].iter().take_while(move |r| r.rect.top() <= bottom)
	}

	/// The shown pages whose box meets `view` (layout space), in page order.
	pub fn visible(&self, current: u32, view: Rect) -> Vec<u32> {
		if !self.layout.continuous || self.rows.is_empty() {
			return self.shown(current).filter(|p| self.rect(*p).intersects(view)).collect();
		}
		self.rows_between(view.top(), view.bottom()).flat_map(|r| r.first..=r.last).filter(|p| self.rect(*p).intersects(view)).collect()
	}

	/// The shown page under a layout point.
	pub fn page_at(&self, current: u32, pt: Pos2) -> Option<u32> {
		if !self.layout.continuous || self.rows.is_empty() {
			return self.shown(current).find(|p| self.rect(*p).contains(pt));
		}
		self.rows_between(pt.y, pt.y).flat_map(|r| r.first..=r.last).find(|p| self.rect(*p).contains(pt))
	}

	/// The page the view is on when its centre is at `pt`: `current` while
	/// `pt` is still in its row (so a spread doesn't flip as you pan
	/// across it), else the page of the row nearest `pt` that holds it, or
	/// is nearest it.
	pub fn page_for_centre(&self, current: u32, pt: Pos2) -> u32 {
		if self.rows.is_empty() {
			return current;
		}
		let k = self.rows.partition_point(|r| r.rect.bottom() < pt.y).min(self.rows.len() - 1);
		// In the gap above row k: the nearer of it and the one before.
		let k = if k > 0 && pt.y < self.rows[k].rect.top() && pt.y - self.rows[k - 1].rect.bottom() < self.rows[k].rect.top() - pt.y { k - 1 } else { k };
		if self.row_of.get(current.wrapping_sub(1) as usize) == Some(&k) {
			return current;
		}
		let r = &self.rows[k];
		let dist = |p: u32| {
			let b = self.rect(p);
			(b.left() - pt.x).max(pt.x - b.right()).max(0.0)
		};
		(r.first..=r.last).min_by(|a, b| dist(*a).total_cmp(&dist(*b))).unwrap_or(r.first)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const SIZES: [(f64, f64); 5] = [(792.0, 612.0), (612.0, 792.0), (792.0, 612.0), (792.0, 612.0), (1224.0, 792.0)];

	#[test]
	fn continuous_stacks_pages_with_a_gap() {
		let a = Arrangement::new(&SIZES, PageLayout::CONTINUOUS);
		let gap = a.gap;
		assert_eq!(gap, 1224.0 * 0.04);
		assert_eq!(a.rect(1), Rect::from_min_size(pos2(-396.0, 0.0), vec2(792.0, 612.0)));
		assert_eq!(a.rect(2).min, pos2(-306.0, 612.0 + gap));
		assert_eq!(a.rect(3).top(), 612.0 + 792.0 + 2.0 * gap);
		assert_eq!(a.shown(3), 1..=5);
		assert_eq!(a.base(4), Vec2::ZERO);
		// A view across the bottom of page 2 and the top of page 3.
		let view = Rect::from_min_max(pos2(-400.0, 1400.0), pos2(400.0, 1600.0));
		assert_eq!(a.visible(1, view), vec![2, 3]);
		assert_eq!(a.page_at(1, pos2(0.0, 1300.0)), Some(2));
		assert_eq!(a.page_at(1, pos2(0.0, 612.0 + gap / 2.0)), None, "the gap is no page");
		assert_eq!(a.page_at(1, pos2(350.0, 1000.0)), None, "beside a narrow page");
	}

	#[test]
	fn single_page_shows_one_page_from_its_own_corner() {
		let a = Arrangement::new(&SIZES, PageLayout::SINGLE);
		assert_eq!(a.shown(3), 3..=3);
		assert_eq!(a.base(3), a.rect(3).min.to_vec2());
		let everything = Rect::from_min_max(pos2(-1e6, -1e6), pos2(1e6, 1e6));
		assert_eq!(a.visible(3, everything), vec![3]);
		assert_eq!(a.page_at(3, a.rect(2).center()), None, "page 2 isn't on screen");
	}

	#[test]
	fn spreads_pair_pages_and_the_cover_stands_alone() {
		let a = Arrangement::new(&SIZES, PageLayout::SPREADS_CONTINUOUS);
		assert_eq!(a.shown(1), 1..=5);
		assert_eq!((a.row(1), a.row(2), a.row(3), a.row(5)), (0, 0, 1, 2));
		assert_eq!(a.rect(1).top(), a.rect(2).top() + 90.0, "landscape centred beside portrait");
		assert!((a.rect(2).left() - a.rect(1).right() - a.gap).abs() < 1e-3);
		assert_eq!(a.row_rect(1).center().x, 0.0);
		let a = Arrangement::new(&SIZES, PageLayout { cover: true, ..PageLayout::SPREADS });
		assert_eq!(a.shown(1), 1..=1);
		assert_eq!(a.shown(3), 2..=3);
		assert_eq!(a.shown(5), 4..=5);
		assert_eq!(a.base(3), a.row_rect(2).min.to_vec2());
	}

	#[test]
	fn the_centre_picks_the_page_and_spreads_dont_flip() {
		let a = Arrangement::new(&SIZES, PageLayout::CONTINUOUS);
		assert_eq!(a.page_for_centre(1, a.rect(3).center()), 3);
		assert_eq!(a.page_for_centre(1, pos2(0.0, -500.0)), 1, "above the first page");
		assert_eq!(a.page_for_centre(1, pos2(0.0, 1e6)), 5, "past the last");
		let gap_top = a.rect(1).bottom();
		assert_eq!(a.page_for_centre(4, pos2(0.0, gap_top + 1.0)), 1, "in a gap: the nearer page");
		assert_eq!(a.page_for_centre(4, pos2(0.0, gap_top + a.gap - 1.0)), 2);
		let s = Arrangement::new(&SIZES, PageLayout::SPREADS_CONTINUOUS);
		assert_eq!(s.page_for_centre(2, s.rect(1).center()), 2, "still in page 2's spread");
		assert_eq!(s.page_for_centre(1, s.rect(4).center()), 4);
		assert_eq!(s.page_for_centre(1, pos2(s.rect(4).right() + 500.0, s.rect(4).center().y)), 4, "beside a spread: the nearer page");
	}
}
