//! Takeoff Desk launch film. Renders 1920×1080 @ 30 fps frame by frame
//! (type, motion graphics, real UI footage recorded by
//! `crates/takeoff-app/tests/footage.rs`), synthesizes the score, and pipes it
//! all to ffmpeg.
//!
//! cargo run --release -- <repo root> <out.mp4>

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::*;

const W: u32 = 1920;
const H: u32 = 1080;
const FPS: f32 = 30.0;

const ORANGE: (u8, u8, u8) = (0xFF, 0x6B, 0x35);
const ORANGE2: (u8, u8, u8) = (0xFF, 0xA3, 0x5C);
const WHITE: (u8, u8, u8) = (0xF5, 0xF5, 0xF7);
const GRAY: (u8, u8, u8) = (0x86, 0x86, 0x8B);

// ── Easing ──────────────────────────────────────────────────────────────

fn clamp01(t: f32) -> f32 {
	t.clamp(0.0, 1.0)
}
fn out_cubic(t: f32) -> f32 {
	1.0 - (1.0 - clamp01(t)).powi(3)
}
fn in_out(t: f32) -> f32 {
	let t = clamp01(t);
	if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}
fn out_expo(t: f32) -> f32 {
	let t = clamp01(t);
	if t >= 1.0 { 1.0 } else { 1.0 - 2f32.powf(-10.0 * t) }
}
/// 0→1 over [a, a+d].
fn ramp(t: f32, a: f32, d: f32) -> f32 {
	clamp01((t - a) / d)
}
/// Fade in at `a`, out ending at `b`.
fn window(t: f32, a: f32, b: f32, fade: f32) -> f32 {
	out_cubic(ramp(t, a, fade)) * (1.0 - in_out(ramp(t, b - fade, fade)))
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a + (b - a) * t
}

// ── Drawing ─────────────────────────────────────────────────────────────

struct Fonts {
	bold: FontRef<'static>,
	semi: FontRef<'static>,
	regular: FontRef<'static>,
}

#[derive(Clone, Copy)]
enum Weight {
	Bold,
	Semi,
	Regular,
}

struct Ctx<'a> {
	pix: Pixmap,
	fonts: &'a Fonts,
}

fn color(c: (u8, u8, u8), a: f32) -> Color {
	Color::from_rgba8(c.0, c.1, c.2, (clamp01(a) * 255.0) as u8)
}

fn solid(c: (u8, u8, u8), a: f32) -> Paint<'static> {
	let mut p = Paint::default();
	p.set_color(color(c, a));
	p.anti_alias = true;
	p
}

impl Ctx<'_> {
	fn clear(&mut self) {
		self.pix.fill(Color::from_rgba8(0, 0, 0, 255));
	}

	fn font(&self, w: Weight) -> &FontRef<'static> {
		match w {
			Weight::Bold => &self.fonts.bold,
			Weight::Semi => &self.fonts.semi,
			Weight::Regular => &self.fonts.regular,
		}
	}

	fn text_width(&self, text: &str, w: Weight, size: f32, tracking: f32) -> f32 {
		let f = self.font(w).as_scaled(PxScale::from(size));
		let mut x = 0.0;
		let mut prev = None;
		for c in text.chars() {
			let id = f.glyph_id(c);
			if let Some(p) = prev {
				x += f.kern(p, id);
			}
			x += f.h_advance(id) + tracking * size;
			prev = Some(id);
		}
		x - tracking * size
	}

	/// Text centered on (cx, baseline), filled with a vertical gradient (top→bottom colors).
	#[allow(clippy::too_many_arguments)]
	fn text(&mut self, text: &str, w: Weight, size: f32, cx: f32, baseline: f32, top: (u8, u8, u8), bottom: (u8, u8, u8), alpha: f32, tracking: f32, align: f32) {
		if alpha <= 0.002 || text.is_empty() {
			return;
		}
		let width = self.text_width(text, w, size, tracking);
		let start = cx - width * align;
		let font = self.font(w).clone();
		let f = font.as_scaled(PxScale::from(size));
		let mut mask = Mask::new(W, H).unwrap();
		let data = mask.data_mut();
		let mut x = start;
		let mut prev = None;
		for c in text.chars() {
			let id = f.glyph_id(c);
			if let Some(p) = prev {
				x += f.kern(p, id);
			}
			let g = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(x, baseline));
			if let Some(o) = font.outline_glyph(g) {
				let b = o.px_bounds();
				o.draw(|gx, gy, cov| {
					let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
					if px >= 0 && py >= 0 && (px as u32) < W && (py as u32) < H {
						let i = (py as u32 * W + px as u32) as usize;
						data[i] = data[i].max((cov.min(1.0) * 255.0) as u8);
					}
				});
			}
			x += f.h_advance(id) + tracking * size;
			prev = Some(id);
		}
		let asc = f.ascent();
		let shader = LinearGradient::new(
			Point::from_xy(0.0, baseline - asc),
			Point::from_xy(0.0, baseline),
			vec![GradientStop::new(0.0, color(top, alpha)), GradientStop::new(1.0, color(bottom, alpha))],
			SpreadMode::Pad,
			Transform::identity(),
		)
		.unwrap_or(Shader::SolidColor(color(top, alpha)));
		let paint = Paint { shader, anti_alias: true, ..Default::default() };
		self.pix.fill_rect(Rect::from_xywh(0.0, 0.0, W as f32, H as f32).unwrap(), &paint, Transform::identity(), Some(&mask));
	}

	fn center_text(&mut self, text: &str, w: Weight, size: f32, baseline: f32, c: (u8, u8, u8), alpha: f32) {
		self.text(text, w, size, W as f32 / 2.0, baseline, c, c, alpha, -0.02, 0.5);
	}

	/// Soft radial glow.
	fn glow(&mut self, cx: f32, cy: f32, r: f32, c: (u8, u8, u8), a: f32) {
		if a <= 0.0 {
			return;
		}
		let shader = RadialGradient::new(
			Point::from_xy(cx, cy),
			Point::from_xy(cx, cy),
			r,
			vec![GradientStop::new(0.0, color(c, a)), GradientStop::new(0.45, color(c, a * 0.35)), GradientStop::new(1.0, color(c, 0.0))],
			SpreadMode::Pad,
			Transform::identity(),
		);
		if let Some(shader) = shader {
			let paint = Paint { shader, anti_alias: true, ..Default::default() };
			self.pix.fill_rect(Rect::from_xywh(cx - r, cy - r, r * 2.0, r * 2.0).unwrap(), &paint, Transform::identity(), None);
		}
	}

	fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
		let mut pb = PathBuilder::new();
		pb.move_to(x + r, y);
		pb.line_to(x + w - r, y);
		pb.quad_to(x + w, y, x + w, y + r);
		pb.line_to(x + w, y + h - r);
		pb.quad_to(x + w, y + h, x + w - r, y + h);
		pb.line_to(x + r, y + h);
		pb.quad_to(x, y + h, x, y + h - r);
		pb.line_to(x, y + r);
		pb.quad_to(x, y, x + r, y);
		pb.close();
		pb.finish()
	}

	/// An image in a rounded "window" with a soft shadow; (cx, cy) center, width w.
	fn window_image(&mut self, img: &Pixmap, cx: f32, cy: f32, w: f32, alpha: f32, radius: f32) {
		if alpha <= 0.002 {
			return;
		}
		let s = w / img.width() as f32;
		let h = img.height() as f32 * s;
		let (x, y) = (cx - w / 2.0, cy - h / 2.0);
		// Shadow: a few expanding translucent rounded rects.
		for k in 1..=6 {
			let g = k as f32 * 7.0;
			if let Some(p) = Self::rounded(x - g * 0.5, y - g * 0.2 + g * 0.6, w + g, h + g * 0.6, radius + g) {
				self.pix.fill_path(&p, &solid((0, 0, 0), 0.07 * alpha), FillRule::Winding, Transform::identity(), None);
			}
		}
		let Some(clip) = Self::rounded(x, y, w, h, radius) else { return };
		let mut mask = Mask::new(W, H).unwrap();
		mask.fill_path(&clip, FillRule::Winding, true, Transform::identity());
		let paint = PixmapPaint { opacity: alpha, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bicubic };
		self.pix.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_row(s, 0.0, 0.0, s, x, y), Some(&mask));
		// Hairline edge.
		let st = Stroke { width: 1.5, ..Default::default() };
		self.pix.stroke_path(&clip, &solid((255, 255, 255), 0.12 * alpha), &st, Transform::identity(), None);
	}

	/// Draw `img` with a crop/zoom ("camera"): the region (fx, fy, fw, fh) in
	/// image fractions fills the window.
	fn window_camera(&mut self, img: &Pixmap, cx: f32, cy: f32, w: f32, h: f32, cam: (f32, f32, f32, f32), alpha: f32, radius: f32) {
		if alpha <= 0.002 {
			return;
		}
		let (x, y) = (cx - w / 2.0, cy - h / 2.0);
		for k in 1..=6 {
			let g = k as f32 * 7.0;
			if let Some(p) = Self::rounded(x - g * 0.5, y + g * 0.4, w + g, h + g * 0.6, radius + g) {
				self.pix.fill_path(&p, &solid((0, 0, 0), 0.07 * alpha), FillRule::Winding, Transform::identity(), None);
			}
		}
		let Some(clip) = Self::rounded(x, y, w, h, radius) else { return };
		let mut mask = Mask::new(W, H).unwrap();
		mask.fill_path(&clip, FillRule::Winding, true, Transform::identity());
		let sx = w / (cam.2 * img.width() as f32);
		let sy = h / (cam.3 * img.height() as f32);
		let tx = x - cam.0 * img.width() as f32 * sx;
		let ty = y - cam.1 * img.height() as f32 * sy;
		let paint = PixmapPaint { opacity: alpha, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bicubic };
		self.pix.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_row(sx, 0.0, 0.0, sy, tx, ty), Some(&mask));
		let st = Stroke { width: 1.5, ..Default::default() };
		self.pix.stroke_path(&clip, &solid((255, 255, 255), 0.12 * alpha), &st, Transform::identity(), None);
	}

	fn image(&mut self, img: &Pixmap, cx: f32, cy: f32, size: f32, alpha: f32) {
		let s = size / img.width() as f32;
		let paint = PixmapPaint { opacity: alpha, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bicubic };
		self.pix.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_row(s, 0.0, 0.0, s, cx - size / 2.0, cy - img.height() as f32 * s / 2.0), None);
	}

	/// macOS-style arrow, or a crosshair for measuring tools.
	fn cursor(&mut self, x: f32, y: f32, kind: &str, scale: f32, alpha: f32) {
		if kind == "cross" {
			let mut pb = PathBuilder::new();
			let r = 13.0 * scale;
			pb.move_to(x - r, y);
			pb.line_to(x - 4.0 * scale, y);
			pb.move_to(x + 4.0 * scale, y);
			pb.line_to(x + r, y);
			pb.move_to(x, y - r);
			pb.line_to(x, y - 4.0 * scale);
			pb.move_to(x, y + 4.0 * scale);
			pb.line_to(x, y + r);
			let p = pb.finish().unwrap();
			self.pix.stroke_path(&p, &solid((255, 255, 255), alpha), &Stroke { width: 4.5 * scale, line_cap: LineCap::Round, ..Default::default() }, Transform::identity(), None);
			self.pix.stroke_path(&p, &solid((0, 0, 0), alpha), &Stroke { width: 2.0 * scale, line_cap: LineCap::Round, ..Default::default() }, Transform::identity(), None);
			return;
		}
		let s = scale * 1.25;
		let pts = [(0.0, 0.0), (0.0, 17.0), (4.2, 13.2), (7.2, 20.0), (10.0, 18.8), (7.1, 12.2), (12.6, 12.2)];
		let mut pb = PathBuilder::new();
		pb.move_to(x + pts[0].0 * s, y + pts[0].1 * s);
		for (px, py) in &pts[1..] {
			pb.line_to(x + px * s, y + py * s);
		}
		pb.close();
		let p = pb.finish().unwrap();
		self.pix.fill_path(&p, &solid((0, 0, 0), alpha * 0.25), FillRule::Winding, Transform::from_translate(1.5, 2.5), None);
		self.pix.fill_path(&p, &solid((0, 0, 0), alpha), FillRule::Winding, Transform::identity(), None);
		self.pix.stroke_path(&p, &solid((255, 255, 255), alpha), &Stroke { width: 1.6 * s, line_join: LineJoin::Round, ..Default::default() }, Transform::identity(), None);
	}

	fn ripple(&mut self, x: f32, y: f32, t: f32) {
		if !(0.0..=1.0).contains(&t) {
			return;
		}
		let r = lerp(6.0, 34.0, out_cubic(t));
		if let Some(p) = PathBuilder::from_circle(x, y, r) {
			self.pix.stroke_path(&p, &solid(ORANGE, (1.0 - t) * 0.9), &Stroke { width: 3.0, ..Default::default() }, Transform::identity(), None);
		}
	}

	fn lock_icon(&mut self, cx: f32, cy: f32, s: f32, a: f32) {
		if let Some(body) = Self::rounded(cx - 60.0 * s, cy - 10.0 * s, 120.0 * s, 95.0 * s, 16.0 * s) {
			let shader = LinearGradient::new(Point::from_xy(0.0, cy - 10.0 * s), Point::from_xy(0.0, cy + 85.0 * s), vec![GradientStop::new(0.0, color(ORANGE2, a)), GradientStop::new(1.0, color(ORANGE, a))], SpreadMode::Pad, Transform::identity()).unwrap();
			self.pix.fill_path(&body, &Paint { shader, anti_alias: true, ..Default::default() }, FillRule::Winding, Transform::identity(), None);
		}
		let mut pb = PathBuilder::new();
		pb.move_to(cx - 38.0 * s, cy - 10.0 * s);
		pb.line_to(cx - 38.0 * s, cy - 40.0 * s);
		pb.cubic_to(cx - 38.0 * s, cy - 92.0 * s, cx + 38.0 * s, cy - 92.0 * s, cx + 38.0 * s, cy - 40.0 * s);
		pb.line_to(cx + 38.0 * s, cy - 10.0 * s);
		if let Some(p) = pb.finish() {
			self.pix.stroke_path(&p, &solid(ORANGE2, a), &Stroke { width: 16.0 * s, line_cap: LineCap::Round, ..Default::default() }, Transform::identity(), None);
		}
		if let Some(hole) = PathBuilder::from_circle(cx, cy + 30.0 * s, 11.0 * s) {
			self.pix.fill_path(&hole, &solid((0, 0, 0), a * 0.85), FillRule::Winding, Transform::identity(), None);
		}
	}
}

// ── Footage ─────────────────────────────────────────────────────────────

struct Clip {
	dir: PathBuf,
	frames: usize,
	/// frame → (x, y, kind)
	cursor: Vec<(f32, f32, String)>,
	clicks: Vec<usize>,
}

impl Clip {
	fn load(root: &Path, name: &str) -> Clip {
		let dir = root.join("marketing/footage").join(name);
		let frames = std::fs::read_dir(&dir).map(|r| r.filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "png"))).count()).unwrap_or(0);
		let mut cursor = Vec::new();
		let mut clicks = Vec::new();
		if let Ok(txt) = std::fs::read_to_string(dir.join("cursor.txt")) {
			for line in txt.lines() {
				let parts: Vec<&str> = line.split_whitespace().collect();
				if parts.first() == Some(&"click") {
					if let Some(n) = parts.get(1).and_then(|n| n.parse().ok()) {
						clicks.push(n);
					}
				} else if parts.len() >= 4 {
					cursor.push((parts[1].parse().unwrap_or(0.0), parts[2].parse().unwrap_or(0.0), parts[3].to_string()));
				}
			}
		}
		Clip { dir, frames, cursor, clicks }
	}
	fn duration(&self) -> f32 {
		self.frames as f32 / FPS
	}
}

struct Footage {
	clips: HashMap<String, Clip>,
	cache: HashMap<(String, usize), Pixmap>,
}

impl Footage {
	fn frame(&mut self, name: &str, idx: usize) -> Option<Pixmap> {
		let clip = self.clips.get(name)?;
		if clip.frames == 0 {
			return None;
		}
		let idx = idx.min(clip.frames - 1);
		let key = (name.to_string(), idx);
		if !self.cache.contains_key(&key) {
			if self.cache.len() > 24 {
				self.cache.clear();
			}
			let img = image::open(clip.dir.join(format!("{idx:04}.png"))).ok()?.to_rgba8();
			let (w, h) = img.dimensions();
			let mut pm = Pixmap::new(w, h)?;
			// Opaque screenshots: straight == premultiplied.
			pm.data_mut().copy_from_slice(img.as_raw());
			self.cache.insert(key.clone(), pm);
		}
		self.cache.get(&key).cloned()
	}
}

/// Plays a clip inside a window, with its cursor and click ripples, starting at `start`.
#[allow(clippy::too_many_arguments)]
fn play(c: &mut Ctx, f: &mut Footage, name: &str, t: f32, start: f32, cx: f32, cy: f32, w: f32, cam: (f32, f32, f32, f32), alpha: f32) {
	let idx = ((t - start).max(0.0) * FPS) as usize;
	let Some(img) = f.frame(name, idx) else { return };
	let h = w * (cam.3 * img.height() as f32) / (cam.2 * img.width() as f32);
	c.window_camera(&img, cx, cy, w, h, cam, alpha, 18.0);
	let clip = &f.clips[name];
	let i = idx.min(clip.cursor.len().saturating_sub(1));
	if let Some((x, y, kind)) = clip.cursor.get(i).cloned() {
		// Image px → screen.
		let (iw, ih) = (img.width() as f32, img.height() as f32);
		let sx = w / (cam.2 * iw);
		let sy = h / (cam.3 * ih);
		let (ox, oy) = (cx - w / 2.0 - cam.0 * iw * sx, cy - h / 2.0 - cam.1 * ih * sy);
		let (px, py) = (ox + x * sx, oy + y * sy);
		for &k in &clip.clicks {
			let dt = (idx as f32 - k as f32) / 12.0;
			c.ripple(px, py, dt);
		}
		if px > cx - w / 2.0 && px < cx + w / 2.0 && py > cy - h / 2.0 && py < cy + h / 2.0 {
			c.cursor(px, py, &kind, (sx * 1.1).clamp(1.0, 2.2), alpha);
		}
	}
}

// ── The film ────────────────────────────────────────────────────────────

const LENGTH: f32 = 74.0;

struct Assets {
	icon: Pixmap,
}

/// Caption block: title + optional subtitle at the top of the frame.
fn caption(c: &mut Ctx, t: f32, a: f32, b: f32, title: &str, sub: &str) {
	let k = window(t, a, b, 0.6);
	let rise = (1.0 - out_cubic(ramp(t, a, 0.9))) * 30.0;
	c.center_text(title, Weight::Bold, 64.0, 128.0 + rise, WHITE, k);
	if !sub.is_empty() {
		let k2 = window(t, a + 0.35, b, 0.6);
		c.center_text(sub, Weight::Regular, 30.0, 180.0 + rise, GRAY, k2);
	}
}

fn render(c: &mut Ctx, f: &mut Footage, a: &Assets, t: f32) {
	c.clear();
	let cx = W as f32 / 2.0;
	let cy = H as f32 / 2.0;

	// 1 ── Opening (0–4.6)
	if t < 4.8 {
		let k = window(t, 0.25, 4.6, 0.8);
		let s = 1.0 + 0.04 * (1.0 - out_expo(ramp(t, 0.25, 2.0)));
		c.text("Takeoff.", Weight::Bold, 170.0 * s, cx, cy - 10.0, WHITE, (0xC8, 0xC8, 0xCD), k, -0.035, 0.5);
		let k2 = window(t, 1.6, 4.6, 0.8);
		c.glow(cx, cy + 120.0, 520.0, ORANGE, 0.10 * k2);
		c.text("Reimagined.", Weight::Bold, 170.0, cx, cy + 170.0, ORANGE2, ORANGE, k2, -0.035, 0.5);
	}

	// 2 ── Icon reveal (4.6–9)
	if (4.4..9.2).contains(&t) {
		let k = window(t, 4.7, 9.0, 0.7);
		let pop = out_expo(ramp(t, 4.7, 1.2));
		c.glow(cx, cy - 70.0, 420.0, ORANGE, 0.22 * k * (0.85 + 0.15 * (t * 2.0).sin()));
		c.image(&a.icon, cx, cy - 70.0, lerp(180.0, 260.0, pop), k);
		c.center_text("Takeoff Desk", Weight::Bold, 96.0, cy + 150.0 - (1.0 - out_cubic(ramp(t, 5.3, 1.0))) * 24.0, WHITE, window(t, 5.3, 9.0, 0.7));
		c.center_text("Quantity takeoff with an AI that can see your drawings.", Weight::Regular, 34.0, cy + 215.0, GRAY, window(t, 6.0, 9.0, 0.7));
	}

	// 3 ── Hero product (9–14.5)
	if (8.9..14.7).contains(&t) {
		let k = window(t, 9.0, 14.5, 0.7);
		let p = out_cubic(ramp(t, 9.0, 1.8));
		let drift = ramp(t, 9.0, 5.5);
		if let Some(img) = f.frame("area", 10_000) {
			let w = lerp(1300.0, 1560.0, p) + drift * 40.0;
			c.glow(cx, cy + 120.0, 900.0, ORANGE, 0.08 * k);
			c.window_image(&img, cx, cy + lerp(190.0, 125.0, p), w * 0.86, k, 16.0);
		}
		caption(c, t, 9.4, 14.5, "Pro-grade takeoff.", "Native on your Mac and PC. Nothing to subscribe to.");
	}

	// 4 ── Speed (14.5–22.5)
	if (14.3..22.7).contains(&t) {
		let k = window(t, 14.5, 22.5, 0.6);
		c.glow(cx, cy, 800.0, ORANGE, 0.07 * k);
		c.center_text("Built in Rust.", Weight::Bold, 78.0, 230.0, WHITE, k);
		c.text("Unreasonably fast.", Weight::Bold, 78.0, cx, 320.0, ORANGE2, ORANGE, window(t, 15.0, 22.5, 0.6), -0.03, 0.5);
		let stats: [(f32, &str, &str, &str); 3] = [(21.0, "ms", "to open an 859-sheet set", ""), (50.0, "ms", "to render a full sheet", "at 200 DPI"), (1.0, "ms", "to switch PDF layers", "<")];
		for (i, (v, unit, label, prefix)) in stats.iter().enumerate() {
			let start = 15.8 + i as f32 * 0.45;
			let kk = window(t, start, 22.5, 0.6);
			let x = cx + (i as f32 - 1.0) * 520.0;
			let count = (v * out_expo(ramp(t, start, 1.6))).round();
			let num = if *prefix == "<" { "<1".to_string() } else { format!("{count:.0}") };
			let rise = (1.0 - out_cubic(ramp(t, start, 0.9))) * 40.0;
			let wnum = c.text_width(&num, Weight::Bold, 150.0, -0.03);
			let wunit = c.text_width(unit, Weight::Semi, 54.0, 0.0);
			let left = x - (wnum + 14.0 + wunit) / 2.0;
			c.text(&num, Weight::Bold, 150.0, left, 600.0 + rise, WHITE, (0xBB, 0xBB, 0xC0), kk, -0.03, 0.0);
			c.text(unit, Weight::Semi, 54.0, left + wnum + 14.0, 600.0 + rise, ORANGE2, ORANGE, kk, 0.0, 0.0);
			c.text(label, Weight::Regular, 30.0, x, 660.0 + rise, GRAY, GRAY, kk, 0.0, 0.5);
			if !prefix.is_empty() && *prefix != "<" {
				c.text(prefix, Weight::Regular, 30.0, x, 700.0 + rise, GRAY, GRAY, kk, 0.0, 0.5);
			}
		}
		c.center_text("47 MB download  ·  110 MB of memory with a drawing open", Weight::Semi, 32.0, 850.0, (0xC8, 0xC8, 0xCD), window(t, 18.0, 22.5, 0.6));
	}

	// 5 ── Count (22.5–28.5)
	let count_len = f.clips.get("count").map(|c| c.duration()).unwrap_or(5.0);
	if (22.3..28.7).contains(&t) {
		let k = window(t, 22.5, 28.5, 0.5);
		let z = in_out(ramp(t, 22.5, 2.5));
		let cam = (lerp(0.0, 0.16, z), lerp(0.0, 0.1, z), lerp(1.0, 0.62, z), lerp(1.0, 0.62, z));
		play(c, f, "count", t, 22.9, cx, cy + 125.0, 1340.0, cam, k);
		caption(c, t, 22.6, 28.5, "Click. Count.", "Numbered as you go. Totals and cost update live.");
	}
	let _ = count_len;

	// 6 ── Area + snapping (28.5–35)
	if (28.3..35.2).contains(&t) {
		let k = window(t, 28.5, 35.0, 0.5);
		let z = in_out(ramp(t, 28.6, 1.5));
		let cam = (lerp(0.0, 0.18, z), lerp(0.0, 0.14, z), lerp(1.0, 0.5, z), lerp(1.0, 0.5, z));
		play(c, f, "area", t, 28.7, cx, cy + 125.0, 1340.0, cam, k);
		caption(c, t, 28.6, 35.0, "Snaps to the drawing's own lines.", "Areas, cutouts, volumes, perimeters, angles — measured live.");
	}

	// 7 ── Select / properties (35–38)
	if (34.8..38.2).contains(&t) {
		let k = window(t, 35.0, 38.0, 0.45);
		let cam = (0.5, 0.1, 0.5, 0.62);
		play(c, f, "select", t, 35.0, cx, cy + 125.0, 1180.0, cam, k);
		caption(c, t, 35.1, 38.0, "Every detail. Editable.", "Markups list, custom columns, CSV and flattened PDF export.");
	}

	// 8 ── AI (38–45)
	if (37.8..45.2).contains(&t) {
		let k = window(t, 38.0, 45.0, 0.6);
		let z = in_out(ramp(t, 39.5, 2.5));
		let cam = (lerp(0.0, 0.16, z), lerp(0.0, 0.08, z), lerp(1.0, 0.84, z), lerp(1.0, 0.84, z));
		c.glow(cx, cy + 80.0, 900.0, ORANGE, 0.10 * k);
		play(c, f, "ai", t, 38.0, cx, cy + 125.0, 1340.0, cam, k);
		caption(c, t, 38.1, 45.0, "Ask the drawing.", "It sets the scale, counts, measures — and checks its own work. You confirm.");
	}

	// 9 ── Private (45–50.5)
	if (44.8..50.7).contains(&t) {
		let k = window(t, 45.0, 50.5, 0.6);
		let pop = out_expo(ramp(t, 45.1, 1.0));
		c.glow(cx, 330.0, 360.0, ORANGE, 0.16 * k);
		c.lock_icon(cx, 340.0, lerp(0.8, 1.0, pop), k);
		c.center_text("Your drawings stay on your machine.", Weight::Bold, 66.0, 560.0, WHITE, window(t, 45.4, 50.5, 0.6));
		c.center_text("Local AI with llama.cpp. Memory checked before it loads.", Weight::Regular, 32.0, 620.0, GRAY, window(t, 45.9, 50.5, 0.6));
		c.center_text("Or sign in to construction.live for cloud models — your choice.", Weight::Regular, 32.0, 664.0, GRAY, window(t, 46.4, 50.5, 0.6));
		if let Some(img) = f.frame("models", 30) {
			let kk = window(t, 46.6, 50.5, 0.6);
			c.window_camera(&img, cx, 860.0, 760.0, 190.0, (0.3, 0.36, 0.4, 0.12), kk, 14.0);
		}
	}

	// 10 ── Secure (50.5–55.5)
	if (50.3..55.7).contains(&t) {
		let lines = [("Open source.", WHITE, 50.6), ("Apache 2.0. Audit every line.", WHITE, 51.2), ("The AI can touch the drawing —", ORANGE2, 52.2), ("and nothing else.", ORANGE2, 52.6)];
		for (i, (txt, col, at)) in lines.iter().enumerate() {
			let kk = window(t, *at, 55.5, 0.5);
			let rise = (1.0 - out_cubic(ramp(t, *at, 0.8))) * 26.0;
			c.center_text(txt, Weight::Bold, 70.0, 360.0 + i as f32 * 96.0 + rise, *col, kk);
		}
		c.center_text("No file access. No shell. No internet.", Weight::Regular, 34.0, 790.0, GRAY, window(t, 53.3, 55.5, 0.5));
	}

	// 11 ── PDF tools (55.5–62)
	if (55.3..62.2).contains(&t) {
		let k1 = window(t, 55.5, 58.6, 0.45);
		play(c, f, "layers", t, 55.3, cx, cy + 125.0, 1340.0, (0.15, 0.1, 0.62, 0.62), k1);
		caption(c, t, 55.6, 58.6, "Layers.", "Turn CAD layers off — for you and for the AI.");
		let k2 = window(t, 58.6, 62.0, 0.45);
		play(c, f, "compare", t, 58.0, cx, cy + 125.0, 1340.0, (0.17, 0.1, 0.62, 0.66), k2);
		caption(c, t, 58.7, 62.0, "Compare revisions.", "Split · Merge · Extract · Rotate · Reorder · Flatten");
	}

	// 12 ── Free (62–67.5)
	if (61.8..67.7).contains(&t) {
		let k = window(t, 62.0, 67.5, 0.6);
		let s = lerp(1.08, 1.0, out_expo(ramp(t, 62.0, 1.4)));
		c.glow(cx, cy - 40.0, 620.0, ORANGE, 0.14 * k);
		c.text("Free.", Weight::Bold, 260.0 * s, cx, cy + 40.0, WHITE, (0xC8, 0xC8, 0xCD), k, -0.04, 0.5);
		c.center_text("No subscription. No seat licenses. No catch.", Weight::Semi, 40.0, cy + 140.0, ORANGE2, window(t, 62.9, 67.5, 0.6));
	}

	// 13 ── End card (67.5–74)
	if t >= 67.3 {
		let k = window(t, 67.5, LENGTH + 1.0, 0.7) * (1.0 - in_out(ramp(t, LENGTH - 1.2, 1.2)));
		c.glow(cx, 330.0, 360.0, ORANGE, 0.18 * k);
		c.image(&a.icon, cx, 330.0, 200.0, k);
		c.center_text("Takeoff Desk", Weight::Bold, 92.0, 560.0, WHITE, k);
		c.center_text("Download for Mac and Windows", Weight::Semi, 38.0, 630.0, ORANGE2, window(t, 68.2, LENGTH + 1.0, 0.6) * (1.0 - in_out(ramp(t, LENGTH - 1.2, 1.2))));
		c.center_text("github.com/constructionlive/takeoffbeam", Weight::Regular, 32.0, 700.0, GRAY, window(t, 68.7, LENGTH + 1.0, 0.6) * (1.0 - in_out(ramp(t, LENGTH - 1.2, 1.2))));
		c.center_text("construction.live", Weight::Semi, 30.0, 960.0, (0xC8, 0xC8, 0xCD), window(t, 69.2, LENGTH + 1.0, 0.6) * (1.0 - in_out(ramp(t, LENGTH - 1.2, 1.2))));
	}
}

// ── Score ───────────────────────────────────────────────────────────────

/// A soft ambient pad (detuned sines through a gentle low-pass), a chord per
/// section, a low pulse in the speed section, and quiet clicks synced to the
/// cursor in the footage.
fn score(clips: &HashMap<String, Clip>, path: &Path) {
	let sr = 48_000usize;
	let n = (LENGTH * sr as f32) as usize;
	let mut l = vec![0f32; n];
	let mut r = vec![0f32; n];
	let midi = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
	// (start s, notes)
	let chords: [(f32, [f32; 5]); 7] = [
		(0.0, [48.0, 55.0, 62.0, 64.0, 67.0]),  // Cadd9
		(9.0, [45.0, 52.0, 60.0, 64.0, 67.0]),  // Am7
		(22.5, [41.0, 48.0, 57.0, 60.0, 64.0]), // Fmaj7
		(38.0, [43.0, 50.0, 59.0, 62.0, 66.0]), // Gmaj7-ish
		(50.5, [45.0, 52.0, 59.0, 60.0, 64.0]), // Am(add9)
		(62.0, [41.0, 48.0, 55.0, 60.0, 65.0]), // F sus
		(67.5, [48.0, 55.0, 62.0, 64.0, 67.0]), // C
	];
	for (ci, (start, notes)) in chords.iter().enumerate() {
		let end = chords.get(ci + 1).map(|c| c.0).unwrap_or(LENGTH);
		let (s0, s1) = (((start - 1.2).max(0.0) * sr as f32) as usize, ((end + 1.2).min(LENGTH) * sr as f32) as usize);
		for (k, note) in notes.iter().enumerate() {
			let f0 = midi(*note);
			for (det, pan) in [(-0.07f32, 0.3f32), (0.07, 0.7)] {
				let f = f0 * 2f32.powf(det / 12.0);
				let amp = 0.035 / (1.0 + k as f32 * 0.25);
				let mut phase = (k as f32 * 1.3 + det) * 6.28;
				for i in s0..s1 {
					let t = i as f32 / sr as f32;
					// Crossfade envelope around the section boundaries.
					let env = clamp01((t - (start - 1.2)) / 2.4) * clamp01(((end + 1.2) - t) / 2.4);
					let swell = 0.85 + 0.15 * (t * 0.35 + k as f32).sin();
					phase += 6.2831853 * f / sr as f32;
					let v = (phase.sin() + 0.18 * (2.0 * phase).sin()) * amp * env * swell;
					l[i] += v * (1.0 - pan);
					r[i] += v * pan;
				}
			}
		}
	}
	// Low heartbeat pulse during the speed section.
	for beat in 0..16 {
		let t0 = 15.0 + beat as f32 * 0.5;
		let i0 = (t0 * sr as f32) as usize;
		for j in 0..(sr / 3) {
			let t = j as f32 / sr as f32;
			let v = (6.2831853 * 55.0 * t).sin() * (-t * 9.0).exp() * 0.12;
			if i0 + j < n {
				l[i0 + j] += v;
				r[i0 + j] += v;
			}
		}
	}
	// Clicks at the footage's click frames (clip starts from the film timeline).
	for (name, start) in [("count", 22.9f32), ("area", 28.7), ("select", 35.0)] {
		if let Some(c) = clips.get(name) {
			for &k in &c.clicks {
				let t0 = start + k as f32 / FPS;
				let i0 = (t0 * sr as f32) as usize;
				for j in 0..(sr / 40) {
					let t = j as f32 / sr as f32;
					let v = ((6.2831853 * 2400.0 * t).sin() * 0.6 + (6.2831853 * 900.0 * t).sin()) * (-t * 260.0).exp() * 0.10;
					if i0 + j < n {
						l[i0 + j] += v;
						r[i0 + j] += v;
					}
				}
			}
		}
	}
	// Gentle one-pole low-pass, master fade, soft limit.
	let mut lp = (0f32, 0f32);
	let a = 0.12;
	for i in 0..n {
		lp.0 += a * (l[i] - lp.0);
		lp.1 += a * (r[i] - lp.1);
		let t = i as f32 / sr as f32;
		let fade = clamp01(t / 1.5) * clamp01((LENGTH - t) / 2.0);
		l[i] = (lp.0 * fade * 1.6).tanh();
		r[i] = (lp.1 * fade * 1.6).tanh();
	}
	// 16-bit WAV.
	let mut f = std::fs::File::create(path).unwrap();
	let data_len = (n * 4) as u32;
	f.write_all(b"RIFF").unwrap();
	f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
	f.write_all(b"WAVEfmt ").unwrap();
	f.write_all(&16u32.to_le_bytes()).unwrap();
	f.write_all(&1u16.to_le_bytes()).unwrap();
	f.write_all(&2u16.to_le_bytes()).unwrap();
	f.write_all(&(sr as u32).to_le_bytes()).unwrap();
	f.write_all(&(sr as u32 * 4).to_le_bytes()).unwrap();
	f.write_all(&4u16.to_le_bytes()).unwrap();
	f.write_all(&16u16.to_le_bytes()).unwrap();
	f.write_all(b"data").unwrap();
	f.write_all(&data_len.to_le_bytes()).unwrap();
	let mut buf = Vec::with_capacity(n * 4);
	for i in 0..n {
		buf.extend_from_slice(&((l[i] * 32000.0) as i16).to_le_bytes());
		buf.extend_from_slice(&((r[i] * 32000.0) as i16).to_le_bytes());
	}
	f.write_all(&buf).unwrap();
}

fn main() {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let root = PathBuf::from(args.first().cloned().unwrap_or_else(|| "../..".into()));
	let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "takeoff-desk-launch.mp4".into()));
	// Optional: render only one frame at time T (seconds) to a PNG for review.
	let still: Option<f32> = std::env::var("STILL").ok().and_then(|s| s.parse().ok());

	let leak = |p: &str| -> &'static [u8] { Box::leak(std::fs::read(root.join(p)).unwrap_or_else(|_| panic!("missing {p}")).into_boxed_slice()) };
	let fonts = Fonts {
		bold: FontRef::try_from_slice(leak("assets/fonts/Inter-Bold.ttf")).unwrap(),
		semi: FontRef::try_from_slice(leak("assets/fonts/Inter-SemiBold.ttf")).unwrap(),
		regular: FontRef::try_from_slice(leak("assets/fonts/Inter-Regular.ttf")).unwrap(),
	};
	let icon = {
		let img = image::open(root.join("assets/icon.png")).unwrap().to_rgba8();
		let mut pm = Pixmap::new(img.width(), img.height()).unwrap();
		for (d, s) in pm.pixels_mut().iter_mut().zip(img.pixels()) {
			*d = ColorU8::from_rgba(s[0], s[1], s[2], s[3]).premultiply();
		}
		pm
	};
	let assets = Assets { icon };
	let mut clips = HashMap::new();
	for name in ["count", "area", "select", "layers", "ai", "compare", "models"] {
		let c = Clip::load(&root, name);
		eprintln!("clip {name}: {} frames", c.frames);
		clips.insert(name.to_string(), c);
	}
	let mut footage = Footage { clips, cache: HashMap::new() };
	let mut ctx = Ctx { pix: Pixmap::new(W, H).unwrap(), fonts: &fonts };

	if let Some(t) = still {
		render(&mut ctx, &mut footage, &assets, t);
		ctx.pix.save_png(&out).unwrap();
		eprintln!("still {t}s → {}", out.display());
		return;
	}

	let wav = out.with_extension("wav");
	score(&footage.clips, &wav);
	let mut ff = Command::new("ffmpeg")
		.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{W}x{H}"), "-r", "30", "-i", "-"])
		.args(["-i"])
		.arg(&wav)
		.args(["-c:v", "libx264", "-preset", "slow", "-crf", "17", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-c:a", "aac", "-b:a", "192k", "-shortest"])
		.arg(&out)
		.stdin(Stdio::piped())
		.spawn()
		.expect("ffmpeg");
	let mut stdin = ff.stdin.take().unwrap();
	let frames = (LENGTH * FPS) as usize;
	for i in 0..frames {
		let t = i as f32 / FPS;
		render(&mut ctx, &mut footage, &assets, t);
		// Pixmap is premultiplied; every pixel ends opaque, so it equals straight RGBA.
		stdin.write_all(ctx.pix.data()).unwrap();
		if i % 150 == 0 {
			eprintln!("{:>5.1}s / {LENGTH}s", t);
		}
	}
	drop(stdin);
	ff.wait().unwrap();
	let _ = std::fs::remove_file(&wav);
	eprintln!("✓ {}", out.display());
}
