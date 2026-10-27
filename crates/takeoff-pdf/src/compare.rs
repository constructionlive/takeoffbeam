//! Revision compare: one page of this drawing over a page of another, in
//! colour (red = only here, blue = only there, black = both), plus where the
//! sheet changed as a coarse grid, to find the takeoff items a new revision
//! may have made wrong.

use image::RgbaImage;
use takeoff_core::slip::{fit_transform, ChangeGrid};

use crate::{Drawing, FracRect, PdfError};

pub struct Diff {
	pub image: RgbaImage,
	/// Share of pixels that differ (0–1).
	pub changed: f32,
	/// Changed cells, already grown by one cell.
	pub grid: ChangeGrid,
}

/// Grid cells along the page's long edge.
const GRID_LONG: usize = 64;

/// Compare `page` of `a` with `other_page` of `b`, rendered to the same
/// pixels. When the paper differs in shape, the other page is fitted into
/// this one (evenly scaled, centred), the same way slip-sheeting carries the
/// takeoff, so the overlay and the items line up.
pub fn compare_pages(a: &Drawing, page: u32, b: &Drawing, other_page: u32, max_px: u32) -> Result<Diff, PdfError> {
	let ra = a.render(page, 300.0, FracRect::FULL, Some(max_px))?;
	let (w, h) = ra.image.dimensions();
	let size_a = a.page_size_pt(page)?;
	let size_b = b.page_size_pt(other_page)?;
	let (sx, sy, ox, oy) = fit_transform((size_b.0 as f64, size_b.1 as f64), (size_a.0 as f64, size_a.1 as f64));
	let (bw, bh) = (((w as f64 * sx).round() as u32).max(1), ((h as f64 * sy).round() as u32).max(1));
	let dpi = bw as f32 / size_b.0 * 72.0;
	let rb = b.render(other_page, dpi, FracRect::FULL, None)?;
	let small = if rb.image.dimensions() == (bw, bh) { rb.image } else { image::imageops::resize(&rb.image, bw, bh, image::imageops::FilterType::Triangle) };
	let mut other = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
	image::imageops::overlay(&mut other, &small, (w as f64 * ox).round() as i64, (h as f64 * oy).round() as i64);
	Ok(diff_images(&ra.image, &other))
}

/// The overlay and change grid of two same-sized renders.
pub fn diff_images(a: &RgbaImage, b: &RgbaImage) -> Diff {
	let (w, h) = a.dimensions();
	let (cols, rows) = if w >= h { (GRID_LONG, ((GRID_LONG as f64 * h as f64 / w as f64).round() as usize).max(1)) } else { (((GRID_LONG as f64 * w as f64 / h as f64).round() as usize).max(1), GRID_LONG) };
	let mut counts = vec![0u32; cols * rows];
	let mut out = RgbaImage::new(w, h);
	let mut changed = 0u64;
	let lum = |p: &image::Rgba<u8>| (p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10;
	for (x, y, pa) in a.enumerate_pixels() {
		let pb = b.get_pixel(x.min(b.width().saturating_sub(1)), y.min(b.height().saturating_sub(1)));
		let (la, lb) = (lum(pa).min(255) as u8, lum(pb).min(255) as u8);
		if la.abs_diff(lb) > 60 {
			changed += 1;
			counts[(y as usize * rows / h as usize) * cols + x as usize * cols / w as usize] += 1;
		}
		// Only-this = red, only-other = blue, both = dark, neither = white.
		out.put_pixel(x, y, image::Rgba([lb, la.min(lb), la, 255]));
	}
	// A cell changed when more than a speck in it did (anti-aliasing is noise).
	let cell_px = (w as f64 / cols as f64) * (h as f64 / rows as f64);
	let need = (cell_px * 0.002).max(3.0) as u32;
	let mut grid = ChangeGrid::new(cols, rows);
	for r in 0..rows {
		for c in 0..cols {
			if counts[r * cols + c] >= need {
				grid.set(c, r);
			}
		}
	}
	Diff { image: out, changed: changed as f32 / (w as f32 * h as f32).max(1.0), grid: grid.dilate() }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn grid_finds_the_changed_corner() {
		let a = RgbaImage::from_pixel(640, 320, image::Rgba([255, 255, 255, 255]));
		let mut b = a.clone();
		for y in 250..300 {
			for x in 560..620 {
				b.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
			}
		}
		// One stray pixel elsewhere is noise.
		b.put_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
		let d = diff_images(&a, &b);
		assert!(d.changed > 0.01 && d.changed < 0.02);
		assert!(d.grid.changed_at(takeoff_core::Point { x: 0.92, y: 0.85 }));
		assert!(!d.grid.changed_at(takeoff_core::Point { x: 0.03, y: 0.06 }));
		assert_eq!(d.image.get_pixel(600, 280)[2], 255, "only there = blue");
	}
}
