//! Overlays: an SVG or an image file laid over part of a page at some
//! opacity. Rasterized here for both the viewer (as a texture) and the
//! agent's renders (composited in), so the two always show the same thing.

use image::RgbaImage;
use resvg::tiny_skia::{Pixmap, Transform};
use takeoff_core::{Overlay, TakeoffData};

use crate::FracRect;

/// Largest edge an overlay is rasterized at.
pub const MAX_EDGE: u32 = 4096;

/// The overlay's picture at exactly `w`×`h` px (stretched to its box, like an
/// SVG with `preserveAspectRatio="none"`), straight (non-premultiplied) alpha.
pub fn rasterize(o: &Overlay, w: u32, h: u32) -> Result<RgbaImage, String> {
	let (w, h) = (w.clamp(1, MAX_EDGE), h.clamp(1, MAX_EDGE));
	if let Some(svg) = &o.svg {
		return rasterize_svg(svg, w, h);
	}
	if let Some(path) = &o.image {
		let img = image::open(path).map_err(|e| format!("could not read overlay image {path}: {e}"))?.to_rgba8();
		return Ok(if img.dimensions() == (w, h) { img } else { image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle) });
	}
	Err("overlay has neither svg nor image".into())
}

pub fn rasterize_svg(svg: &str, w: u32, h: u32) -> Result<RgbaImage, String> {
	// Text in overlays renders in the app's bundled font (no system font scan).
	let mut opt = resvg::usvg::Options { font_family: "Inter".into(), ..Default::default() };
	opt.fontdb_mut().load_font_data(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf").to_vec());
	let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| format!("invalid SVG: {e}"))?;
	let mut pix = Pixmap::new(w, h).ok_or("empty overlay")?;
	let size = tree.size();
	let t = Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
	resvg::render(&tree, t, &mut pix.as_mut());
	let mut out = RgbaImage::new(w, h);
	for (dst, src) in out.pixels_mut().zip(pix.pixels()) {
		let c = src.demultiply();
		*dst = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
	}
	Ok(out)
}

/// Blend the page's visible overlays into `img`, which shows `region` of `page`.
/// Overlays that fail to load are skipped (the viewer reports them).
pub fn composite(img: &mut RgbaImage, region: FracRect, data: &TakeoffData, page: u32) {
	for o in data.overlays.iter().filter(|o| o.page == page && !o.hidden && o.opacity > 0.0) {
		let _ = composite_one(img, region, o);
	}
}

fn composite_one(img: &mut RgbaImage, region: FracRect, o: &Overlay) -> Result<(), String> {
	let (iw, ih) = (img.width() as f64, img.height() as f64);
	// Overlay box in this image's px (may run off the image).
	let x0 = (o.x - region.x) / region.w * iw;
	let y0 = (o.y - region.y) / region.h * ih;
	let ow = o.w / region.w * iw;
	let oh = o.h / region.h * ih;
	if ow < 1.0 || oh < 1.0 || x0 >= iw || y0 >= ih || x0 + ow <= 0.0 || y0 + oh <= 0.0 {
		return Ok(());
	}
	// Rasterize at the size it appears, capped; scale up into place if capped.
	let s = (MAX_EDGE as f64 / ow.max(oh)).min(1.0);
	let pic = rasterize(o, (ow * s).round() as u32, (oh * s).round() as u32)?;
	let (pw, ph) = (pic.width() as f64, pic.height() as f64);
	let op = o.opacity.clamp(0.0, 1.0) as f32;
	let (xa, xb) = (x0.max(0.0) as u32, (x0 + ow).min(iw).ceil() as u32);
	let (ya, yb) = (y0.max(0.0) as u32, (y0 + oh).min(ih).ceil() as u32);
	for y in ya..yb.min(img.height()) {
		let sy = (((y as f64 + 0.5 - y0) / oh * ph) as u32).min(pic.height() - 1);
		for x in xa..xb.min(img.width()) {
			let sx = (((x as f64 + 0.5 - x0) / ow * pw) as u32).min(pic.width() - 1);
			let src = pic.get_pixel(sx, sy).0;
			let a = src[3] as f32 / 255.0 * op;
			if a <= 0.0 {
				continue;
			}
			let dst = &mut img.get_pixel_mut(x, y).0;
			for k in 0..3 {
				dst[k] = (src[k] as f32 * a + dst[k] as f32 * (1.0 - a)).round() as u8;
			}
			dst[3] = ((a + dst[3] as f32 / 255.0 * (1.0 - a)) * 255.0).round() as u8;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn overlay(svg: &str, opacity: f64) -> Overlay {
		Overlay {
			id: "ov_1".into(),
			page: 1,
			x: 0.5,
			y: 0.0,
			w: 0.5,
			h: 1.0,
			svg: Some(svg.into()),
			image: None,
			opacity,
			label: None,
			hidden: false,
			source: None,
			meta: Default::default(),
		}
	}

	#[test]
	fn svg_overlay_blends_into_its_box_only() {
		let mut data = TakeoffData::default();
		data.overlays.push(overlay(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##, 0.5));
		let mut img = RgbaImage::from_pixel(100, 50, image::Rgba([255, 255, 255, 255]));
		composite(&mut img, FracRect::FULL, &data, 1);
		assert_eq!(img.get_pixel(10, 25).0, [255, 255, 255, 255], "left half untouched");
		let p = img.get_pixel(75, 25).0;
		assert_eq!(p[0], 255);
		assert!((p[1] as i32 - 128).abs() <= 2, "half-transparent red over white, got {p:?}");
	}

	#[test]
	fn hidden_and_other_page_overlays_are_skipped() {
		let mut data = TakeoffData::default();
		let mut o = overlay(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#, 1.0);
		o.hidden = true;
		data.overlays.push(o.clone());
		o.hidden = false;
		o.page = 2;
		data.overlays.push(o);
		let mut img = RgbaImage::from_pixel(20, 20, image::Rgba([255, 255, 255, 255]));
		composite(&mut img, FracRect::FULL, &data, 1);
		assert!(img.pixels().all(|p| p.0 == [255, 255, 255, 255]));
	}
}
