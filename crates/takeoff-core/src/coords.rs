//! Vision coordinate dialects: how a given vision model is asked to write a
//! location on an image, and how its answer becomes fractions of that image.
//! Port of construction.live `visionCoords.ts`, plus the Qwen3-VL dialect.
//!
//! - Gemini: `box_2d [ymin, xmin, ymax, xmax]` on a 0–1000 grid ([y, x] order).
//! - Qwen3-VL family: `bbox_2d [x1, y1, x2, y2]` on a 0–1000 grid ([x, y] order).
//! - Claude, GPT, Qwen2.5-VL and unknown models: absolute pixels of the image
//!   as the model sees it, so we size the image so the model never resizes it.
//!
//! All conversion maths stays in code; the model only ever reads.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
	/// 0–1000, pairs as [y, x] (Gemini).
	Grid1000Yx,
	/// 0–1000, pairs as [x, y] (Qwen3-VL and later).
	Grid1000Xy,
	/// Absolute pixels of the image the model saw, [x, y].
	Pixel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
	Gemini,
	Anthropic,
	OpenAi,
	Qwen3,
	Qwen25,
	Other,
}

pub fn family_of(model_id: &str) -> Family {
	let id = model_id.to_lowercase();
	if id.is_empty() {
		return Family::Other;
	}
	if id.contains("gemini") || id.starts_with("google/") && !id.contains("gemma") {
		return Family::Gemini;
	}
	if ["claude", "sonnet", "opus", "haiku", "anthropic/"].iter().any(|k| id.contains(k)) {
		return Family::Anthropic;
	}
	if id.contains("gpt-") || id.starts_with("openai/") || id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") {
		return Family::OpenAi;
	}
	if id.contains("qwen2.5") || id.contains("qwen2_5") {
		return Family::Qwen25;
	}
	if id.contains("qwen3") || id.contains("qwen-3") {
		return Family::Qwen3;
	}
	Family::Other
}

/// Which convention to ask this model for.
pub fn dialect_for(model_id: &str) -> Dialect {
	match family_of(model_id) {
		Family::Gemini => Dialect::Grid1000Yx,
		Family::Qwen3 => Dialect::Grid1000Xy,
		_ => Dialect::Pixel,
	}
}

impl Dialect {
	pub fn is_grid(self) -> bool {
		self != Dialect::Pixel
	}

	fn frame(self, w: f64, h: f64) -> (f64, f64) {
		if self.is_grid() {
			(1000.0, 1000.0)
		} else {
			(w, h)
		}
	}

	/// Sentence telling the model how to write a box. `w`/`h`: the size it will see.
	pub fn box_request(self, w: u32, h: u32) -> String {
		match self {
			Dialect::Grid1000Yx => {
				"Express each location as {\"box_2d\": [ymin, xmin, ymax, xmax]} with box_2d on the 0-1000 scale of the image.".into()
			}
			Dialect::Grid1000Xy => {
				"Express each location as {\"bbox_2d\": [x1, y1, x2, y2]} on the 0-1000 scale of the image (x1,y1 = top-left, x2,y2 = bottom-right).".into()
			}
			Dialect::Pixel => format!(
				"The image is {w}x{h} pixels. Express each location as {{\"box\": [x1, y1, x2, y2]}} in absolute pixel coordinates of this image (origin top-left, x right, y down; x1,y1 = top-left corner, x2,y2 = bottom-right corner)."
			),
		}
	}

	pub fn point_request(self, w: u32, h: u32) -> String {
		match self {
			Dialect::Grid1000Yx | Dialect::Grid1000Xy => {
				"Express each point as {\"x\": <0-1000>, \"y\": <0-1000>} on the 0-1000 scale of the image.".into()
			}
			Dialect::Pixel => format!(
				"The image is {w}x{h} pixels. Express each point as {{\"x\": <px>, \"y\": <px>}} in absolute pixel coordinates of this image (origin top-left)."
			),
		}
	}

	/// The caption line for an image injected into the agent's context.
	pub fn reading_rule(self, w: u32, h: u32) -> String {
		match self {
			Dialect::Grid1000Yx => "Locations you read from this image are box_2d [y1,x1,y2,x2] on ITS 0-1000 grid".into(),
			Dialect::Grid1000Xy => "Locations you read from this image are bbox_2d [x1,y1,x2,y2] on ITS 0-1000 grid".into(),
			Dialect::Pixel => format!(
				"This image is {w}x{h} pixels. Locations you read from it are [x1,y1,x2,y2] in ITS absolute pixel coordinates (origin top-left)"
			),
		}
	}

	/// How a coordinate pair is written.
	pub fn pair_spec(self) -> &'static str {
		if self == Dialect::Grid1000Yx {
			"[y, x]"
		} else {
			"[x, y]"
		}
	}

	pub fn pair_to_point(self, pair: &Value) -> Option<(f64, f64)> {
		let arr = pair.as_array()?;
		let [a, b] = arr.as_slice() else { return None };
		let (a, b) = (num(a)?, num(b)?);
		Some(if self == Dialect::Grid1000Yx { (b, a) } else { (a, b) })
	}

	/// Raw box in x/y order → fractions (x, y, w, h) of the image it was read from.
	pub fn box_to_fractions(self, b: RawBox, img_w: f64, img_h: f64) -> (f64, f64, f64, f64) {
		let (sx, sy) = self.frame(img_w, img_h);
		(b.x1 / sx, b.y1 / sy, (b.x2 - b.x1) / sx, (b.y2 - b.y1) / sy)
	}

	pub fn point_to_fractions(self, x: f64, y: f64, img_w: f64, img_h: f64) -> (f64, f64) {
		let (sx, sy) = self.frame(img_w, img_h);
		(x / sx, y / sy)
	}

	/// Are both corners inside the image (1% tolerance for edge overshoot)?
	pub fn box_in_bounds(self, b: RawBox, img_w: f64, img_h: f64) -> bool {
		self.point_in_bounds(b.x1, b.y1, img_w, img_h) && self.point_in_bounds(b.x2, b.y2, img_w, img_h)
	}

	pub fn point_in_bounds(self, x: f64, y: f64, img_w: f64, img_h: f64) -> bool {
		let (sx, sy) = self.frame(img_w, img_h);
		let tol = 0.01;
		x >= -sx * tol && x <= sx * (1.0 + tol) && y >= -sy * tol && y <= sy * (1.0 + tol)
	}

	/// Parse a model reply into boxes in this dialect's units, x/y order.
	/// Any of `box_2d`, `bbox_2d`, `box`, `bbox` is accepted: the dialect
	/// decides the ORDER, because a model told one name sometimes uses another.
	pub fn parse_boxes(self, raw: &str) -> Vec<RawBox> {
		let mut out = Vec::new();
		for entry in extract_array(raw) {
			let Some(obj) = entry.as_object() else { continue };
			let arr = ["box_2d", "bbox_2d", "box", "bbox"].iter().find_map(|k| obj.get(*k).and_then(Value::as_array));
			let Some(arr) = arr else { continue };
			let nums: Option<Vec<f64>> = arr.iter().map(num).collect();
			let Some([a, b, c, d]) = nums.as_deref().and_then(|n| <[f64; 4]>::try_from(n).ok()) else { continue };
			let mut bx = if self == Dialect::Grid1000Yx {
				RawBox { x1: b, y1: a, x2: d, y2: c, label: None }
			} else {
				RawBox { x1: a, y1: b, x2: c, y2: d, label: None }
			};
			bx.label = obj
				.get("label")
				.or_else(|| obj.get("text"))
				.and_then(Value::as_str)
				.filter(|s| !s.is_empty())
				.map(str::to_string);
			out.push(bx);
		}
		out
	}
}

/// A box in the dialect's units, already in x/y order.
#[derive(Debug, Clone, PartialEq)]
pub struct RawBox {
	pub x1: f64,
	pub y1: f64,
	pub x2: f64,
	pub y2: f64,
	pub label: Option<String>,
}

fn num(v: &Value) -> Option<f64> {
	match v {
		Value::Number(n) => n.as_f64(),
		Value::String(s) => s.trim().parse().ok(),
		_ => None,
	}
	.filter(|n: &f64| n.is_finite())
}

/// First JSON array in a reply, tolerating code fences and prose around it.
pub fn extract_array(raw: &str) -> Vec<Value> {
	let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) else { return vec![] };
	if end <= start {
		return vec![];
	}
	match serde_json::from_str::<Value>(&raw[start..=end]) {
		Ok(Value::Array(items)) => items,
		_ => vec![],
	}
}

/// Parse `{"x":..,"y":..}` points.
pub fn parse_points(raw: &str) -> Vec<(f64, f64)> {
	extract_array(raw)
		.iter()
		.filter_map(|e| {
			let o = e.as_object()?;
			Some((num(o.get("x")?)?, num(o.get("y")?)?))
		})
		.collect()
}

/// Map fractions of a rendered region back to fractions of the full page.
pub fn region_to_page(region: (f64, f64, f64, f64), f: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
	(region.0 + f.0 * region.2, region.1 + f.1 * region.3, f.2 * region.2, f.3 * region.3)
}

// ── Image sizing ──────────────────────────────────────────────────────────

const CLAUDE_MAX_EDGE: u32 = 1568;
const CLAUDE_MAX_TOKENS: u64 = 1568;
const CLAUDE_PATCH: u32 = 28;
const OPENAI_MAX_EDGE: u32 = 6000;
const OPENAI_MAX_PIXELS: f64 = 10_240_000.0;
/// Qwen-VL vision towers work on 28 px patches (14 px × 2 merge) — llama.cpp
/// resizes to multiples of it, so we send exact multiples.
const QWEN_PATCH: u32 = 28;
/// Keep local vision prompts affordable: ~1.6 MP is ~2000 image tokens on Qwen.
const LOCAL_MAX_PIXELS: f64 = 1_600_000.0;

fn round_half_even(v: f64) -> f64 {
	let f = v.floor();
	if v - f != 0.5 {
		return v.round();
	}
	if f % 2.0 == 0.0 {
		f
	} else {
		f + 1.0
	}
}

/// Anthropic's reference resize: the largest aspect-preserving size that fits.
pub fn claude_resized_size(w: u32, h: u32) -> (u32, u32) {
	let fits = |w: u32, h: u32| {
		w.div_ceil(CLAUDE_PATCH) * CLAUDE_PATCH <= CLAUDE_MAX_EDGE
			&& h.div_ceil(CLAUDE_PATCH) * CLAUDE_PATCH <= CLAUDE_MAX_EDGE
			&& (w.div_ceil(CLAUDE_PATCH) as u64) * (h.div_ceil(CLAUDE_PATCH) as u64) <= CLAUDE_MAX_TOKENS
	};
	if fits(w, h) {
		return (w, h);
	}
	if h > w {
		let (a, b) = claude_resized_size(h, w);
		return (b, a);
	}
	let aspect = w as f64 / h as f64;
	let (mut lo, mut hi) = (1u32, w);
	while lo + 1 < hi {
		let mid = (lo + hi) / 2;
		if fits(mid, (round_half_even(mid as f64 / aspect) as u32).max(1)) {
			lo = mid;
		} else {
			hi = mid;
		}
	}
	(lo, (round_half_even(lo as f64 / aspect) as u32).max(1))
}

fn fit_within(w: u32, h: u32, max_edge: u32, max_pixels: f64) -> (u32, u32) {
	let mut scale: f64 = 1.0;
	let edge = w.max(h);
	if edge > max_edge {
		scale = scale.min(max_edge as f64 / edge as f64);
	}
	let px = w as f64 * h as f64;
	if px > max_pixels {
		scale = scale.min((max_pixels / px).sqrt());
	}
	if scale >= 1.0 {
		return (w, h);
	}
	(((w as f64 * scale).floor() as u32).max(1), ((h as f64 * scale).floor() as u32).max(1))
}

/// The pixel size the model will look at for a (w, h) image. Callers render
/// to this size BEFORE sending, so pixel answers map 1:1 onto our image.
pub fn image_fit(model_id: &str, w: u32, h: u32) -> (u32, u32) {
	match family_of(model_id) {
		Family::Gemini => (w, h),
		Family::OpenAi => fit_within(w, h, OPENAI_MAX_EDGE, OPENAI_MAX_PIXELS),
		Family::Qwen3 | Family::Qwen25 => {
			let (fw, fh) = fit_within(w, h, u32::MAX, LOCAL_MAX_PIXELS);
			((fw / QWEN_PATCH).max(1) * QWEN_PATCH, (fh / QWEN_PATCH).max(1) * QWEN_PATCH)
		}
		Family::Anthropic | Family::Other => claude_resized_size(w, h),
	}
}

/// Largest long edge (≤ preferred) at which a crop of ANY aspect is sent unresized.
pub fn safe_max_edge(model_id: &str, preferred: u32) -> u32 {
	match family_of(model_id) {
		Family::Gemini => preferred,
		Family::OpenAi => preferred.min(OPENAI_MAX_EDGE),
		Family::Qwen3 | Family::Qwen25 => preferred.min(((LOCAL_MAX_PIXELS.sqrt() as u32) / QWEN_PATCH) * QWEN_PATCH),
		Family::Anthropic | Family::Other => preferred.min(((CLAUDE_MAX_TOKENS as f64).sqrt() as u32) * CLAUDE_PATCH),
	}
}

/// OpenAI models are asked for `detail: "original"` so they see our size.
pub fn wants_original_detail(model_id: &str) -> bool {
	family_of(model_id) == Family::OpenAi
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn families() {
		assert_eq!(dialect_for("google/gemini-3.6-flash"), Dialect::Grid1000Yx);
		assert_eq!(dialect_for("anthropic/claude-opus-5.5"), Dialect::Pixel);
		assert_eq!(dialect_for("Qwen3.8-27B-Q4_K_M"), Dialect::Grid1000Xy);
		assert_eq!(dialect_for("qwen2.5-vl-7b"), Dialect::Pixel);
		assert_eq!(dialect_for("google/gemma-4-27b"), Dialect::Pixel);
	}

	#[test]
	fn parse_orders() {
		let raw = "```json\n[{\"box_2d\": [100, 200, 300, 400], \"label\": \"a\"}]\n```";
		let g = Dialect::Grid1000Yx.parse_boxes(raw);
		assert_eq!((g[0].x1, g[0].y1, g[0].x2, g[0].y2), (200.0, 100.0, 400.0, 300.0));
		assert_eq!(g[0].label.as_deref(), Some("a"));
		let q = Dialect::Grid1000Xy.parse_boxes("[{\"bbox_2d\": [100, 200, 300, 400]}]");
		assert_eq!((q[0].x1, q[0].y1), (100.0, 200.0));
		let (x, y, w, h) = Dialect::Grid1000Xy.box_to_fractions(q[0].clone(), 0.0, 0.0);
		assert_eq!((x, y, w, h), (0.1, 0.2, 0.2, 0.2));
		let p = Dialect::Pixel.parse_boxes("[{\"box\": [10, 20, 30, 40]}]");
		let (x, y, _, _) = Dialect::Pixel.box_to_fractions(p[0].clone(), 100.0, 200.0);
		assert_eq!((x, y), (0.1, 0.1));
	}

	#[test]
	fn claude_fit_matches_platform() {
		// Square crops at 1092 fit exactly (39×39 patches = 1521 tokens).
		assert_eq!(claude_resized_size(1092, 1092), (1092, 1092));
		let (w, h) = claude_resized_size(3400, 2200);
		assert!(w <= 1568 && (w.div_ceil(28) as u64) * (h.div_ceil(28) as u64) <= 1568);
		assert_eq!(safe_max_edge("claude", 2000), 1092);
	}

	#[test]
	fn qwen_fit_is_patch_aligned() {
		let (w, h) = image_fit("qwen3.8-27b", 3400, 2200);
		assert_eq!((w % 28, h % 28), (0, 0));
		assert!((w as f64 * h as f64) <= 1_600_000.0);
	}
}
