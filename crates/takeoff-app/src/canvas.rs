//! The drawing canvas: page rendering (base + sharp textures), the takeoff
//! overlay, selection and every tool's interaction.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use eframe::egui::{self, Align2, Color32, CursorIcon, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};
use takeoff_core::geometry::measurement_quantity;
use takeoff_core::snap::{Snap, SnapKind};
use takeoff_core::units::format_quantity;
use takeoff_core::*;

use crate::docstate::OpenDoc;
use crate::geom;
use crate::theme::{self, hex, Palette};
use crate::tools::Tool;

/// What the canvas asks the app to do (dialogs it can't show itself).
#[derive(Debug, Clone)]
pub enum CanvasRequest {
	Calibrate { page: u32, p1: Point, p2: Point },
	Viewport { page: u32, rect: (f64, f64, f64, f64) },
	Depth { id: String },
	Status(String),
}

enum Drag {
	Pan,
	Move { last: Point, moved: bool },
	Marquee { start: Pos2 },
	Vertex { id: String, idx: usize, cutout: Option<usize> },
	Resize { id: String, corner: usize },
	Shape { start: Point },
	Ink { points: Vec<Point> },
}

pub struct Canvas {
	pub tool: Tool,
	pub active_category: Option<String>,
	pub annot_style: AnnotationStyle,
	pub highlight_color: String,
	pub stamp_text: String,
	/// Points placed so far by a multi-point tool.
	pub points: Vec<Point>,
	/// Callout waiting for its leader tip: (box a, box b).
	callout_box: Option<(Point, Point)>,
	drag: Option<Drag>,
	pub editing_text: Option<String>,
	text_buffer: String,
	hover_snap: Option<Snap>,
	pub clipboard: Clipboard,
	pub ghost: Option<(u32, (f64, f64, f64, f64), String, Instant)>,
	pub flash: Option<(HashSet<String>, Instant)>,
	seq_cache: Option<(u64, HashMap<String, u32>, HashMap<String, u32>)>,
	pub requests: Vec<CanvasRequest>,
	pub cursor_page: Option<Point>,
	/// The page `cursor_page` is on.
	pub cursor_on: u32,
	pub show_badges: bool,
	/// Mapped callouts are links (click one to go where it points).
	pub map_links: bool,
	/// How Length / Area points are joined.
	pub curve: CurveMode,
	/// Along-the-line: the path traced through the points placed (and
	/// whether a span is still being traced in the picture).
	follow_cache: Option<(Vec<Point>, Vec<Point>, bool)>,
	/// The last scroll came from a trackpad (pixel deltas), not a mouse wheel.
	trackpad: bool,
	/// The person zoomed or panned this frame (stops following the AI).
	pub user_moved: bool,
	/// Rasterized overlays by id: (content key, texture or why it failed).
	overlay_tex: HashMap<String, (u64, Result<egui::TextureHandle, String>)>,
	/// A fill the worker is making.
	fill_job: Option<FillJob>,
	/// The last fill: (measurement, page, points clicked), for Shift-click to add to.
	last_fill: Option<(String, u32, Vec<Point>)>,
}

struct FillJob {
	token: u64,
	page: u32,
	at: Vec<Point>,
	/// Shift-click: the fill to grow, instead of a new one.
	grow: Option<String>,
}

impl Default for Canvas {
	fn default() -> Self {
		Self {
			tool: Tool::Select,
			active_category: None,
			annot_style: AnnotationStyle::default(),
			highlight_color: "#FFD60A".into(),
			stamp_text: "APPROVED".into(),
			points: vec![],
			callout_box: None,
			drag: None,
			editing_text: None,
			text_buffer: String::new(),
			hover_snap: None,
			clipboard: Clipboard::default(),
			ghost: None,
			flash: None,
			seq_cache: None,
			requests: vec![],
			cursor_page: None,
			cursor_on: 1,
			show_badges: true,
			map_links: true,
			curve: CurveMode::Straight,
			follow_cache: None,
			trackpad: false,
			user_moved: false,
			overlay_tex: HashMap::new(),
			fill_job: None,
			last_fill: None,
		}
	}
}

/// How the Length / Area tools join the points clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CurveMode {
	#[default]
	Straight,
	/// A smooth curve through the points.
	Smooth,
	/// Along the drawing's own line between each pair of points.
	Follow,
}

impl CurveMode {
	pub fn label(self) -> &'static str {
		match self {
			CurveMode::Straight => "Straight lines",
			CurveMode::Smooth => "Smooth curve",
			CurveMode::Follow => "Along the drawn line",
		}
	}
}

const HIT_PX: f32 = 6.0;
const SNAP_PX: f32 = 10.0;
const UNGROUPED: &str = "#22D3EE";

fn with_alpha(c: Color32, a: f32) -> Color32 {
	Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

impl Canvas {
	pub fn set_tool(&mut self, tool: Tool) {
		self.tool = tool;
		self.points.clear();
		self.callout_box = None;
		self.drag = None;
	}

	pub fn is_busy(&self) -> bool {
		!self.points.is_empty() || self.drag.is_some() || self.callout_box.is_some()
	}

	/// Something in progress belongs to the current page (a shape, a drag,
	/// a note being typed), so it stays current wherever the pointer goes.
	fn holds_page(&self) -> bool {
		!self.points.is_empty() || self.callout_box.is_some() || self.editing_text.is_some() || self.drag.as_ref().is_some_and(|d| !matches!(d, Drag::Pan))
	}

	/// Escape: cancel what's in progress, else drop back to Select.
	pub fn escape(&mut self, doc: &mut OpenDoc) {
		if self.editing_text.is_some() {
			self.commit_text(doc);
		} else if self.is_busy() {
			self.points.clear();
			self.callout_box = None;
			self.drag = None;
		} else if self.tool != Tool::Select {
			self.tool = Tool::Select;
			self.active_category = None;
		} else {
			doc.selection.clear();
		}
	}

	/// The category the current tool records into, creating one if needed.
	fn category_for(&mut self, doc: &OpenDoc, kind: CategoryKind) -> String {
		let mut d = doc.doc.lock();
		if let Some(id) = &self.active_category {
			if d.data.category(id).is_some_and(|c| c.kind() == kind) {
				return id.clone();
			}
		}
		if let Some(c) = d.data.categories.iter().find(|c| c.kind() == kind) {
			let id = c.id.clone();
			self.active_category = Some(id.clone());
			return id;
		}
		let n = d.data.categories.iter().filter(|c| c.kind() == kind).count() + 1;
		let name = match kind {
			CategoryKind::Count => format!("Count {n}"),
			CategoryKind::Length => format!("Length {n}"),
			CategoryKind::Area => format!("Area {n}"),
		};
		let color = d.data.next_category_color().to_string();
		let id = new_id("c");
		let c = Category::new(id.clone(), name, color, kind);
		d.edit("New item", |t| t.categories.push(c));
		self.active_category = Some(id.clone());
		id
	}

	fn seqs(&mut self, d: &Document) -> (&HashMap<String, u32>, &HashMap<String, u32>) {
		let rev = d.revision();
		if self.seq_cache.as_ref().is_none_or(|(r, _, _)| *r != rev) {
			self.seq_cache = Some((rev, numbering::mark_seq(&d.data.marks), numbering::measurement_seq(&d.data.measurements)));
		}
		let (_, a, b) = self.seq_cache.as_ref().unwrap();
		(a, b)
	}

	/// Turn a finished fill into an area measurement (or the bigger outline
	/// of the one Shift-clicked).
	fn poll_fill(&mut self, doc: &mut OpenDoc, ctx: &egui::Context) {
		let Some(job) = &self.fill_job else { return };
		let Some(res) = doc.take_fill(job.token) else {
			ctx.request_repaint_after(std::time::Duration::from_millis(50));
			return;
		};
		let job = self.fill_job.take().unwrap();
		use takeoff_pdf::fill::NoFill;
		match res {
			Some(Ok(f)) => {
				let id = match job.grow {
					Some(id) => {
						doc.doc.lock().edit("Fill area", |d| {
							if let Some(m) = d.measurements.iter_mut().find(|m| m.id == id) {
								m.points = f.points;
							}
						});
						id
					}
					None => {
						let cat = self.category_for(doc, CategoryKind::Area);
						let mut m = Measurement::new(new_id("ms"), job.page, MeasurementType::Area, f.points);
						m.category_id = Some(cat);
						m.meta = doc.doc.lock().new_meta();
						let id = m.id.clone();
						doc.doc.lock().edit("Fill area", |d| d.measurements.push(m));
						id
					}
				};
				self.flash = Some(([id.clone()].into(), Instant::now()));
				self.last_fill = Some((id, job.page, job.at));
			}
			Some(Err(NoFill::OnLine)) => self.requests.push(CanvasRequest::Status("That's on a line. Click in the open space inside.".into())),
			Some(Err(NoFill::Open)) => self.requests.push(CanvasRequest::Status("No closed outline around that point: the lines leave it open. Use Area and click its corners.".into())),
			None => self.requests.push(CanvasRequest::Status("Couldn't read this page to fill it.".into())),
		}
	}

	// ── Main entry ──────────────────────────────────────────────────────

	pub fn ui(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette, snap_on: bool, wheel_zooms: bool) {
		let rect = ui.available_rect_before_wrap();
		let resp = ui.allocate_rect(rect, Sense::click_and_drag());
		if doc.canvas_rect != rect {
			doc.canvas_rect = rect;
			doc.touch_view();
		}
		let ctx = ui.ctx().clone();
		doc.update_renders(&ctx);
		self.poll_fill(doc, &ctx);
		let painter = ui.painter_at(rect);
		painter.rect_filled(rect, 0.0, p.canvas);

		if !doc.loaded {
			match doc.error.clone() {
				Some(e) => {
					painter.text(rect.center(), Align2::CENTER_CENTER, e, theme::medium(14.0), p.danger);
				}
				None => {
					let c = rect.center();
					ui.put(Rect::from_center_size(c - Vec2::new(0.0, 22.0), Vec2::splat(30.0)), egui::Spinner::new().size(30.0).color(p.accent));
					painter.text(c + Vec2::new(0.0, 14.0), Align2::CENTER_CENTER, format!("Opening {}…", doc.name), theme::medium(14.0), p.text2);
					painter.text(c + Vec2::new(0.0, 36.0), Align2::CENTER_CENTER, "Large drawing sets can take a moment", theme::medium(12.0), p.muted);
				}
			}
			return;
		}

		self.navigate(ui, &resp, doc, wheel_zooms);
		if !self.holds_page() {
			doc.follow_view();
		}
		// Every page on screen (one, unless continuous or spreads), each
		// painted as the current page: pictures first, then the takeoff.
		let pages = doc.visible_pages();
		let current = doc.page;
		for &pg in &pages {
			doc.page = pg;
			self.paint_page(&painter, doc, p);
			self.page_spinner(ui, doc, p);
		}
		doc.page = current;
		self.paint_sheet_overlays(&ctx, &painter, doc, p, &pages);
		for &pg in &pages {
			doc.page = pg;
			self.paint_items(&painter, doc, p);
			self.paint_overlays(&painter, doc, p);
			self.paint_map(&painter, doc, p);
			self.paint_annotation_text(ui, doc);
		}
		doc.page = current;
		// Tools work on the page under the pointer: it is current while they do.
		let hover = ui.input(|i| i.pointer.hover_pos()).filter(|h| doc.canvas_rect.contains(*h));
		let under = hover.and_then(|h| doc.page_at(h)).filter(|_| doc.layout.multi() && !self.holds_page());
		if let Some(pg) = under {
			doc.page = pg;
		}
		self.interact(ui, &resp, doc, p, snap_on);
		self.update_follow_cache(doc);
		self.paint_in_progress(&painter, doc, p);
		self.text_editor(ui, doc);
		self.context_menu(&resp, doc);
		// Just hovering leaves the current page as it was; a click (or a shape begun) makes it this one.
		let pressed = ui.input(|i| i.pointer.any_pressed());
		if under.is_some() && under == Some(doc.page) && !pressed && !self.holds_page() {
			doc.page = current;
		}
	}

	fn navigate(&mut self, ui: &egui::Ui, resp: &egui::Response, doc: &mut OpenDoc, wheel_zooms: bool) {
		let hovered = resp.hovered() || resp.dragged();
		let (zoom, scroll, mods, hover_pos) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers, i.pointer.hover_pos()));
		// Trackpads scroll in points, mouse wheels in lines. Remember which, since
		// smoothed wheel scrolling carries on for frames after the event.
		if let Some(t) = ui.input(|i| {
			i.raw.events.iter().rev().find_map(|e| match e {
				egui::Event::MouseWheel { unit, .. } => Some(*unit == egui::MouseWheelUnit::Point),
				_ => None,
			})
		}) {
			self.trackpad = t;
		}
		if hovered && (zoom != 1.0 || scroll != Vec2::ZERO) {
			self.user_moved = true;
		}
		if hovered {
			let anchor = hover_pos.unwrap_or(doc.canvas_rect.center());
			if zoom != 1.0 {
				doc.zoom_at(anchor, zoom);
			} else if scroll != Vec2::ZERO {
				// Two fingers on a trackpad move the sheet (pinch zooms), like every Mac app.
				if wheel_zooms && !self.trackpad && !mods.shift && !mods.alt {
					let factor = (scroll.y / 400.0).exp();
					doc.zoom_at(anchor, factor);
				} else {
					doc.pan += scroll;
					doc.touch_view();
				}
			}
		}
		// Space-drag / middle-drag / pan tool.
		let space = ui.input(|i| i.key_down(Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
		let middle = ui.input(|i| i.pointer.middle_down());
		if (resp.drag_started() && (self.tool == Tool::Pan || space)) || (middle && resp.dragged()) {
			self.drag = Some(Drag::Pan);
		}
		if matches!(self.drag, Some(Drag::Pan)) {
			if resp.dragged() {
				doc.pan += resp.drag_delta();
				doc.touch_view();
				self.user_moved = true;
				ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
			}
			if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
				self.drag = None;
			}
		} else if (space || self.tool == Tool::Pan) && resp.hovered() {
			ui.ctx().set_cursor_icon(CursorIcon::Grab);
		}
	}

	fn page_spinner(&self, ui: &mut egui::Ui, doc: &OpenDoc, p: &Palette) {
		let waiting_base = !doc.base.contains_key(&doc.page) && !doc.thumbs.contains_key(&doc.page);
		if waiting_base {
			let r = doc.page_rect().intersect(doc.canvas_rect);
			if r.width() > 40.0 && r.height() > 40.0 {
				ui.put(Rect::from_center_size(r.center(), Vec2::splat(28.0)), egui::Spinner::new().size(28.0).color(p.accent));
			}
		}
	}

	fn paint_page(&self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
		let pr = doc.page_rect();
		// Fluent-style elevation: two soft shadow layers.
		for (grow, alpha) in [(6.0, if p.dark { 60 } else { 18 }), (2.0, if p.dark { 90 } else { 28 })] {
			painter.rect_filled(pr.expand(grow).translate(Vec2::new(0.0, grow / 2.0)), 2.0, Color32::from_black_alpha(alpha));
		}
		painter.rect_filled(pr, 0.0, Color32::WHITE);
		let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
		// A page far out in a continuous view may only have its thumbnail.
		if let Some(tex) = doc.base.get(&doc.page).or(doc.thumbs.get(&doc.page)) {
			painter.image(tex.id(), pr, uv, Color32::WHITE);
		}
		if let Some(s) = doc.sharp.as_ref().filter(|s| s.page == doc.page).or(doc.side_sharp.get(&doc.page)) {
			let r = Rect::from_min_size(
				Pos2::new(pr.min.x + s.region.x as f32 * pr.width(), pr.min.y + s.region.y as f32 * pr.height()),
				Vec2::new(s.region.w as f32 * pr.width(), s.region.h as f32 * pr.height()),
			);
			painter.image(s.tex.id(), r, uv, Color32::WHITE);
		}
		if let Some(c) = &doc.compare {
			if c.page == doc.page && c.show_overlay {
				match &c.tex {
					Some(t) => {
						painter.image(t.id(), pr, uv, Color32::WHITE);
					}
					None => {
						painter.rect_filled(pr, 0.0, Color32::from_white_alpha(200));
						painter.text(pr.center(), Align2::CENTER_CENTER, "Comparing revisions…", theme::medium(13.0), Color32::from_gray(90));
					}
				}
			}
		}
	}

	/// Overlays (SVG / images the AI or a person laid over the sheet), between
	/// the page and the takeoff so marks stay on top.
	fn paint_sheet_overlays(&mut self, ctx: &egui::Context, painter: &egui::Painter, doc: &OpenDoc, p: &Palette, pages: &[u32]) {
		let overlays: Vec<Overlay> = doc.doc.lock().data.overlays.iter().filter(|o| pages.contains(&o.page) && !o.hidden).cloned().collect();
		for o in &overlays {
			let pr = doc.page_rect_of(o.page);
			let at = |x: f64, y: f64| Pos2::new(pr.min.x + x as f32 * pr.width(), pr.min.y + y as f32 * pr.height());
			let r = Rect::from_min_max(at(o.x, o.y), at(o.x + o.w, o.y + o.h));
			if !r.is_positive() {
				continue;
			}
			let key = overlay_key(o);
			let stale = self.overlay_tex.get(&o.id).is_none_or(|(k, _)| *k != key);
			if stale {
				// Raster at a fixed long edge (re-rasterizing per zoom step would stall panning).
				let s = 2048.0 / r.width().max(r.height());
				let (w, h) = ((r.width() * s).round() as u32, (r.height() * s).round() as u32);
				let tex = takeoff_pdf::overlay::rasterize(o, w, h).map(|img| {
					let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
					ctx.load_texture(format!("overlay-{}", o.id), ci, egui::TextureOptions::LINEAR)
				});
				self.overlay_tex.insert(o.id.clone(), (key, tex));
			}
			match &self.overlay_tex[&o.id].1 {
				Ok(t) => {
					painter.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE.gamma_multiply(o.opacity.clamp(0.0, 1.0) as f32));
				}
				Err(e) => {
					painter.rect_stroke(r, 0.0, Stroke::new(1.0, p.danger), StrokeKind::Inside);
					painter.text(r.left_top() + Vec2::new(6.0, 6.0), Align2::LEFT_TOP, format!("Overlay {}: {e}", o.label.as_deref().unwrap_or(&o.id)), theme::medium(11.0), p.danger);
				}
			}
		}
		// Drop textures of overlays that are gone (deleted, other page).
		let live: HashSet<&str> = overlays.iter().map(|o| o.id.as_str()).collect();
		self.overlay_tex.retain(|id, _| live.contains(id.as_str()));
	}

	fn paint_items(&mut self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
		let page = doc.page;
		let lock = doc.doc.clone();
		let d = lock.lock();
		let (mseq, sseq) = {
			let (a, b) = self.seqs(&d);
			(a.clone(), b.clone())
		};
		let data = &d.data;
		let cats: HashMap<&str, &Category> = data.categories.iter().map(|c| (c.id.as_str(), c)).collect();
		let zoom = doc.zoom;
		let sel = &doc.selection;
		let flash: HashSet<String> = self.flash.as_ref().filter(|(_, t)| t.elapsed().as_secs_f32() < 1.6).map(|(s, _)| s.clone()).unwrap_or_default();
		let mut shapes: Vec<Shape> = Vec::new();
		let mut labels: Vec<(Pos2, String, Color32, bool)> = Vec::new();
		let to = |pt: &Point| doc.to_screen(*pt);
		let sw = |w: f32| (w * zoom).clamp(1.0, 6.0);

		// Annotations under quantities.
		for a in data.annotations.iter().filter(|a| a.page == page) {
			let color = with_alpha(hex(&a.style.stroke), a.style.opacity as f32);
			let width = (a.style.width as f32 * zoom).max(1.0);
			let stroke = Stroke::new(width, color);
			let fill = a.style.fill.as_deref().map(|f| with_alpha(hex(f), 0.5 * a.style.opacity as f32));
			let pts: Vec<Pos2> = a.points.iter().map(to).collect();
			let dashed = matches!(a.style.line_style, Some(LineStyle::Dashed | LineStyle::Dotted));
			let outline = |pts: &[Pos2], closed: bool, shapes: &mut Vec<Shape>| {
				if dashed {
					geom::dashed(pts, closed, stroke, 8.0 * zoom.max(0.5), 5.0 * zoom.max(0.5), shapes);
				} else if closed {
					shapes.push(Shape::closed_line(pts.to_vec(), stroke));
				} else {
					shapes.push(Shape::line(pts.to_vec(), stroke));
				}
			};
			match a.kind {
				AnnotationKind::Rectangle | AnnotationKind::Text | AnnotationKind::Stamp | AnnotationKind::Callout | AnnotationKind::Highlight | AnnotationKind::Ellipse if pts.len() >= 2 => {
					let r = Rect::from_two_pos(pts[0], pts[1]);
					match a.kind {
						AnnotationKind::Highlight => shapes.push(Shape::rect_filled(r, 0.0, with_alpha(hex(&a.style.stroke), 0.35))),
						AnnotationKind::Ellipse => {
							let e = geom::ellipse_points(pts[0], pts[1], 72);
							if let Some(f) = fill {
								shapes.push(Shape::convex_polygon(e.clone(), f, Stroke::NONE));
							}
							outline(&e, true, &mut shapes);
						}
						_ => {
							let bg = fill.or(matches!(a.kind, AnnotationKind::Text | AnnotationKind::Callout).then(|| with_alpha(Color32::WHITE, 0.92)));
							if let Some(f) = bg {
								shapes.push(Shape::rect_filled(r, 0.0, f));
							}
							if a.kind == AnnotationKind::Stamp {
								shapes.push(Shape::rect_stroke(r, 3.0 * zoom, Stroke::new(width * 1.5, color), StrokeKind::Inside));
							} else if a.style.width > 0.0 {
								outline(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], true, &mut shapes);
							}
							if a.kind == AnnotationKind::Callout && pts.len() >= 3 {
								let from = Pos2::new(pts[2].x.clamp(r.left(), r.right()), pts[2].y.clamp(r.top(), r.bottom()));
								shapes.push(Shape::line_segment([from, pts[2]], stroke));
								shapes.push(geom::arrow_head(from, pts[2], (width * 4.0).max(8.0), color));
							}
						}
					}
				}
				AnnotationKind::Line | AnnotationKind::Arrow | AnnotationKind::Polyline | AnnotationKind::Ink => {
					outline(&pts, false, &mut shapes);
					if a.kind == AnnotationKind::Arrow && pts.len() >= 2 {
						shapes.push(geom::arrow_head(pts[pts.len() - 2], pts[pts.len() - 1], (width * 4.0).max(9.0), color));
					}
				}
				AnnotationKind::Polygon => {
					if let Some(f) = fill {
						shapes.push(geom::fill_polygon(&pts, &[], f));
					}
					outline(&pts, true, &mut shapes);
				}
				AnnotationKind::Cloud => {
					let c = geom::cloud(&pts, (14.0 * zoom).clamp(6.0, 60.0));
					if let Some(f) = fill {
						shapes.push(geom::fill_polygon(&pts, &[], f));
					}
					shapes.push(Shape::closed_line(c, stroke));
				}
				_ => {}
			}
			if sel.contains(&a.id) || flash.contains(&a.id) {
				if let Some(r) = bbox(&pts) {
					shapes.push(Shape::rect_stroke(r.expand(4.0), 2.0, Stroke::new(1.5, p.selection), StrokeKind::Outside));
				}
			}
		}

		// Measurements.
		for m in data.measurements.iter().filter(|m| m.page == page && !m.id.starts_with("probe_")) {
			let cat = m.category_id.as_deref().and_then(|c| cats.get(c));
			if cat.is_some_and(|c| c.hidden) {
				continue;
			}
			let color = hex(m.color.as_deref().or(cat.map(|c| c.color.as_str())).unwrap_or(if m.kind == MeasurementType::Area { "#A78BFA" } else { UNGROUPED }));
			let opacity = cat.and_then(|c| c.opacity).unwrap_or(1.0) as f32;
			let pts: Vec<Pos2> = m.points.iter().map(to).collect();
			let selected = sel.contains(&m.id) || flash.contains(&m.id);
			let width = sw(if selected { 3.0 } else { 2.0 }).max(if selected { 2.5 } else { 1.8 });
			let stroke = Stroke::new(width, with_alpha(color, opacity.max(0.6)));
			match m.kind {
				MeasurementType::Area => {
					let holes: Vec<Vec<Pos2>> = m.cutouts.iter().map(|c| c.iter().map(to).collect()).collect();
					shapes.push(geom::fill_polygon(&pts, &holes, with_alpha(color, 0.2 * opacity)));
					if let Some(h) = cat.and_then(|c| c.hatch) {
						let (angle, cross) = match h {
							Hatch::Diagonal => (45.0, false),
							Hatch::Cross => (45.0, true),
							Hatch::Grid => (0.0, true),
							Hatch::Dots => (45.0, false),
						};
						let hs = Stroke::new(1.0, with_alpha(color, 0.55 * opacity));
						let gap = (10.0 * zoom).clamp(5.0, 26.0);
						geom::hatch(&pts, &holes, angle, gap, hs, &mut shapes);
						if cross {
							geom::hatch(&pts, &holes, angle + 90.0, gap, hs, &mut shapes);
						}
					}
					shapes.push(Shape::closed_line(pts.clone(), stroke));
					for h in &holes {
						geom::dashed(h, true, Stroke::new(1.2, color), 5.0, 3.0, &mut shapes);
					}
				}
				MeasurementType::Length => {
					let dash = cat.and_then(|c| c.line_style);
					if matches!(dash, Some(LineStyle::Dashed | LineStyle::Dotted)) {
						let (a, b) = if dash == Some(LineStyle::Dotted) { (1.5, 4.0) } else { (9.0, 5.0) };
						geom::dashed(&pts, m.closed, stroke, a, b, &mut shapes);
					} else if m.closed {
						shapes.push(Shape::closed_line(pts.clone(), stroke));
					} else {
						shapes.push(Shape::line(pts.clone(), stroke));
					}
				}
				MeasurementType::Angle => {
					shapes.push(Shape::line(pts.clone(), stroke));
				}
			}
			// A curve is many short segments: dot only its ends, unless it is being edited.
			let dense = pts.len() > 12 && !selected;
			for (k, v) in pts.iter().enumerate() {
				if dense && k != 0 && k + 1 != pts.len() {
					continue;
				}
				shapes.push(Shape::circle_filled(*v, if selected { 4.0 } else { 2.5 }, color));
			}
			if selected {
				for v in &pts {
					shapes.push(Shape::circle_stroke(*v, 5.5, Stroke::new(1.5, p.selection)));
				}
			}
			if self.show_badges && !pts.is_empty() {
				let n = sseq.get(&m.id).copied().unwrap_or(0);
				let q = match (m.value, &m.unit) {
					(Some(v), Some(u)) => format_quantity(v, u, &data.display),
					_ if m.kind == MeasurementType::Angle => measurement_quantity(m, None, 1.0, 1.0).map(|_| String::new()).unwrap_or_default(),
					_ => "no scale".into(),
				};
				let q = if m.kind == MeasurementType::Angle {
					let (w, h) = doc.page_size(page);
					measurement_quantity(m, None, w, h).map(|q| format!("{:.1}°", q.value)).unwrap_or_default()
				} else {
					q
				};
				let anchor = if m.kind == MeasurementType::Area { centroid(&pts) } else { mid_of(&pts) };
				labels.push((anchor, format!("{n}  {q}"), color, selected));
			}
		}

		// Count marks on top.
		for m in data.marks.iter().filter(|m| m.page == page) {
			let cat = m.category_id.as_deref().and_then(|c| cats.get(c));
			if cat.is_some_and(|c| c.hidden) {
				continue;
			}
			let color = hex(cat.map(|c| c.color.as_str()).unwrap_or(UNGROUPED));
			let opacity = cat.and_then(|c| c.opacity).unwrap_or(1.0) as f32;
			let r = Rect::from_two_pos(to(&Point { x: m.x, y: m.y }), to(&Point { x: m.x + m.w, y: m.y + m.h }));
			let r = if r.width() < 6.0 { Rect::from_center_size(r.center(), Vec2::splat(6.0)) } else { r };
			let selected = sel.contains(&m.id) || flash.contains(&m.id);
			shapes.push(Shape::rect_filled(r, 2.0, with_alpha(color, 0.18 * opacity)));
			let stroke = Stroke::new(if selected { 2.5 } else { 1.6 }, with_alpha(color, opacity.max(0.7)));
			if m.status == MarkStatus::Pending {
				geom::dashed(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], true, stroke, 4.0, 3.0, &mut shapes);
			} else {
				shapes.push(Shape::rect_stroke(r, 2.0, stroke, StrokeKind::Middle));
			}
			if selected {
				shapes.push(Shape::rect_stroke(r.expand(3.0), 3.0, Stroke::new(1.5, p.selection), StrokeKind::Outside));
			}
			if self.show_badges {
				let n = mseq.get(&m.id).copied().unwrap_or(0);
				badge(&mut shapes, painter, r.left_top() + Vec2::new(-2.0, -2.0), &n.to_string(), color, m.status == MarkStatus::Pending);
			}
		}
		painter.extend(shapes);
		for (at, text, color, selected) in labels {
			pill(painter, at, &text, color, selected);
		}

		// Scale reference lines and viewports.
		for s in data.page_scales.iter().filter(|s| s.page == page) {
			if s.p1 == (Point { x: 0.0, y: 0.0 }) && s.p2.y == 0.0 {
				continue;
			}
			let (a, b) = (to(&s.p1), to(&s.p2));
			let mut v = Vec::new();
			geom::dashed(&[a, b], false, Stroke::new(2.0, p.warning), 6.0, 4.0, &mut v);
			painter.extend(v);
			for e in [a, b] {
				painter.circle_filled(e, 3.0, p.warning);
			}
		}
		for vp in data.viewports.iter().filter(|v| v.page == page) {
			let r = Rect::from_two_pos(to(&Point { x: vp.x, y: vp.y }), to(&Point { x: vp.x + vp.w, y: vp.y + vp.h }));
			let mut v = Vec::new();
			geom::dashed(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], true, Stroke::new(1.5, p.warning), 8.0, 5.0, &mut v);
			painter.extend(v);
			painter.text(r.left_top() + Vec2::new(6.0, 4.0), Align2::LEFT_TOP, format!("{} · {} {}", vp.name, trim(vp.scale.distance), vp.scale.unit), theme::medium(11.0), p.warning);
		}
	}

	fn paint_overlays(&mut self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
		// Search hits on this page.
		if let Some(s) = &doc.search {
			for (i, h) in s.hits.iter().enumerate().filter(|(_, h)| h.page == doc.page) {
				let r = Rect::from_two_pos(doc.to_screen(Point { x: h.rect.0, y: h.rect.1 }), doc.to_screen(Point { x: h.rect.0 + h.rect.2, y: h.rect.1 + h.rect.3 })).expand(2.0);
				let active = s.active == Some(i);
				painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(255, 214, 10, if active { 120 } else { 70 }));
				if active {
					painter.rect_stroke(r, 2.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
				}
			}
		}
		// The agent's "ghost viewport": where it is looking right now.
		if let Some((page, reg, label, at)) = &self.ghost {
			let age = at.elapsed().as_secs_f32();
			if *page == doc.page && age < 30.0 {
				let r = Rect::from_two_pos(doc.to_screen(Point { x: reg.0, y: reg.1 }), doc.to_screen(Point { x: reg.0 + reg.2, y: reg.1 + reg.3 }));
				let pulse = 0.55 + 0.25 * (age * 3.0).sin();
				painter.rect_filled(r, 4.0, with_alpha(p.accent, 0.06));
				painter.rect_stroke(r, 4.0, Stroke::new(2.0, with_alpha(p.accent, pulse)), StrokeKind::Middle);
				let tag = format!("{}  {label}", crate::icons::SPARKLES);
				let galley = painter.layout_no_wrap(tag, theme::medium(11.0), p.on_accent);
				let tr = Rect::from_min_size(r.left_top() - Vec2::new(0.0, galley.size().y + 6.0), galley.size() + Vec2::new(12.0, 6.0));
				painter.rect_filled(tr, 3.0, p.accent_fill);
				painter.galley(tr.min + Vec2::new(6.0, 3.0), galley, p.on_accent);
				painter.ctx().request_repaint_after(std::time::Duration::from_millis(60));
			}
		}
	}

	/// The document map on the sheet: mapped callouts as links, the area
	/// under the pointer with where it leads, the area the Map tab shows,
	/// and the one just arrived at.
	fn paint_map(&self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
		let Some(map) = &doc.map else { return };
		let Some(sheet) = map.sheet(doc.page) else { return };
		let to_rect = |r: docmap::Region| Rect::from_two_pos(doc.to_screen(Point { x: r.x, y: r.y }), doc.to_screen(Point { x: r.x + r.w, y: r.y + r.h }));
		let hover = painter.ctx().input(|i| i.pointer.hover_pos()).filter(|h| doc.canvas_rect.contains(*h));
		let linking = self.map_links && self.tool == Tool::Select;
		if linking {
			for part in sheet.parts.iter().filter(|x| x.kind == docmap::PartKind::Callout) {
				let Some(r) = part.region else { continue };
				let Some(l) = map.links.iter().find(|l| l.from == part.id) else { continue };
				let c = crate::graph_view::link_color(l.kind);
				let rect = to_rect(r).expand(3.0);
				painter.rect_filled(rect, 4.0, with_alpha(c, 0.12));
				painter.rect_stroke(rect, 4.0, Stroke::new(1.5, with_alpha(c, 0.75)), StrokeKind::Outside);
			}
		}
		// The area the Map tab shows (picked on the sheet), outlined quietly.
		if let Some(f) = doc.nav.focus.as_ref().filter(|f| f.contains('.')) {
			if let Some(r) = map.node(f).filter(|n| n.page() == doc.page).and_then(|n| n.region()) {
				painter.rect_stroke(to_rect(r).expand(2.0), 4.0, Stroke::new(1.5, with_alpha(p.accent, 0.6)), StrokeKind::Outside);
			}
		}
		// Just arrived: the area pulses for a moment.
		if let Some((id, at)) = &doc.nav.flash {
			let age = at.elapsed().as_secs_f32();
			if let Some(r) = map.node(id).filter(|n| n.page() == doc.page && age < 2.4).and_then(|n| n.region()) {
				let a = (1.0 - age / 2.4).powf(0.7) * (0.6 + 0.4 * (age * 7.0).cos().abs());
				let rect = to_rect(r).expand(4.0 + 6.0 * (1.0 - age / 2.4));
				painter.rect_filled(rect, 6.0, with_alpha(p.accent, 0.10 * a));
				painter.rect_stroke(rect, 6.0, Stroke::new(3.0, with_alpha(p.accent, a)), StrokeKind::Outside);
				painter.ctx().request_repaint();
			}
		}
		// Under the pointer: what it is and where it leads.
		let (Some(h), true) = (hover, linking) else { return };
		if self.drag.is_some() || self.hit(doc, h).is_some() {
			return;
		}
		let Some(id) = doc.map_area_at(doc.to_page(h)) else { return };
		let Some(docmap::NodeRef::Part(_, part)) = map.node(&id) else { return };
		let targets: Vec<&docmap::Link> = map.links.iter().filter(|l| l.from == id).collect();
		let callout = part.kind == docmap::PartKind::Callout;
		if !callout && targets.is_empty() {
			return;
		}
		if let Some(r) = part.region {
			painter.rect_stroke(to_rect(r).expand(3.0), 4.0, Stroke::new(2.0, crate::graph_view::kind_color(part.kind)), StrokeKind::Outside);
		}
		let mut lines: Vec<String> = targets.iter().take(3).map(|l| format!("→ {}", map.node(&l.to).map(|n| n.title()).unwrap_or_else(|| l.to.clone()))).collect();
		if targets.len() > 3 {
			lines.push(format!("+{} more in the Map tab", targets.len() - 3));
		}
		let head = if callout && !targets.is_empty() { "Click to go there" } else { "Click for its links" };
		let text = format!("{}\n{}", lines.join("\n"), head);
		let g = painter.layout(text.trim_start().to_string(), theme::medium(11.5), p.text, 320.0);
		let at = h + Vec2::new(14.0, 16.0);
		let r = Rect::from_min_size(at, g.size() + Vec2::new(14.0, 10.0));
		let r = r.translate(Vec2::new((doc.canvas_rect.right() - r.right()).min(0.0), (doc.canvas_rect.bottom() - r.bottom()).min(0.0)));
		painter.rect_filled(r, 5.0, p.panel);
		painter.rect_stroke(r, 5.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
		painter.galley(r.min + Vec2::new(7.0, 5.0), g, p.text);
		if callout && !targets.is_empty() {
			painter.ctx().set_cursor_icon(CursorIcon::PointingHand);
		}
	}

	fn paint_in_progress(&self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
		let hover = painter.ctx().input(|i| i.pointer.hover_pos());
		let cursor = hover.filter(|h| doc.canvas_rect.contains(*h)).map(|h| self.hover_snap.map(|s| doc.to_screen(s.point)).unwrap_or(h));
		let accent = p.accent;
		let stroke = Stroke::new(2.0, accent);
		if !self.points.is_empty() {
			let closed = matches!(self.tool, Tool::Area | Tool::Volume | Tool::Cutout | Tool::Annot(AnnotationKind::Polygon | AnnotationKind::Cloud));
			// The run as it will be stored: curved through the cursor, or traced along the line so far.
			let mut page_pts = self.points.clone();
			if let Some(c) = cursor {
				page_pts.push(doc.to_page(c));
			}
			let shaped: Vec<Point> = match (self.curve_mode(), self.curves()) {
				(CurveMode::Smooth, true) => {
					let (w, h) = doc.page_size(doc.page);
					takeoff_core::geometry::smooth_path(&page_pts, closed && page_pts.len() >= 3, w, h)
				}
				(CurveMode::Follow, true) => {
					let mut v = self.follow_cache.as_ref().map(|c| c.1.clone()).unwrap_or_else(|| self.points.clone());
					v.extend(page_pts.last().copied().filter(|_| cursor.is_some()));
					v
				}
				_ => page_pts.clone(),
			};
			let mut pts: Vec<Pos2> = shaped.iter().map(|q| doc.to_screen(*q)).collect();
			if pts.is_empty() {
				pts = self.points.iter().map(|q| doc.to_screen(*q)).collect();
			}
			if closed && pts.len() >= 3 {
				painter.add(geom::fill_polygon(&pts, &[], with_alpha(accent, 0.12)));
			}
			if closed && pts.len() >= 3 {
				painter.add(Shape::closed_line(pts.clone(), stroke));
			} else {
				painter.add(Shape::line(pts.clone(), stroke));
			}
			for q in &self.points {
				painter.circle(doc.to_screen(*q), 3.5, Color32::WHITE, Stroke::new(1.5, accent));
			}
			// Live quantity next to the cursor (of the run as shaped).
			if let Some(c) = cursor {
				let all = if self.curves() { shaped.clone() } else { page_pts.clone() };
				if let Some(text) = self.live_quantity(doc, &all) {
					let text = if self.curves() && self.curve_mode() != CurveMode::Straight { format!("{text} · {}", self.curve_mode().label().to_lowercase()) } else { text };
					pill(painter, c + Vec2::new(16.0, 18.0), &text, accent, true);
				}
			}
		}
		if let Some(Drag::Shape { start }) = &self.drag {
			if let Some(c) = cursor {
				let r = Rect::from_two_pos(doc.to_screen(*start), c);
				match self.tool {
					Tool::Annot(AnnotationKind::Ellipse) => {
						painter.add(Shape::closed_line(geom::ellipse_points(r.min, r.max, 64), stroke));
					}
					Tool::Annot(AnnotationKind::Line | AnnotationKind::Arrow) => {
						painter.line_segment([doc.to_screen(*start), c], stroke);
						if self.tool == Tool::Annot(AnnotationKind::Arrow) {
							painter.add(geom::arrow_head(doc.to_screen(*start), c, 10.0, accent));
						}
					}
					_ => {
						painter.rect_filled(r, 0.0, with_alpha(accent, 0.08));
						painter.rect_stroke(r, 0.0, stroke, StrokeKind::Middle);
						if self.tool == Tool::RectArea {
							let a = *start;
							let b = doc.to_page(c);
							let pts = vec![a, Point { x: b.x, y: a.y }, b, Point { x: a.x, y: b.y }];
							if let Some(t) = self.live_area(doc, &pts) {
								pill(painter, c + Vec2::new(16.0, 18.0), &t, accent, true);
							}
						}
					}
				}
			}
		}
		if let Some(Drag::Ink { points }) = &self.drag {
			let pts: Vec<Pos2> = points.iter().map(|q| doc.to_screen(*q)).collect();
			painter.add(Shape::line(pts, Stroke::new((self.annot_style.width as f32 * doc.zoom).max(1.5), hex(&self.annot_style.stroke))));
		}
		if let Some((a, b)) = self.callout_box {
			let r = Rect::from_two_pos(doc.to_screen(a), doc.to_screen(b));
			painter.rect_stroke(r, 0.0, stroke, StrokeKind::Middle);
			if let Some(c) = cursor {
				let from = Pos2::new(c.x.clamp(r.left(), r.right()), c.y.clamp(r.top(), r.bottom()));
				painter.line_segment([from, c], stroke);
			}
		}
		if let Some(Drag::Marquee { start }) = &self.drag {
			if let Some(c) = hover {
				let r = Rect::from_two_pos(*start, c);
				painter.rect_filled(r, 0.0, with_alpha(p.selection, 0.08));
				painter.rect_stroke(r, 0.0, Stroke::new(1.0, p.selection), StrokeKind::Middle);
			}
		}
		// Snap marker: ■ endpoint, ▲ midpoint, ✕ intersection, ◆ on line.
		if let Some(s) = &self.hover_snap {
			let c = doc.to_screen(s.point);
			let col = Color32::from_rgb(0x10, 0xB9, 0x81);
			let st = Stroke::new(2.0, col);
			match s.kind {
				SnapKind::Endpoint => {
					painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(10.0)), 0.0, st, StrokeKind::Middle);
				}
				SnapKind::Midpoint => {
					painter.add(Shape::closed_line(vec![c + Vec2::new(0.0, -6.0), c + Vec2::new(6.0, 5.0), c + Vec2::new(-6.0, 5.0)], st));
				}
				SnapKind::Intersection => {
					painter.line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], st);
					painter.line_segment([c + Vec2::new(-5.0, 5.0), c + Vec2::new(5.0, -5.0)], st);
				}
				SnapKind::OnLine => {
					painter.add(Shape::closed_line(vec![c + Vec2::new(0.0, -6.0), c + Vec2::new(6.0, 0.0), c + Vec2::new(0.0, 6.0), c + Vec2::new(-6.0, 0.0)], st));
				}
			}
		}
	}

	fn live_quantity(&self, doc: &OpenDoc, pts: &[Point]) -> Option<String> {
		let d = doc.doc.lock();
		let (w, h) = doc.page_size(doc.page);
		let kind = match self.tool {
			Tool::Length | Tool::Curve | Tool::Trace | Tool::Perimeter | Tool::Calibrate => MeasurementType::Length,
			Tool::Area | Tool::Volume | Tool::Cutout => MeasurementType::Area,
			Tool::Angle => MeasurementType::Angle,
			_ => return None,
		};
		let mut m = Measurement::new("live", doc.page, kind, pts.to_vec());
		m.closed = self.tool == Tool::Perimeter;
		if self.tool == Tool::Calibrate {
			let px = takeoff_core::geometry::polyline_length_px(pts, w, h);
			return Some(format!("{:.1} pt on paper", px));
		}
		let q = measurement_quantity(&m, d.data.scale_at(doc.page, pts[0]), w, h)?;
		Some(format_quantity(q.value, &q.unit, &d.data.display))
	}

	fn live_area(&self, doc: &OpenDoc, pts: &[Point]) -> Option<String> {
		let d = doc.doc.lock();
		let (w, h) = doc.page_size(doc.page);
		let m = Measurement::new("live", doc.page, MeasurementType::Area, pts.to_vec());
		let q = measurement_quantity(&m, d.data.scale_at(doc.page, pts[0]), w, h)?;
		Some(format_quantity(q.value, &q.unit, &d.data.display))
	}

	// ── Hit testing ─────────────────────────────────────────────────────

	/// Topmost item under the screen point.
	pub fn hit(&self, doc: &OpenDoc, pos: Pos2) -> Option<String> {
		let d = doc.doc.lock();
		let data = &d.data;
		let page = doc.page;
		let hidden: HashSet<&str> = data.categories.iter().filter(|c| c.hidden).map(|c| c.id.as_str()).collect();
		let vis = |c: &Option<String>| !c.as_deref().is_some_and(|c| hidden.contains(c));
		for m in data.marks.iter().rev().filter(|m| m.page == page && vis(&m.category_id)) {
			let r = Rect::from_two_pos(doc.to_screen(Point { x: m.x, y: m.y }), doc.to_screen(Point { x: m.x + m.w, y: m.y + m.h }));
			if r.expand(HIT_PX / 2.0).contains(pos) {
				return Some(m.id.clone());
			}
		}
		for m in data.measurements.iter().rev().filter(|m| m.page == page && vis(&m.category_id)) {
			let pts: Vec<Pos2> = m.points.iter().map(|q| doc.to_screen(*q)).collect();
			let closed = m.kind == MeasurementType::Area || m.closed;
			if geom::dist_to_polyline(pos, &pts, closed) <= HIT_PX || (m.kind == MeasurementType::Area && geom::point_in_poly(pos, &pts)) {
				return Some(m.id.clone());
			}
		}
		for a in data.annotations.iter().rev().filter(|a| a.page == page) {
			let pts: Vec<Pos2> = a.points.iter().map(|q| doc.to_screen(*q)).collect();
			let inside = match a.kind {
				AnnotationKind::Line | AnnotationKind::Arrow | AnnotationKind::Polyline | AnnotationKind::Ink => geom::dist_to_polyline(pos, &pts, false) <= HIT_PX,
				AnnotationKind::Polygon | AnnotationKind::Cloud => geom::point_in_poly(pos, &pts) || geom::dist_to_polyline(pos, &pts, true) <= HIT_PX,
				_ if pts.len() >= 2 => {
					let r = Rect::from_two_pos(pts[0], pts[1]).expand(HIT_PX / 2.0);
					r.contains(pos) || (a.kind == AnnotationKind::Callout && pts.len() >= 3 && (pos - pts[2]).length() < HIT_PX * 2.0)
				}
				_ => false,
			};
			if inside {
				return Some(a.id.clone());
			}
		}
		None
	}

	/// A vertex or resize handle of a selected item under the cursor.
	fn handle_at(&self, doc: &OpenDoc, pos: Pos2) -> Option<Drag> {
		if doc.selection.len() != 1 {
			return None;
		}
		let id = doc.selection.iter().next()?.clone();
		let d = doc.doc.lock();
		let near = |q: &Point| (doc.to_screen(*q) - pos).length() <= 7.0;
		if let Some(m) = d.data.measurements.iter().find(|m| m.id == id && m.page == doc.page) {
			if let Some(idx) = m.points.iter().position(near) {
				return Some(Drag::Vertex { id, idx, cutout: None });
			}
			for (ci, c) in m.cutouts.iter().enumerate() {
				if let Some(idx) = c.iter().position(near) {
					return Some(Drag::Vertex { id, idx, cutout: Some(ci) });
				}
			}
		}
		if let Some(a) = d.data.annotations.iter().find(|a| a.id == id && a.page == doc.page) {
			let rect_like = matches!(a.kind, AnnotationKind::Rectangle | AnnotationKind::Ellipse | AnnotationKind::Text | AnnotationKind::Stamp | AnnotationKind::Highlight | AnnotationKind::Callout);
			if let Some(idx) = a.points.iter().position(near) {
				return Some(if rect_like && idx < 2 { Drag::Resize { id, corner: idx } } else { Drag::Vertex { id, idx, cutout: None } });
			}
		}
		if let Some(m) = d.data.marks.iter().find(|m| m.id == id && m.page == doc.page) {
			let corners = [Point { x: m.x, y: m.y }, Point { x: m.x + m.w, y: m.y + m.h }];
			if let Some(c) = corners.iter().position(near) {
				return Some(Drag::Resize { id, corner: c });
			}
		}
		None
	}

	// ── Interaction ─────────────────────────────────────────────────────

	fn snapped(&mut self, doc: &mut OpenDoc, pos: Pos2, mods: Modifiers, snap_on: bool) -> Point {
		let mut pt = doc.to_page(pos);
		self.hover_snap = None;
		if snap_on && !mods.alt && self.tool.snaps() {
			if let Some(idx) = doc.ensure_snap() {
				if let Some(s) = idx.nearest(pt, (SNAP_PX / doc.zoom) as f64) {
					self.hover_snap = Some(s);
					pt = s.point;
				}
			}
		}
		if mods.shift {
			let from = self.points.last().copied().or(match &self.drag {
				Some(Drag::Shape { start }) => Some(*start),
				_ => None,
			});
			if let Some(f) = from {
				let (w, h) = doc.page_size(doc.page);
				pt = takeoff_core::snap::ortho(f, pt, w, h);
				self.hover_snap = None;
			}
		}
		Point { x: pt.x.clamp(0.0, 1.0), y: pt.y.clamp(0.0, 1.0) }
	}

	fn interact(&mut self, ui: &mut egui::Ui, resp: &egui::Response, doc: &mut OpenDoc, p: &Palette, snap_on: bool) {
		if matches!(self.drag, Some(Drag::Pan)) {
			return;
		}
		let (mods, hover, double, primary_pressed) = ui.input(|i| (i.modifiers, i.pointer.hover_pos(), i.pointer.button_double_clicked(egui::PointerButton::Primary) || i.pointer.button_triple_clicked(egui::PointerButton::Primary), i.pointer.primary_pressed()));
		let space = ui.input(|i| i.key_down(Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
		if space || self.tool == Tool::Pan {
			return;
		}
		let in_canvas = hover.is_some_and(|h| doc.canvas_rect.contains(h));
		self.cursor_page = hover.filter(|_| in_canvas).map(|h| doc.to_page(h));
		self.cursor_on = doc.page;
		let pos = hover.unwrap_or(doc.canvas_rect.center());
		// Continuous / spreads: a shape stays on the page it was begun on (clicks
		// off it don't add to it), and a new one starts on a page, not in a gap.
		let open_shape = !self.points.is_empty() || self.callout_box.is_some();
		let off = doc.layout.multi() && if open_shape { !doc.page_rect().expand(doc.arr.gap * doc.zoom / 2.0).contains(pos) } else { self.tool != Tool::Select && self.drag.is_none() && doc.page_at(pos).is_none() };
		if off && open_shape && resp.clicked() {
			self.requests.push(CanvasRequest::Status(format!("This shape is on page {}: finish it there (Enter), or Esc to cancel.", doc.page)));
		}
		let in_canvas = in_canvas && !off;
		if !in_canvas && self.drag.is_none() {
			self.hover_snap = None;
		}
		let pt = if in_canvas || self.drag.is_some() { self.snapped(doc, pos, mods, snap_on) } else { doc.to_page(pos) };
		// Drags start where the button went down, not where egui noticed the drag.
		let press = ui.input(|i| i.pointer.press_origin());
		let start_pt = match press {
			Some(o) if resp.drag_started() => {
				let saved = self.hover_snap;
				let p0 = self.snapped(doc, o, mods, snap_on);
				self.hover_snap = saved;
				p0
			}
			_ => pt,
		};

		// Keyboard for in-progress shapes.
		if self.tool.is_multi_point() && !self.points.is_empty() && !ui.ctx().egui_wants_keyboard_input() {
			if ui.input(|i| i.key_pressed(Key::Enter)) {
				self.finish_points(doc);
			}
			if ui.input(|i| i.key_pressed(Key::Backspace)) {
				self.points.pop();
			}
		}

		match self.tool {
			Tool::Select => self.select_tool(ui, resp, doc, pos, pt, mods, double, primary_pressed),
			Tool::Count => {
				if resp.clicked() && in_canvas {
					let cat = self.category_for(doc, CategoryKind::Count);
					let (w, h) = doc.page_size(doc.page);
					let (mw, mh) = (18.0 / w, 18.0 / h);
					let meta = doc.doc.lock().new_meta();
					let mark = Mark {
						id: new_id("m"),
						category_id: Some(cat),
						page: doc.page,
						x: (pt.x - mw / 2.0).clamp(0.0, 1.0 - mw),
						y: (pt.y - mh / 2.0).clamp(0.0, 1.0 - mh),
						w: mw,
						h: mh,
						source: Source::User,
						status: MarkStatus::Confirmed,
						tile: None,
						note: None,
						fields: None,
						meta,
					};
					doc.doc.lock().edit("Count", |d| d.marks.push(mark));
				}
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
			}
			t if t.is_multi_point() => {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
				if double && !self.points.is_empty() {
					// The double-click's first click already added a point at the same spot.
					if self.points.len() >= 2 {
						let (a, b) = (self.points[self.points.len() - 1], self.points[self.points.len() - 2]);
						if (doc.to_screen(a) - doc.to_screen(b)).length() < 4.0 {
							self.points.pop();
						}
					}
					self.finish_points(doc);
				} else if resp.clicked() && in_canvas {
					if self.tool == Tool::Cutout && self.cutout_target(doc).is_none() {
						self.requests.push(CanvasRequest::Status("Select an area measurement first, then draw its cutout.".into()));
					} else {
						self.points.push(pt);
						let needed = match self.tool {
							Tool::Angle => 3,
							Tool::Calibrate => 2,
							_ => usize::MAX,
						};
						if self.points.len() >= needed {
							self.finish_points(doc);
						}
					}
				}
			}
			Tool::Fill => {
				ui.ctx().set_cursor_icon(if self.fill_job.is_some() { CursorIcon::Progress } else { CursorIcon::Crosshair });
				if resp.clicked() && in_canvas && self.fill_job.is_none() {
					let raw = doc.to_page(pos);
					// Shift-click: another piece of the last fill, filled together with it.
					let grow = self.last_fill.clone().filter(|(id, page, _)| mods.shift && *page == doc.page && doc.doc.lock().data.measurements.iter().any(|m| &m.id == id));
					let at = match &grow {
						Some((_, _, at)) => at.iter().copied().chain([raw]).collect(),
						None => vec![raw],
					};
					let token = doc.request_fill(at.clone());
					self.fill_job = Some(FillJob { token, page: doc.page, at, grow: grow.map(|g| g.0) });
				}
			}
			Tool::FollowLine => {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
				if resp.clicked() && in_canvas {
					let raw = doc.to_page(pos);
					match doc.ensure_snap() {
						Some(idx) if !idx.is_empty() => match idx.nearest(raw, (SNAP_PX / doc.zoom) as f64 * 1.5) {
							Some(s) => {
								let run = idx.follow(s.segment, 0.75);
								if run.len() >= 2 {
									let cat = self.category_for(doc, CategoryKind::Length);
									let mut m = Measurement::new(new_id("ms"), doc.page, MeasurementType::Length, run);
									m.category_id = Some(cat);
									m.meta = doc.doc.lock().new_meta();
									m.label = Some("Followed line".into());
									let id = m.id.clone();
									doc.doc.lock().edit("Follow line", |d| d.measurements.push(m));
									self.flash = Some(([id].into(), Instant::now()));
								}
							}
							None => self.requests.push(CanvasRequest::Status("No drawing line there. Click right on a line.".into())),
						},
						Some(_) => self.requests.push(CanvasRequest::Status("This sheet has no vector lines (scanned?). Use Length instead.".into())),
						None => self.requests.push(CanvasRequest::Status("Reading the drawing's lines…".into())),
					}
				}
			}
			Tool::RectArea | Tool::Viewport | Tool::Annot(AnnotationKind::Rectangle | AnnotationKind::Ellipse | AnnotationKind::Highlight | AnnotationKind::Text | AnnotationKind::Stamp | AnnotationKind::Line | AnnotationKind::Arrow) => {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
				if resp.drag_started() && in_canvas {
					self.drag = Some(Drag::Shape { start: start_pt });
				}
				if resp.drag_stopped() {
					if let Some(Drag::Shape { start }) = self.drag.take() {
						self.finish_shape(doc, start, pt);
					}
				} else if resp.clicked() && in_canvas && matches!(self.tool, Tool::Annot(AnnotationKind::Text | AnnotationKind::Stamp)) {
					// Click without drag: a default-size box.
					let (w, h) = doc.page_size(doc.page);
					let b = Point { x: (pt.x + 160.0 / w).min(1.0), y: (pt.y + 40.0 / h).min(1.0) };
					self.finish_shape(doc, pt, b);
				}
			}
			Tool::Annot(AnnotationKind::Callout) => {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
				if let Some((a, b)) = self.callout_box {
					if resp.clicked() && in_canvas {
						self.callout_box = None;
						let ann = self.new_annotation(doc, AnnotationKind::Callout, vec![a, b, pt]);
						let id = ann.id.clone();
						doc.doc.lock().edit("Callout", |d| d.annotations.push(ann));
						self.start_text_edit(doc, &id);
					}
				} else {
					if resp.drag_started() && in_canvas {
						self.drag = Some(Drag::Shape { start: start_pt });
					}
					if resp.drag_stopped() {
						if let Some(Drag::Shape { start }) = self.drag.take() {
							self.callout_box = Some((start, pt));
						}
					}
				}
			}
			Tool::Annot(AnnotationKind::Ink) => {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
				if resp.drag_started() && in_canvas {
					self.drag = Some(Drag::Ink { points: vec![start_pt, pt] });
				}
				if let Some(Drag::Ink { points }) = &mut self.drag {
					if resp.dragged() && points.last().is_none_or(|l| (doc.to_screen(*l) - pos).length() > 2.0) {
						points.push(doc.to_page(pos));
					}
				}
				if resp.drag_stopped() {
					if let Some(Drag::Ink { points }) = self.drag.take() {
						if points.len() >= 2 {
							let ann = self.new_annotation(doc, AnnotationKind::Ink, points);
							doc.doc.lock().edit("Pen", |d| d.annotations.push(ann));
						}
					}
				}
			}
			_ => {}
		}
		let _ = p;
	}

	#[allow(clippy::too_many_arguments)]
	fn select_tool(&mut self, ui: &mut egui::Ui, resp: &egui::Response, doc: &mut OpenDoc, pos: Pos2, pt: Point, mods: Modifiers, double: bool, primary_pressed: bool) {
		let in_canvas = doc.canvas_rect.contains(pos);
		let cmd = mods.command || mods.ctrl;
		if primary_pressed && in_canvas && self.drag.is_none() && self.editing_text.is_none() {
			if let Some(h) = self.handle_at(doc, pos) {
				doc.doc.lock().begin_gesture("Edit shape");
				self.drag = Some(h);
			}
		}
		if resp.drag_started() && self.drag.is_none() && in_canvas {
			let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
			let raw = doc.to_page(origin);
			match self.hit(doc, origin) {
				Some(id) => {
					if !doc.selection.contains(&id) {
						if !mods.shift && !cmd {
							doc.selection.clear();
						}
						doc.selection.insert(id);
					}
					doc.doc.lock().begin_gesture("Move");
					self.drag = Some(Drag::Move { last: raw, moved: false });
				}
				// Empty sheet: box-select (Shift/⌘ adds to the selection). Pan with
				// two fingers, Space-drag or the wheel button.
				None => {
					if !mods.shift && !cmd {
						doc.selection.clear();
					}
					self.drag = Some(Drag::Marquee { start: origin });
				}
			}
		}
		match &mut self.drag {
			Some(Drag::Move { last, moved }) if resp.dragged() => {
				let raw = doc.to_page(pos);
				let (dx, dy) = (raw.x - last.x, raw.y - last.y);
				if dx != 0.0 || dy != 0.0 {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Move", |d| d.translate_ids(&ids, dx, dy));
					*last = raw;
					*moved = true;
				}
				ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
			}
			Some(Drag::Vertex { id, idx, cutout }) if resp.dragged() || ui.input(|i| i.pointer.primary_down()) => {
				let (id, idx, cutout) = (id.clone(), *idx, *cutout);
				doc.doc.lock().edit("Edit shape", |d| {
					if let Some(m) = d.measurements.iter_mut().find(|m| m.id == id) {
						let target = match cutout {
							Some(ci) => m.cutouts.get_mut(ci).and_then(|c| c.get_mut(idx)),
							None => m.points.get_mut(idx),
						};
						if let Some(v) = target {
							*v = pt;
						}
						m.meta.modified_at = Some(now_ms());
					} else if let Some(a) = d.annotations.iter_mut().find(|a| a.id == id) {
						if let Some(v) = a.points.get_mut(idx) {
							*v = pt;
						}
						a.meta.modified_at = Some(now_ms());
					}
				});
			}
			Some(Drag::Resize { id, corner }) if resp.dragged() || ui.input(|i| i.pointer.primary_down()) => {
				let (id, corner) = (id.clone(), *corner);
				doc.doc.lock().edit("Resize", |d| {
					if let Some(a) = d.annotations.iter_mut().find(|a| a.id == id) {
						if let Some(v) = a.points.get_mut(corner) {
							*v = pt;
						}
					} else if let Some(m) = d.marks.iter_mut().find(|m| m.id == id) {
						let (x1, y1) = (m.x + m.w, m.y + m.h);
						if corner == 0 {
							m.x = pt.x.min(x1 - 0.001);
							m.y = pt.y.min(y1 - 0.001);
							m.w = x1 - m.x;
							m.h = y1 - m.y;
						} else {
							m.w = (pt.x - m.x).max(0.001);
							m.h = (pt.y - m.y).max(0.001);
						}
					}
				});
			}
			_ => {}
		}
		let released = ui.input(|i| i.pointer.primary_released());
		if released || resp.drag_stopped() {
			match self.drag.take() {
				Some(Drag::Marquee { start }) => {
					let r = Rect::from_two_pos(start, pos);
					if r.width() > 3.0 || r.height() > 3.0 {
						let ids = self.items_in(doc, r);
						doc.selection.extend(ids);
					}
				}
				Some(Drag::Move { .. }) | Some(Drag::Vertex { .. }) | Some(Drag::Resize { .. }) => {
					doc.doc.lock().end_gesture();
				}
				other => self.drag = other,
			}
		}
		if resp.clicked() && in_canvas {
			match self.hit(doc, pos) {
				Some(id) => {
					if mods.shift || cmd {
						if !doc.selection.remove(&id) {
							doc.selection.insert(id);
						}
					} else {
						doc.selection.clear();
						doc.selection.insert(id);
					}
				}
				None => {
					if !mods.shift && !cmd {
						doc.selection.clear();
					}
					// The map: a callout goes where it points; any other area shows in the Map tab.
					if self.map_links && !mods.shift && !cmd {
						match doc.map_area_at(pt) {
							Some(a) => {
								let is_callout = doc.map.as_ref().and_then(|m| m.node(&a)).is_some_and(|n| matches!(n, docmap::NodeRef::Part(_, x) if x.kind == docmap::PartKind::Callout));
								match doc.map_targets(&a).first().cloned() {
									Some(t) if is_callout => {
										doc.map_peek(&a);
										doc.map_go(&t);
									}
									_ => doc.map_peek(&a),
								}
							}
							None => doc.nav.focus = None,
						}
					}
				}
			}
		}
		if double && in_canvas {
			if let Some(id) = self.hit(doc, pos) {
				let is_text = doc.doc.lock().data.annotations.iter().any(|a| a.id == id && matches!(a.kind, AnnotationKind::Text | AnnotationKind::Callout | AnnotationKind::Stamp));
				if is_text {
					self.start_text_edit(doc, &id);
				}
			}
		}
		// Hover cursor feedback.
		if self.drag.is_none() && in_canvas {
			if self.handle_at(doc, pos).is_some() {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
			} else if self.hit(doc, pos).is_some() {
				ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
			}
		}
	}

	fn items_in(&self, doc: &OpenDoc, r: Rect) -> Vec<String> {
		let d = doc.doc.lock();
		let page = doc.page;
		let inside = |q: &Point| r.contains(doc.to_screen(*q));
		let mut out = Vec::new();
		for m in d.data.marks.iter().filter(|m| m.page == page) {
			if inside(&Point { x: m.x + m.w / 2.0, y: m.y + m.h / 2.0 }) {
				out.push(m.id.clone());
			}
		}
		for m in d.data.measurements.iter().filter(|m| m.page == page) {
			if m.points.iter().all(inside) {
				out.push(m.id.clone());
			}
		}
		for a in d.data.annotations.iter().filter(|a| a.page == page) {
			if a.points.iter().all(inside) {
				out.push(a.id.clone());
			}
		}
		out
	}

	fn cutout_target(&self, doc: &OpenDoc) -> Option<String> {
		let d = doc.doc.lock();
		doc.selection.iter().find(|id| d.data.measurements.iter().any(|m| &m.id == *id && m.kind == MeasurementType::Area)).cloned()
	}

	fn new_annotation(&self, doc: &OpenDoc, kind: AnnotationKind, points: Vec<Point>) -> Annotation {
		let mut style = self.annot_style.clone();
		match kind {
			AnnotationKind::Highlight => style.stroke = self.highlight_color.clone(),
			AnnotationKind::Text => style.width = 0.0,
			_ => {}
		}
		let text = match kind {
			AnnotationKind::Stamp => Some(self.stamp_text.clone()),
			_ => None,
		};
		Annotation { id: new_id("an"), page: doc.page, kind, points, text, style, subject: None, note: None, source: None, meta: doc.doc.lock().new_meta() }
	}

	fn finish_shape(&mut self, doc: &mut OpenDoc, a: Point, b: Point) {
		let (w, h) = doc.page_size(doc.page);
		let tiny = ((a.x - b.x) * w).abs() < 2.0 && ((a.y - b.y) * h).abs() < 2.0;
		if tiny && !matches!(self.tool, Tool::Annot(AnnotationKind::Text | AnnotationKind::Stamp)) {
			return;
		}
		let (tl, br) = (Point { x: a.x.min(b.x), y: a.y.min(b.y) }, Point { x: a.x.max(b.x), y: a.y.max(b.y) });
		match self.tool {
			Tool::RectArea => {
				let cat = self.category_for(doc, CategoryKind::Area);
				let mut m = Measurement::new(new_id("ms"), doc.page, MeasurementType::Area, vec![tl, Point { x: br.x, y: tl.y }, br, Point { x: tl.x, y: br.y }]);
				m.category_id = Some(cat);
				m.meta = doc.doc.lock().new_meta();
				doc.doc.lock().edit("Area", |d| d.measurements.push(m));
			}
			Tool::Viewport => {
				self.requests.push(CanvasRequest::Viewport { page: doc.page, rect: (tl.x, tl.y, br.x - tl.x, br.y - tl.y) });
			}
			Tool::Annot(kind @ (AnnotationKind::Line | AnnotationKind::Arrow)) => {
				let ann = self.new_annotation(doc, kind, vec![a, b]);
				doc.doc.lock().edit(kind.label(), |d| d.annotations.push(ann));
			}
			Tool::Annot(kind) => {
				let ann = self.new_annotation(doc, kind, vec![tl, br]);
				let id = ann.id.clone();
				doc.doc.lock().edit(kind.label(), |d| d.annotations.push(ann));
				if kind == AnnotationKind::Text {
					self.start_text_edit(doc, &id);
				}
			}
			_ => {}
		}
	}

	/// Whether the tool draws a run the curve mode applies to.
	fn curves(&self) -> bool {
		matches!(self.tool, Tool::Length | Tool::Curve | Tool::Trace | Tool::CurveMarkup | Tool::Perimeter | Tool::Area | Tool::Volume | Tool::Cutout)
	}

	/// How this tool joins its points: the Curve and Trace tools always
	/// curve; the others as the curve setting says.
	pub fn curve_mode(&self) -> CurveMode {
		match self.tool {
			Tool::Curve | Tool::CurveMarkup => CurveMode::Smooth,
			Tool::Trace => CurveMode::Follow,
			_ => self.curve,
		}
	}

	/// The points joined as the curve mode says (`closed`: back to the
	/// first), and whether a span is still being traced in the picture.
	fn shape(&self, doc: &mut OpenDoc, pts: &[Point], closed: bool) -> (Vec<Point>, bool) {
		let (w, h) = doc.page_size(doc.page);
		match self.curve_mode() {
			CurveMode::Straight => (pts.to_vec(), false),
			CurveMode::Smooth => (takeoff_core::geometry::smooth_path(pts, closed, w, h), false),
			CurveMode::Follow => {
				let idx = doc.ensure_snap();
				let mut spans: Vec<(Point, Point)> = pts.windows(2).map(|p| (p[0], p[1])).collect();
				if closed && pts.len() > 2 {
					spans.push((pts[pts.len() - 1], pts[0]));
				}
				let mut out: Vec<Point> = Vec::new();
				let mut pending = false;
				for (a, b) in spans {
					// The drawing's vector lines; else the line in the page's picture (scans, photos).
					let run = match idx.as_ref().filter(|i| !i.is_empty()).and_then(|i| i.track(a, b)) {
						Some(r) => r,
						None => match doc.trace_raster(a, b) {
							Some(Some(r)) => r,
							Some(None) => vec![a, b],
							None => {
								pending = true;
								vec![a, b]
							}
						},
					};
					let skip = usize::from(!out.is_empty());
					out.extend(run.into_iter().skip(skip));
				}
				if closed && out.len() > 2 && out.first() == out.last() {
					out.pop();
				}
				(out, pending)
			}
		}
	}

	/// Along-the-line preview: trace the spans placed so far, once each.
	fn update_follow_cache(&mut self, doc: &mut OpenDoc) {
		if self.curve_mode() != CurveMode::Follow || !self.curves() {
			self.follow_cache = None;
			return;
		}
		// The page's line work loads in the background: ask early, and trace once it's here.
		if doc.ensure_snap().is_none() {
			self.follow_cache = None;
			return;
		}
		// Traced already, unless a span was still being traced in the picture.
		if self.points.len() < 2 || self.follow_cache.as_ref().is_some_and(|(k, _, pending)| *k == self.points && !pending) {
			return;
		}
		let pts = self.points.clone();
		let (path, pending) = self.shape(doc, &pts, false);
		self.follow_cache = Some((pts, path, pending));
	}

	pub fn finish_points(&mut self, doc: &mut OpenDoc) {
		let raw = std::mem::take(&mut self.points);
		self.follow_cache = None;
		let page = doc.page;
		let pts = if self.curves() && raw.len() >= 2 {
			let closed = matches!(self.tool, Tool::Perimeter | Tool::Area | Tool::Volume | Tool::Cutout);
			self.shape(doc, &raw, closed).0
		} else {
			raw
		};
		match self.tool {
			Tool::Length | Tool::Curve | Tool::Trace | Tool::Perimeter if pts.len() >= 2 => {
				let cat = self.category_for(doc, CategoryKind::Length);
				let mut m = Measurement::new(new_id("ms"), page, MeasurementType::Length, pts);
				m.category_id = Some(cat);
				m.closed = self.tool == Tool::Perimeter;
				m.meta = doc.doc.lock().new_meta();
				doc.doc.lock().edit(self.tool.label(), |d| d.measurements.push(m));
			}
			Tool::Area | Tool::Volume if pts.len() >= 3 => {
				let cat = self.category_for(doc, CategoryKind::Area);
				let mut m = Measurement::new(new_id("ms"), page, MeasurementType::Area, pts);
				m.category_id = Some(cat);
				m.meta = doc.doc.lock().new_meta();
				let id = m.id.clone();
				doc.doc.lock().edit(self.tool.label(), |d| d.measurements.push(m));
				if self.tool == Tool::Volume {
					self.requests.push(CanvasRequest::Depth { id });
				}
			}
			Tool::Cutout if pts.len() >= 3 => {
				if let Some(target) = self.cutout_target(doc) {
					doc.doc.lock().edit("Cutout", |d| {
						if let Some(m) = d.measurements.iter_mut().find(|m| m.id == target) {
							m.cutouts.push(pts);
						}
					});
				}
			}
			Tool::Angle if pts.len() == 3 => {
				let mut m = Measurement::new(new_id("ms"), page, MeasurementType::Angle, pts);
				m.meta = doc.doc.lock().new_meta();
				doc.doc.lock().edit("Angle", |d| d.measurements.push(m));
			}
			Tool::Calibrate if pts.len() == 2 => {
				self.requests.push(CanvasRequest::Calibrate { page, p1: pts[0], p2: pts[1] });
			}
			Tool::CurveMarkup if pts.len() >= 2 => {
				let ann = self.new_annotation(doc, AnnotationKind::Polyline, pts);
				doc.doc.lock().edit("Curve", |d| d.annotations.push(ann));
			}
			Tool::Annot(kind @ (AnnotationKind::Polyline | AnnotationKind::Polygon | AnnotationKind::Cloud)) => {
				let min = if kind == AnnotationKind::Polyline { 2 } else { 3 };
				if pts.len() >= min {
					let ann = self.new_annotation(doc, kind, pts);
					doc.doc.lock().edit(kind.label(), |d| d.annotations.push(ann));
				}
			}
			_ => {}
		}
	}

	// ── Text editing ────────────────────────────────────────────────────

	pub fn start_text_edit(&mut self, doc: &OpenDoc, id: &str) {
		self.text_buffer = doc.doc.lock().data.annotations.iter().find(|a| a.id == id).and_then(|a| a.text.clone()).unwrap_or_default();
		self.editing_text = Some(id.to_string());
	}

	fn commit_text(&mut self, doc: &OpenDoc) {
		if let Some(id) = self.editing_text.take() {
			let text = std::mem::take(&mut self.text_buffer);
			doc.doc.lock().edit("Edit text", |d| {
				if let Some(a) = d.annotations.iter_mut().find(|a| a.id == id) {
					a.text = (!text.trim().is_empty()).then_some(text);
					a.meta.modified_at = Some(now_ms());
				}
			});
		}
	}

	fn text_editor(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc) {
		let Some(id) = self.editing_text.clone() else { return };
		let (rect, size) = {
			let d = doc.doc.lock();
			let Some(a) = d.data.annotations.iter().find(|a| a.id == id) else {
				self.editing_text = None;
				return;
			};
			if a.page != doc.page || a.points.len() < 2 {
				drop(d);
				self.commit_text(doc);
				return;
			}
			(Rect::from_two_pos(doc.to_screen(a.points[0]), doc.to_screen(a.points[1])), (a.style.font_size as f32 * doc.zoom).clamp(8.0, 48.0))
		};
		let mut done = false;
		egui::Area::new(egui::Id::new("text-edit")).fixed_pos(rect.min).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
			ui.set_width(rect.width().max(80.0));
			let te = egui::TextEdit::multiline(&mut self.text_buffer)
				.font(egui::FontId::proportional(size))
				.desired_width(rect.width().max(80.0) - 8.0)
				.desired_rows(((rect.height() / (size * 1.3)) as usize).max(1))
				.hint_text("Type a note…");
			let r = ui.add(te);
			r.request_focus();
			if ui.input(|i| i.key_pressed(Key::Escape)) || (r.lost_focus() && !ui.input(|i| i.key_pressed(Key::Enter))) {
				done = true;
			}
		});
		if done {
			self.commit_text(doc);
		}
	}

	fn paint_annotation_text(&self, ui: &egui::Ui, doc: &OpenDoc) {
		let painter = ui.painter_at(doc.canvas_rect);
		let d = doc.doc.lock();
		for a in d.data.annotations.iter().filter(|a| a.page == doc.page && a.points.len() >= 2) {
			if Some(&a.id) == self.editing_text.as_ref() {
				continue;
			}
			let Some(text) = a.text.as_deref().filter(|t| !t.is_empty()) else { continue };
			let r = Rect::from_two_pos(doc.to_screen(a.points[0]), doc.to_screen(a.points[1]));
			let size = (a.style.font_size as f32 * doc.zoom).clamp(4.0, 200.0);
			if size < 4.5 {
				continue;
			}
			let (font, color) = if a.kind == AnnotationKind::Stamp { (theme::bold(size * 1.2), hex(&a.style.stroke)) } else { (egui::FontId::proportional(size), Color32::from_rgb(20, 24, 32)) };
			let galley = painter.layout(text.to_string(), font, color, (r.width() - size * 0.6).max(10.0));
			let at = if a.kind == AnnotationKind::Stamp { r.center() - galley.size() / 2.0 } else { r.min + Vec2::splat(size * 0.3) };
			painter.with_clip_rect(r.intersect(doc.canvas_rect)).galley(at, galley, color);
		}
	}

	fn context_menu(&mut self, resp: &egui::Response, doc: &mut OpenDoc) {
		if resp.secondary_clicked() {
			if let Some(pos) = resp.interact_pointer_pos() {
				if let Some(id) = self.hit(doc, pos) {
					if !doc.selection.contains(&id) {
						doc.selection.clear();
						doc.selection.insert(id);
					}
				}
			}
		}
		resp.context_menu(|ui| {
			let n = doc.selection.len();
			if n > 0 {
				let any_pending = {
					let d = doc.doc.lock();
					d.data.marks.iter().any(|m| doc.selection.contains(&m.id) && m.status == MarkStatus::Pending)
				};
				if any_pending && ui.button(format!("{}  Confirm", crate::icons::CHECK)).clicked() {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Confirm", |d| {
						for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.status = MarkStatus::Confirmed;
						}
					});
					ui.close();
				}
				if ui.button(format!("{}  Copy", crate::icons::COPY)).clicked() {
					self.copy(doc);
					ui.close();
				}
				if ui.button(format!("{}  Duplicate", crate::icons::COPY)).clicked() {
					self.copy(doc);
					self.paste(doc, true);
					ui.close();
				}
				let cats: Vec<(String, String, CategoryKind)> = doc.doc.lock().data.categories.iter().map(|c| (c.id.clone(), c.name.clone(), c.kind())).collect();
				if !cats.is_empty() {
					ui.menu_button(format!("{}  Move to item", crate::icons::TAG), |ui| {
						for (id, name, kind) in &cats {
							if ui.button(name).clicked() {
								let ids = doc.selection.clone();
								let (id, kind) = (id.clone(), *kind);
								doc.doc.lock().edit("Change item", |d| {
									for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
										if kind == CategoryKind::Count {
											m.category_id = Some(id.clone());
										}
									}
									for m in d.measurements.iter_mut().filter(|m| ids.contains(&m.id)) {
										let fits = matches!((kind, m.kind), (CategoryKind::Length, MeasurementType::Length) | (CategoryKind::Area, MeasurementType::Area));
										if fits {
											m.category_id = Some(id.clone());
										}
									}
								});
								ui.close();
							}
						}
					});
				}
				let locked = {
					let d = doc.doc.lock();
					d.data.marks.iter().filter(|m| doc.selection.contains(&m.id)).any(|m| m.meta.locked)
						|| d.data.measurements.iter().filter(|m| doc.selection.contains(&m.id)).any(|m| m.meta.locked)
						|| d.data.annotations.iter().filter(|a| doc.selection.contains(&a.id)).any(|a| a.meta.locked)
				};
				if ui.button(if locked { format!("{}  Unlock", crate::icons::LOCK_OPEN) } else { format!("{}  Lock", crate::icons::LOCK) }).clicked() {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Lock", |d| {
						for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.meta.locked = !locked;
						}
						for m in d.measurements.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.meta.locked = !locked;
						}
						for a in d.annotations.iter_mut().filter(|a| ids.contains(&a.id)) {
							a.meta.locked = !locked;
						}
					});
					ui.close();
				}
				ui.separator();
				if ui.button(egui::RichText::new(format!("{}  Delete {}", crate::icons::TRASH_2, if n > 1 { format!("{n} items") } else { String::new() })).color(Color32::from_rgb(0xF8, 0x71, 0x71))).clicked() {
					self.delete_selection(doc);
					ui.close();
				}
			} else {
				if !self.clipboard.is_empty() && ui.button(format!("{}  Paste", crate::icons::CLIPBOARD_PASTE)).clicked() {
					self.paste(doc, false);
					ui.close();
				}
				if ui.button(format!("{}  Select all on page", crate::icons::MOUSE_POINTER_2)).clicked() {
					self.select_all(doc);
					ui.close();
				}
				if ui.button(format!("{}  Fit page", crate::icons::MAXIMIZE)).clicked() {
					doc.fit_page();
					ui.close();
				}
			}
		});
	}

	// ── Edit commands ───────────────────────────────────────────────────

	pub fn delete_selection(&mut self, doc: &mut OpenDoc) {
		if doc.selection.is_empty() {
			return;
		}
		let ids = doc.selection.clone();
		let n = doc.doc.lock().edit("Delete", |d| d.delete_ids(&ids));
		doc.selection.clear();
		if n < ids.len() {
			self.requests.push(CanvasRequest::Status("Locked items were kept.".into()));
		}
	}

	pub fn copy(&mut self, doc: &OpenDoc) {
		self.clipboard = doc.doc.lock().data.clip(&doc.selection);
	}

	pub fn paste(&mut self, doc: &mut OpenDoc, offset: bool) {
		if self.clipboard.is_empty() {
			return;
		}
		let (dx, dy) = if offset { (0.01, 0.01) } else { (0.0, 0.0) };
		let clip = self.clipboard.clone();
		let page = doc.page;
		let meta = doc.doc.lock().new_meta();
		let ids = doc.doc.lock().edit("Paste", |d| d.paste(&clip, page, dx, dy, &meta));
		doc.selection = ids.into_iter().collect();
	}

	pub fn select_all(&mut self, doc: &mut OpenDoc) {
		let d = doc.doc.lock();
		let p = doc.page;
		let ids: HashSet<String> = d.data.marks.iter().filter(|m| m.page == p).map(|m| m.id.clone())
			.chain(d.data.measurements.iter().filter(|m| m.page == p).map(|m| m.id.clone()))
			.chain(d.data.annotations.iter().filter(|a| a.page == p).map(|a| a.id.clone()))
			.collect();
		drop(d);
		doc.selection = ids;
	}

	pub fn nudge(&mut self, doc: &mut OpenDoc, dx_px: f32, dy_px: f32) {
		if doc.selection.is_empty() {
			return;
		}
		let (fx, fy) = doc.frac_scale();
		let ids = doc.selection.clone();
		doc.doc.lock().edit("Nudge", |d| d.translate_ids(&ids, (dx_px / fx) as f64, (dy_px / fy) as f64));
	}
}

// ── Painting helpers ────────────────────────────────────────────────────

/// Changes when the overlay's picture does (not its opacity or position).
fn overlay_key(o: &Overlay) -> u64 {
	use std::hash::{Hash, Hasher};
	let mut h = std::collections::hash_map::DefaultHasher::new();
	o.svg.hash(&mut h);
	o.image.hash(&mut h);
	if let Some(m) = o.image.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()) {
		m.hash(&mut h);
	}
	h.finish()
}

fn bbox(pts: &[Pos2]) -> Option<Rect> {
	let first = *pts.first()?;
	Some(pts.iter().fold(Rect::from_min_max(first, first), |r, p| r.union(Rect::from_min_max(*p, *p))))
}

fn centroid(pts: &[Pos2]) -> Pos2 {
	if pts.is_empty() {
		return Pos2::ZERO;
	}
	let s = pts.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2());
	(s / pts.len() as f32).to_pos2()
}

fn mid_of(pts: &[Pos2]) -> Pos2 {
	if pts.len() < 2 {
		return pts.first().copied().unwrap_or(Pos2::ZERO);
	}
	// Midpoint of the longest segment reads best.
	let (i, _) = pts.windows(2).enumerate().map(|(i, w)| (i, (w[1] - w[0]).length())).fold((0, 0.0), |b, x| if x.1 > b.1 { x } else { b });
	Pos2::new((pts[i].x + pts[i + 1].x) / 2.0, (pts[i].y + pts[i + 1].y) / 2.0 - 12.0)
}

fn trim(v: f64) -> String {
	let s = format!("{v:.3}");
	s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn badge(shapes: &mut Vec<Shape>, painter: &egui::Painter, at: Pos2, text: &str, color: Color32, pending: bool) {
	let galley = painter.layout_no_wrap(text.to_string(), theme::semibold(10.0), Color32::WHITE);
	let size = galley.size() + Vec2::new(8.0, 2.0);
	let r = Rect::from_min_size(at - Vec2::new(size.x * 0.4, size.y * 0.6), Vec2::new(size.x.max(size.y), size.y));
	shapes.push(Shape::rect_filled(r, r.height() / 2.0, if pending { with_alpha(color, 0.75) } else { color }));
	shapes.push(Shape::galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE));
}

fn pill(painter: &egui::Painter, at: Pos2, text: &str, color: Color32, strong: bool) {
	let galley = painter.layout_no_wrap(text.to_string(), theme::semibold(11.0), Color32::WHITE);
	let size = galley.size() + Vec2::new(12.0, 5.0);
	let r = Rect::from_center_size(at, size);
	painter.rect_filled(r.translate(Vec2::new(0.0, 1.0)), size.y / 2.0, Color32::from_black_alpha(50));
	painter.rect_filled(r, size.y / 2.0, if strong { color } else { with_alpha(color, 0.9) });
	painter.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
}
