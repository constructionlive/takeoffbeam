//! Drawing access for Takeoff Desk: open a PDF, read page sizes and text,
//! render a page or a region of it at a given DPI.
//!
//! Regions are full-page fractions (see `takeoff_core`). A region render is
//! the same pixels as the matching crop of a full-page render at that DPI,
//! without rasterizing the rest of the sheet.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::RgbaImage;
use pdfium_render::prelude::*;

pub use pdfium_render::prelude::PdfiumError;

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
	#[error("pdfium library not found (looked in: {0})")]
	LibraryNotFound(String),
	#[error("pdfium: {0}")]
	Pdfium(#[from] PdfiumError),
	#[error("page {0} out of range (1..={1})")]
	PageOutOfRange(u32, u32),
	#[error("empty render region")]
	EmptyRegion,
}

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// Places we look for libpdfium, in order: `$TAKEOFF_PDFIUM_DIR`, next to the
/// executable, the app bundle's Frameworks dir, then the repo's `vendor/pdfium/lib`
/// (dev builds; filled by `scripts/fetch-pdfium.sh`).
fn library_dirs() -> Vec<PathBuf> {
	let mut dirs = Vec::new();
	if let Ok(d) = std::env::var("TAKEOFF_PDFIUM_DIR") {
		dirs.push(PathBuf::from(d));
	}
	if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
		dirs.push(exe_dir.join("../Frameworks"));
		dirs.push(exe_dir.clone());
	}
	dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib"));
	dirs
}

pub fn pdfium() -> Result<&'static Pdfium, PdfError> {
	let loaded = PDFIUM.get_or_init(|| {
		let dirs = library_dirs();
		for dir in &dirs {
			let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
			if lib.exists() {
				return Pdfium::bind_to_library(&lib).map(Pdfium::new).map_err(|e| e.to_string());
			}
		}
		Pdfium::bind_to_system_library().map(Pdfium::new).map_err(|_| {
			dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")
		})
	});
	loaded.as_ref().map_err(|e| PdfError::LibraryNotFound(e.clone()))
}

/// A rectangle as fractions of the full page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FracRect {
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

impl FracRect {
	pub const FULL: FracRect = FracRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
}

/// What came out of a render, with the geometry needed to map pixels back to the page.
pub struct Render {
	pub image: RgbaImage,
	/// Full-page size in px at the DPI actually used.
	pub page_px: (u32, u32),
	/// The region that was rendered (fractions), after clamping to the page.
	pub region: FracRect,
	pub dpi: f32,
}

pub struct Drawing {
	doc: PdfDocument<'static>,
	path: PathBuf,
}

impl Drawing {
	pub fn open(path: impl AsRef<Path>) -> Result<Self, PdfError> {
		let path = path.as_ref().to_path_buf();
		let doc = pdfium()?.load_pdf_from_file(&path, None)?;
		Ok(Self { doc, path })
	}

	pub fn path(&self) -> &Path {
		&self.path
	}

	pub fn page_count(&self) -> u32 {
		self.doc.pages().len() as u32
	}

	fn page(&self, page: u32) -> Result<PdfPage<'_>, PdfError> {
		let n = self.page_count();
		if page == 0 || page > n {
			return Err(PdfError::PageOutOfRange(page, n));
		}
		Ok(self.doc.pages().get((page - 1) as i32)?)
	}

	/// Page size in PDF points (1/72 in), as displayed (rotation applied).
	/// Cheap: pdfium reads the size without loading the page content.
	pub fn page_size_pt(&self, page: u32) -> Result<(f32, f32), PdfError> {
		let n = self.page_count();
		if page == 0 || page > n {
			return Err(PdfError::PageOutOfRange(page, n));
		}
		let r = self.doc.pages().page_size((page - 1) as i32)?;
		Ok((r.width().value, r.height().value))
	}

	/// Page label as printed in the PDF (e.g. "TMP-1"), if the file has labels.
	pub fn page_label(&self, page: u32) -> Result<Option<String>, PdfError> {
		Ok(self.page(page)?.label().map(str::to_string))
	}

	/// Renders `region` of `page` at `dpi`. When `max_px` is set and the output
	/// would be larger on its long edge, the DPI is lowered to fit.
	pub fn render(&self, page: u32, dpi: f32, region: FracRect, max_px: Option<u32>) -> Result<Render, PdfError> {
		let p = self.page(page)?;
		let region = clamp_region(region).ok_or(PdfError::EmptyRegion)?;
		let (pw_pt, ph_pt) = (p.width().value as f64, p.height().value as f64);
		let mut dpi = dpi as f64;
		if let Some(max) = max_px {
			let long_edge_px = (pw_pt * region.w).max(ph_pt * region.h) * dpi / 72.0;
			if long_edge_px > max as f64 {
				dpi *= max as f64 / long_edge_px;
			}
		}
		let page_w = (pw_pt * dpi / 72.0).round().max(1.0) as i32;
		let page_h = (ph_pt * dpi / 72.0).round().max(1.0) as i32;
		let left = (region.x * page_w as f64).floor() as i32;
		let top = (region.y * page_h as f64).floor() as i32;
		let out_w = (((region.x + region.w) * page_w as f64).ceil() as i32 - left).max(1);
		let out_h = (((region.y + region.h) * page_h as f64).ceil() as i32 - top).max(1);

		let mut bitmap = PdfBitmap::empty(out_w, out_h, PdfBitmapFormat::BGRA)?;
		let config = PdfRenderConfig::new()
			.set_fixed_size(page_w, page_h)
			.set_origin(-left, -top)
			.render_form_data(true)
			.render_annotations(true)
			.clear_before_rendering(true)
			.set_clear_color(PdfColor::WHITE);
		p.render_into_bitmap_with_config(&mut bitmap, &config)?;
		let image = RgbaImage::from_raw(out_w as u32, out_h as u32, bitmap.as_rgba_bytes())
			.expect("bitmap size matches");
		Ok(Render {
			image,
			page_px: (page_w as u32, page_h as u32),
			region: FracRect {
				x: left as f64 / page_w as f64,
				y: top as f64 / page_h as f64,
				w: out_w as f64 / page_w as f64,
				h: out_h as f64 / page_h as f64,
			},
			dpi: dpi as f32,
		})
	}

	/// All text on the page in reading order (as pdfium extracts it).
	pub fn text(&self, page: u32) -> Result<String, PdfError> {
		Ok(self.page(page)?.text()?.all())
	}

	/// Text inside a region of the page.
	pub fn text_in(&self, page: u32, region: FracRect) -> Result<String, PdfError> {
		let p = self.page(page)?;
		let region = clamp_region(region).ok_or(PdfError::EmptyRegion)?;
		let (w, h) = (p.width().value, p.height().value);
		// PDF space is bottom-left origin.
		let rect = PdfRect::new_from_values(
			h * (1.0 - (region.y + region.h) as f32),
			w * region.x as f32,
			h * (1.0 - region.y as f32),
			w * (region.x + region.w) as f32,
		);
		let text = p.text()?.inside_rect(rect);
		Ok(text)
	}
}

fn clamp_region(r: FracRect) -> Option<FracRect> {
	let x0 = r.x.clamp(0.0, 1.0);
	let y0 = r.y.clamp(0.0, 1.0);
	let x1 = (r.x + r.w).clamp(0.0, 1.0);
	let y1 = (r.y + r.h).clamp(0.0, 1.0);
	(x1 > x0 && y1 > y0).then(|| FracRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}
