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

mod paint;
mod select;

/// What the canvas asks the app to do (dialogs it can't show itself).
#[derive(Debug, Clone)]
pub enum CanvasRequest {
	Calibrate { page: u32, p1: Point, p2: Point },
	Viewport { page: u32, rect: (f64, f64, f64, f64) },
	Depth { id: String },
	Status(String),
	/// Something new to set up (a space's name, a legend's options): show Properties.
	ShowProperties,
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
	/// Fill Space: the outline becomes a space, not an area.
	space: bool,
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
			Some(Ok(f)) if job.space => {
				let id = match job.grow {
					Some(id) => {
						doc.doc.lock().edit("Fill space", |d| {
							if let Some(s) = d.spaces.iter_mut().find(|s| s.id == id) {
								s.points = f.points;
							}
						});
						id
					}
					None => self.space_added(doc, crate::spaces::add(doc, f.points)),
				};
				self.flash = Some(([id.clone()].into(), Instant::now()));
				self.last_fill = Some((id, job.page, job.at));
			}
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

	/// Select a space just made, with its name ready to change in Properties.
	fn space_added(&mut self, doc: &mut OpenDoc, id: String) -> String {
		doc.selection.clear();
		doc.selection.insert(id.clone());
		self.requests.push(CanvasRequest::ShowProperties);
		self.requests.push(CanvasRequest::Status("Space added: name it in Properties".into()));
		id
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

	// ── Hit testing ─────────────────────────────────────────────────────

	/// Topmost item under the screen point.
	pub fn hit(&self, doc: &OpenDoc, pos: Pos2) -> Option<String> {
		let d = doc.doc.lock();
		let data = &d.data;
		let page = doc.page;
		if let Some(id) = crate::legends::hit(doc, data, pos) {
			return Some(id);
		}
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
		crate::spaces::hit(doc, data, pos)
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
		if let Some(idx) = d.data.spaces.iter().find(|s| s.id == id).and_then(|s| s.points.iter().position(near)) {
			return Some(Drag::Vertex { id, idx, cutout: None });
		}
		if let Some(corner) = d.data.legends.iter().find(|l| l.id == id).and_then(|l| crate::legends::corner_at(doc, l, pos)) {
			return Some(Drag::Resize { id, corner });
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
			Tool::Fill | Tool::SpaceFill => {
				ui.ctx().set_cursor_icon(if self.fill_job.is_some() { CursorIcon::Progress } else { CursorIcon::Crosshair });
				if resp.clicked() && in_canvas && self.fill_job.is_none() {
					let raw = doc.to_page(pos);
					let space = self.tool == Tool::SpaceFill;
					// Shift-click: another piece of the last fill, filled together with it.
					let grow = self.last_fill.clone().filter(|(id, page, _)| {
						let d = doc.doc.lock();
						mods.shift && *page == doc.page && if space { d.data.spaces.iter().any(|s| &s.id == id) } else { d.data.measurements.iter().any(|m| &m.id == id) }
					});
					let at = match &grow {
						Some((_, _, at)) => at.iter().copied().chain([raw]).collect(),
						None => vec![raw],
					};
					let token = doc.request_fill(at.clone());
					self.fill_job = Some(FillJob { token, page: doc.page, at, grow: grow.map(|g| g.0), space });
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
			Tool::RectArea | Tool::Viewport | Tool::Legend | Tool::Annot(AnnotationKind::Rectangle | AnnotationKind::Ellipse | AnnotationKind::Highlight | AnnotationKind::Text | AnnotationKind::Stamp | AnnotationKind::Line | AnnotationKind::Arrow) => {
				ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
				if resp.drag_started() && in_canvas {
					self.drag = Some(Drag::Shape { start: start_pt });
				}
				if resp.drag_stopped() {
					if let Some(Drag::Shape { start }) = self.drag.take() {
						self.finish_shape(doc, start, pt);
					}
				} else if resp.clicked() && in_canvas && self.tool == Tool::Legend {
					self.finish_shape(doc, pt, pt);
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
		if tiny && !matches!(self.tool, Tool::Legend | Tool::Annot(AnnotationKind::Text | AnnotationKind::Stamp)) {
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
			Tool::Legend => {
				let id = crate::legends::add(doc, tl, br);
				doc.selection.clear();
				doc.selection.insert(id);
				self.requests.push(CanvasRequest::ShowProperties);
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
		matches!(self.tool, Tool::Length | Tool::Curve | Tool::Trace | Tool::CurveMarkup | Tool::Perimeter | Tool::Area | Tool::Volume | Tool::Cutout | Tool::Space)
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
			let closed = matches!(self.tool, Tool::Perimeter | Tool::Area | Tool::Volume | Tool::Cutout | Tool::Space);
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
			Tool::Space if pts.len() >= 3 => {
				let id = crate::spaces::add(doc, pts);
				self.space_added(doc, id);
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
}

// ── Helpers ─────────────────────────────────────────────────────────────

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

