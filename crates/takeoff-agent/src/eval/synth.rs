//! Synthetic drawings with exact answers, for the committed eval cases:
//! plain PDFs written by hand (no fonts beyond Helvetica, no timestamps), so
//! the bytes are the same on every run and the ground truth is the code.
//!
//! - `symbols.pdf`: an E-sheet at 1/8" = 1'-0" with 9 duplex receptacles and
//!   6 light fixtures in the plan, plus one of each in the legend.
//! - `rooms.pdf`: an A-sheet at 1/4" = 1'-0", OFFICE 101 (20'×15') and
//!   STORAGE 102 (12'×15') with a doorway between, dimensioned.

use std::path::{Path, PathBuf};

/// 17×11 in sheets.
pub const W: f64 = 1224.0;
pub const H: f64 = 792.0;

/// Duplex receptacle centres in the plan (pt, top-down); radius 7, lines ±11.
pub const RECEPTACLES: [(f64, f64); 9] = [(180.0, 96.0), (380.0, 96.0), (700.0, 96.0), (240.0, 664.0), (640.0, 664.0), (860.0, 664.0), (96.0, 300.0), (96.0, 520.0), (904.0, 250.0)];
/// Light fixture centres in the plan (pt, top-down); 20 pt squares with an X.
pub const LIGHTS: [(f64, f64); 6] = [(200.0, 250.0), (380.0, 250.0), (200.0, 500.0), (380.0, 500.0), (710.0, 230.0), (710.0, 530.0)];

/// Each receptacle's box in page fractions (x, y, w, h).
pub fn receptacle_boxes() -> Vec<[f64; 4]> {
	RECEPTACLES.iter().map(|&(x, y)| frac(x - 7.0, y - 11.0, 14.0, 22.0)).collect()
}

/// Each light fixture's box in page fractions (x, y, w, h).
pub fn light_boxes() -> Vec<[f64; 4]> {
	LIGHTS.iter().map(|&(x, y)| frac(x - 10.0, y - 10.0, 20.0, 20.0)).collect()
}

fn frac(x: f64, y: f64, w: f64, h: f64) -> [f64; 4] {
	let r = |v: f64| (v * 1e6).round() / 1e6;
	[r(x / W), r(y / H), r(w / W), r(h / H)]
}

/// A content stream in top-down points.
struct Sheet(String);

impl Sheet {
	fn new() -> Self {
		Sheet("0 0 0 RG 0 0 0 rg 1 w\n".into())
	}
	fn width(&mut self, w: f64) {
		self.0.push_str(&format!("{w} w\n"));
	}
	fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
		self.0.push_str(&format!("{:.2} {:.2} m {:.2} {:.2} l S\n", a.0, H - a.1, b.0, H - b.1));
	}
	fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
		self.0.push_str(&format!("{x:.2} {:.2} {w:.2} {h:.2} re S\n", H - y - h));
	}
	fn circle(&mut self, cx: f64, cy: f64, r: f64) {
		let k = 0.5523 * r;
		let cy = H - cy;
		self.0.push_str(&format!(
			"{:.2} {cy:.2} m {:.2} {:.2} {:.2} {:.2} {cx:.2} {:.2} c {:.2} {:.2} {:.2} {:.2} {:.2} {cy:.2} c {:.2} {:.2} {:.2} {:.2} {cx:.2} {:.2} c {:.2} {:.2} {:.2} {:.2} {:.2} {cy:.2} c S\n",
			cx + r, cx + r, cy + k, cx + k, cy + r, cy + r,
			cx - k, cy + r, cx - r, cy + k, cx - r,
			cx - r, cy - k, cx - k, cy - r, cy - r,
			cx + k, cy - r, cx + r, cy - k, cx + r,
		));
	}
	/// Text with its baseline starting at (x, y).
	fn text(&mut self, x: f64, y: f64, size: f64, s: &str) {
		let s = s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
		self.0.push_str(&format!("BT /F1 {size} Tf {x:.2} {:.2} Td ({s}) Tj ET\n", H - y));
	}
	fn receptacle(&mut self, cx: f64, cy: f64) {
		self.circle(cx, cy, 7.0);
		self.line((cx - 3.0, cy - 11.0), (cx - 3.0, cy + 11.0));
		self.line((cx + 3.0, cy - 11.0), (cx + 3.0, cy + 11.0));
	}
	fn light(&mut self, cx: f64, cy: f64) {
		self.rect(cx - 10.0, cy - 10.0, 20.0, 20.0);
		self.line((cx - 10.0, cy - 10.0), (cx + 10.0, cy + 10.0));
		self.line((cx - 10.0, cy + 10.0), (cx + 10.0, cy - 10.0));
	}
	/// Border and title block, the same on every sheet.
	fn frame(&mut self, title: &str, sheet: &str, scale: &str) {
		self.width(2.0);
		self.rect(18.0, 18.0, W - 36.0, H - 36.0);
		self.width(1.0);
		self.rect(1000.0, 640.0, 206.0, 134.0);
		self.line((1000.0, 676.0), (1206.0, 676.0));
		self.line((1000.0, 738.0), (1206.0, 738.0));
		self.text(1010.0, 664.0, 12.0, "EVAL PROJECT");
		self.text(1010.0, 700.0, 11.0, title);
		self.text(1010.0, 724.0, 9.0, scale);
		self.text(1010.0, 762.0, 16.0, sheet);
	}
	/// A dimension string from a to b (horizontal or vertical) with its text.
	fn dimension(&mut self, a: (f64, f64), b: (f64, f64), label: &str) {
		self.width(0.5);
		self.line(a, b);
		for p in [a, b] {
			self.line((p.0 - 4.0, p.1 + 4.0), (p.0 + 4.0, p.1 - 4.0));
		}
		self.width(1.0);
		if a.1 == b.1 {
			self.text((a.0 + b.0) / 2.0 - 14.0, a.1 - 6.0, 10.0, label);
		} else {
			self.text(a.0 - 40.0, (a.1 + b.1) / 2.0 + 4.0, 10.0, label);
		}
	}
}

/// One page, Helvetica as /F1.
fn pdf(content: &str) -> Vec<u8> {
	let objects = [
		String::new(),
		"<< /Type /Catalog /Pages 2 0 R >>".to_string(),
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
		format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"),
		format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
		"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
	];
	let mut out = String::from("%PDF-1.4\n");
	let mut offsets = vec![0usize; objects.len()];
	for (n, o) in objects.iter().enumerate().skip(1) {
		offsets[n] = out.len();
		out.push_str(&format!("{n} 0 obj\n{o}\nendobj\n"));
	}
	let xref = out.len();
	out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len()));
	for off in offsets.iter().skip(1) {
		out.push_str(&format!("{off:010} 00000 n \n"));
	}
	out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len()));
	out.into_bytes()
}

pub fn symbols_pdf() -> Vec<u8> {
	let mut s = Sheet::new();
	s.frame("LIGHTING & POWER PLAN", "E-101", "SCALE: 1/8\" = 1'-0\"");
	// Legend: one of each symbol, not part of the count.
	s.rect(1000.0, 36.0, 206.0, 120.0);
	s.text(1010.0, 56.0, 11.0, "LEGEND");
	s.receptacle(1022.0, 88.0);
	s.text(1042.0, 92.0, 9.0, "DUPLEX RECEPTACLE");
	s.light(1022.0, 128.0);
	s.text(1042.0, 132.0, 9.0, "LIGHT FIXTURE");
	// Plan: three rooms.
	s.width(2.0);
	s.rect(80.0, 80.0, 840.0, 600.0);
	s.line((500.0, 80.0), (500.0, 360.0));
	s.line((500.0, 410.0), (500.0, 680.0));
	s.line((500.0, 380.0), (920.0, 380.0));
	s.width(1.0);
	s.text(250.0, 390.0, 12.0, "OPEN OFFICE");
	s.text(680.0, 300.0, 12.0, "CONFERENCE");
	s.text(690.0, 600.0, 12.0, "BREAK ROOM");
	for (x, y) in RECEPTACLES {
		s.receptacle(x, y);
	}
	for (x, y) in LIGHTS {
		s.light(x, y);
	}
	pdf(&s.0)
}

pub fn rooms_pdf() -> Vec<u8> {
	let mut s = Sheet::new();
	s.frame("FIRST FLOOR PLAN", "A-101", "SCALE: 1/4\" = 1'-0\"");
	// 18 pt = 1 ft. OFFICE 101: x 200..560, STORAGE 102: x 560..776, y 200..470.
	s.line((200.0, 200.0), (776.0, 200.0));
	s.line((200.0, 470.0), (776.0, 470.0));
	s.line((200.0, 200.0), (200.0, 470.0));
	s.line((776.0, 200.0), (776.0, 470.0));
	s.line((560.0, 200.0), (560.0, 310.0));
	s.line((560.0, 364.0), (560.0, 470.0));
	s.line((560.0, 364.0), (614.0, 364.0));
	s.dimension((200.0, 160.0), (560.0, 160.0), "20'-0\"");
	s.dimension((560.0, 160.0), (776.0, 160.0), "12'-0\"");
	s.dimension((160.0, 200.0), (160.0, 470.0), "15'-0\"");
	s.text(345.0, 330.0, 12.0, "OFFICE");
	s.text(365.0, 346.0, 12.0, "101");
	s.text(635.0, 330.0, 12.0, "STORAGE");
	s.text(655.0, 346.0, 12.0, "102");
	pdf(&s.0)
}

/// Every synthetic drawing by file name.
pub fn drawings() -> Vec<(&'static str, Vec<u8>)> {
	vec![("symbols.pdf", symbols_pdf()), ("rooms.pdf", rooms_pdf())]
}

/// Write them into `dir` (each replaced whole, so a reader never sees half a
/// file). Returns the paths.
pub fn write_all(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
	std::fs::create_dir_all(dir)?;
	let mut out = Vec::new();
	for (name, bytes) in drawings() {
		let p = dir.join(name);
		if std::fs::read(&p).ok().as_deref() != Some(&bytes[..]) {
			let tmp = dir.join(format!(".{name}.{}", std::process::id()));
			std::fs::write(&tmp, bytes)?;
			std::fs::rename(&tmp, &p)?;
		}
		out.push(p);
	}
	Ok(out)
}
