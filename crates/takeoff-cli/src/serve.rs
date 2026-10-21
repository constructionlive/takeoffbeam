//! `takeoff serve`: one drawing kept open for an app that shows it (HaloLive's
//! Takeoff Bench), so a page or a tile of one renders without opening the PDF
//! again. JSON, one message per line, over stdio:
//!
//!   → {"id":1,"op":"pages"}
//!   ← {"id":1,"ok":true,"pages":[{"page":1,"label":"A1.01 FLOOR PLAN","width":36.0,"height":24.0,"unit":"in"}]}
//!   → {"id":2,"op":"render","page":3,"dpi":300,"region":[0.5,0,0.25,0.25],"out":"/tmp/t.png"}
//!   ← {"id":2,"ok":true,"out":"/tmp/t.png","width":2700,"height":1800}
//!
//! The first line out is `{"ready":true,"pages":N,"version":"…"}`. If the PDF
//! changes on disk (pages rotated, deleted) it is opened again. Ends with stdin.

use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageEncoder};
use serde_json::{json, Value};
use takeoff_pdf::{Drawing, FracRect};

/// The longest edge a render may have (a tile, or a whole page at low DPI).
const MAX_PX: u32 = 8192;

struct Open {
	drawing: Drawing,
	at: Option<SystemTime>,
}

fn modified(p: &Path) -> Option<SystemTime> {
	std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Open {
	fn new(pdf: &Path) -> Result<Self, String> {
		Ok(Self { drawing: Drawing::open(pdf).map_err(|e| e.to_string())?, at: modified(pdf) })
	}

	/// The drawing as it is on disk now.
	fn fresh(&mut self, pdf: &Path) -> Result<&Drawing, String> {
		let now = modified(pdf);
		if now != self.at {
			*self = Self::new(pdf)?;
		}
		Ok(&self.drawing)
	}
}

fn pages(d: &Drawing) -> Result<Value, String> {
	let out: Vec<Value> = (1..=d.page_count())
		.map(|p| {
			let (w, h) = d.page_size_pt(p).map_err(|e| e.to_string())?;
			let label = d.page_label(p).ok().flatten().unwrap_or_default();
			Ok(json!({ "page": p, "label": label, "width": w as f64 / 72.0, "height": h as f64 / 72.0, "unit": "in" }))
		})
		.collect::<Result<_, String>>()?;
	Ok(json!({ "pages": out }))
}

fn region(v: Option<&Value>) -> Result<FracRect, String> {
	let Some(v) = v else { return Ok(FracRect::FULL) };
	let n: Vec<f64> = v.as_array().ok_or("region is [x, y, w, h]")?.iter().filter_map(Value::as_f64).collect();
	let [x, y, w, h] = n[..] else { return Err("region is [x, y, w, h], fractions of the page".into()) };
	Ok(FracRect { x, y, w, h })
}

/// Line work on white, saved quickly: an app asks for many of these while you zoom.
fn save_png(img: image::RgbaImage, out: &Path) -> Result<(u32, u32), String> {
	let rgb = DynamicImage::ImageRgba8(img).into_rgb8();
	let tmp = out.with_extension("part");
	let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
	PngEncoder::new_with_quality(BufWriter::new(file), CompressionType::Fast, FilterType::Sub)
		.write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
		.map_err(|e| e.to_string())?;
	// Whoever reads `out` never sees half a file.
	std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
	Ok(rgb.dimensions())
}

fn render(d: &Drawing, req: &Value) -> Result<Value, String> {
	let page = req.get("page").and_then(Value::as_u64).unwrap_or(1) as u32;
	let dpi = req.get("dpi").and_then(Value::as_f64).unwrap_or(150.0).clamp(10.0, 1200.0) as f32;
	let out = PathBuf::from(req.get("out").and_then(Value::as_str).ok_or("say where to save it: out")?);
	let img = d.render(page, dpi, region(req.get("region"))?, Some(MAX_PX)).map_err(|e| e.to_string())?;
	let (w, h) = save_png(img.image, &out)?;
	Ok(json!({ "out": out, "width": w, "height": h, "dpi": img.dpi }))
}

pub fn serve(pdf: PathBuf) -> Result<(), String> {
	let mut open = Open::new(&pdf)?;
	let stdout = std::io::stdout();
	let tell = |v: Value| -> bool {
		let mut o = stdout.lock();
		writeln!(o, "{v}").and_then(|_| o.flush()).is_ok()
	};
	if !tell(json!({ "ready": true, "pages": open.drawing.page_count(), "version": env!("CARGO_PKG_VERSION") })) {
		return Ok(());
	}
	for line in std::io::stdin().lock().lines() {
		let Ok(line) = line else { break };
		if line.trim().is_empty() {
			continue;
		}
		let req: Value = match serde_json::from_str(&line) {
			Ok(v) => v,
			Err(e) => {
				tell(json!({ "ok": false, "error": format!("not JSON: {e}") }));
				continue;
			}
		};
		let id = req.get("id").cloned().unwrap_or(Value::Null);
		let res = open.fresh(&pdf).and_then(|d| match req.get("op").and_then(Value::as_str) {
			Some("pages") => pages(d),
			Some("render") => render(d, &req),
			other => Err(format!("no op {} (there are pages and render)", other.unwrap_or("given"))),
		});
		let mut msg = match res {
			Ok(Value::Object(m)) => Value::Object(m),
			Ok(v) => json!({ "result": v }),
			Err(e) => json!({ "ok": false, "error": e }),
		};
		msg["id"] = id;
		if msg.get("ok").is_none() {
			msg["ok"] = json!(true);
		}
		if !tell(msg) {
			break;
		}
	}
	Ok(())
}
