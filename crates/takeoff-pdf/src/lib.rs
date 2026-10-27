//! Drawing access for Takeoff Desk: open a PDF, read page sizes and text,
//! render a page or a region of it at a given DPI.
//!
//! Regions are full-page fractions (see `takeoff_core`). A region render is
//! the same pixels as the matching crop of a full-page render at that DPI,
//! without rasterizing the rest of the sheet.

pub mod burn;
pub mod compare;
pub mod cos;
pub mod export;
pub mod fill;
pub mod layers;
pub mod ops;
pub mod outline;
pub mod overlay;
pub mod search;
pub mod sheets;
pub mod slip;
pub mod tools;
pub mod trace;
pub mod vectors;

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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
	#[error("{0}")]
	Io(String),
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
	layers: Option<layers::LayerInfo>,
	hidden: HashSet<String>,
}

/// Original file bytes (memory-mapped) followed by an incremental update.
struct ChainReader {
	base: Arc<memmap2::Mmap>,
	tail: Vec<u8>,
	pos: u64,
}

impl Read for ChainReader {
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		let base_len = self.base.len() as u64;
		let total = base_len + self.tail.len() as u64;
		if self.pos >= total {
			return Ok(0);
		}
		let n = if self.pos < base_len {
			let src = &self.base[self.pos as usize..];
			let n = src.len().min(buf.len());
			buf[..n].copy_from_slice(&src[..n]);
			n
		} else {
			let src = &self.tail[(self.pos - base_len) as usize..];
			let n = src.len().min(buf.len());
			buf[..n].copy_from_slice(&src[..n]);
			n
		};
		self.pos += n as u64;
		Ok(n)
	}
}

impl Seek for ChainReader {
	fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
		let total = (self.base.len() + self.tail.len()) as i64;
		let next = match to {
			SeekFrom::Start(p) => p as i64,
			SeekFrom::End(d) => total + d,
			SeekFrom::Current(d) => self.pos as i64 + d,
		};
		if next < 0 {
			return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
		}
		self.pos = next as u64;
		Ok(self.pos)
	}
}

impl Drawing {
	pub fn open(path: impl AsRef<Path>) -> Result<Self, PdfError> {
		let path = path.as_ref().to_path_buf();
		// Two threads loading at once can fail inside pdfium (parallel
		// subagents each open the drawing): one load at a time.
		static OPENING: std::sync::Mutex<()> = std::sync::Mutex::new(());
		let doc = {
			let _one = OPENING.lock().unwrap_or_else(|e| e.into_inner());
			pdfium()?.load_pdf_from_file(&path, None)?
		};
		// Layer list is optional: a parse failure just means "no layer control".
		let layers = layers::LayerInfo::read(&path).ok().flatten();
		Ok(Self { doc, path, layers, hidden: HashSet::new() })
	}

	/// The PDF's layers (optional content groups), de-duplicated by name.
	pub fn layer_names(&self) -> Vec<String> {
		self.layers.as_ref().map(|l| l.names()).unwrap_or_default()
	}

	/// Names of layers the document itself hides by default.
	pub fn default_off_layers(&self) -> Vec<String> {
		self.layers
			.as_ref()
			.map(|l| l.layers.iter().filter(|x| x.default_off).map(|x| x.name.clone()).collect())
			.unwrap_or_default()
	}

	pub fn hidden_layers(&self) -> &HashSet<String> {
		&self.hidden
	}

	/// Hide exactly these layers (by name) in every later render. Reloads the
	/// document from "file + small update" in memory; the file is untouched.
	pub fn set_hidden_layers(&mut self, hidden: &HashSet<String>) -> Result<(), PdfError> {
		if *hidden == self.hidden {
			return Ok(());
		}
		let Some(info) = &self.layers else { return Ok(()) };
		let doc = if hidden.is_empty() && info.layers.iter().all(|l| !l.default_off) {
			pdfium()?.load_pdf_from_file(&self.path, None)?
		} else {
			let reader = ChainReader { base: info.mmap(), tail: info.update_hiding(hidden), pos: 0 };
			pdfium()?.load_pdf_from_reader(reader, None)?
		};
		self.doc = doc;
		self.hidden = hidden.clone();
		Ok(())
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
		self.render_lines(page, dpi, region, max_px, true)
	}

	/// [`Drawing::render`]; `smooth: false` draws lines without anti-aliasing,
	/// so a hairline stays a solid pixel wide at any resolution (for reading
	/// the line work, not for looking at).
	pub fn render_lines(&self, page: u32, dpi: f32, region: FracRect, max_px: Option<u32>, smooth: bool) -> Result<Render, PdfError> {
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
			.set_path_smoothing(smooth)
			.set_clear_color(PdfColor::WHITE);
		p.render_into_bitmap_with_config(&mut bitmap, &config)?;
		let image = RgbaImage::from_raw(out_w as u32, out_h as u32, bitmap.as_rgba_bytes())
			.ok_or_else(|| PdfError::Io(format!("pdfium bitmap is not {out_w} × {out_h}")))?;
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

	/// Number of the bookmark tree, flattened with depth.
	pub fn outline(&self) -> Vec<outline::Bookmark> {
		outline::outline(&self.doc)
	}

	/// Stroked line work on a page, for snapping.
	pub fn segments(&self, page: u32) -> Result<Vec<takeoff_core::snap::Segment>, PdfError> {
		Ok(vectors::page_segments(&self.page(page)?)?)
	}

	/// Search one page.
	pub fn search_page(&self, page: u32, query: &str, match_case: bool, whole_word: bool, limit: usize) -> Result<Vec<search::Hit>, PdfError> {
		Ok(search::search_page(&self.page(page)?, page, query, match_case, whole_word, limit))
	}

	/// Positioned text lines (optionally within a region of the page).
	pub fn text_runs(&self, page: u32, region: Option<FracRect>, limit: usize) -> Result<Vec<search::TextRun>, PdfError> {
		Ok(search::text_runs(&self.page(page)?, region.map(|r| (r.x, r.y, r.w, r.h)), limit))
	}

	/// The underlying pdfium document (for exports that draw into it).
	pub fn document(&self) -> &PdfDocument<'static> {
		&self.doc
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
	(x1 > x0 && y1 > y0).then_some(FracRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}
