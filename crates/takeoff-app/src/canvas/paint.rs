//! Painting the sheet: page pictures, the takeoff items, overlays, the document
//! map, shapes in progress with their live quantity, and markup text.

use super::*;

impl Canvas {
	pub(super) fn paint_page(&self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
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
	pub(super) fn paint_sheet_overlays(&mut self, ctx: &egui::Context, painter: &egui::Painter, doc: &OpenDoc, p: &Palette, pages: &[u32]) {
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

	pub(super) fn paint_items(&mut self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
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

		// Spaces under everything.
		crate::spaces::paint(painter, doc, data, &flash, p);

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
		// Legends on top.
		crate::legends::paint(painter, doc, &d, &flash, p);
	}

	pub(super) fn paint_overlays(&mut self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
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
	pub(super) fn paint_map(&self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
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

	pub(super) fn paint_in_progress(&self, painter: &egui::Painter, doc: &OpenDoc, p: &Palette) {
		let hover = painter.ctx().input(|i| i.pointer.hover_pos());
		let cursor = hover.filter(|h| doc.canvas_rect.contains(*h)).map(|h| self.hover_snap.map(|s| doc.to_screen(s.point)).unwrap_or(h));
		let accent = p.accent;
		let stroke = Stroke::new(2.0, accent);
		if !self.points.is_empty() {
			let closed = matches!(self.tool, Tool::Area | Tool::Volume | Tool::Cutout | Tool::Space | Tool::Annot(AnnotationKind::Polygon | AnnotationKind::Cloud));
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

	pub(super) fn live_quantity(&self, doc: &OpenDoc, pts: &[Point]) -> Option<String> {
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

	pub(super) fn live_area(&self, doc: &OpenDoc, pts: &[Point]) -> Option<String> {
		let d = doc.doc.lock();
		let (w, h) = doc.page_size(doc.page);
		let m = Measurement::new("live", doc.page, MeasurementType::Area, pts.to_vec());
		let q = measurement_quantity(&m, d.data.scale_at(doc.page, pts[0]), w, h)?;
		Some(format_quantity(q.value, &q.unit, &d.data.display))
	}

	pub(super) fn paint_annotation_text(&self, ui: &egui::Ui, doc: &OpenDoc) {
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
}

pub(super) fn badge(shapes: &mut Vec<Shape>, painter: &egui::Painter, at: Pos2, text: &str, color: Color32, pending: bool) {
	let galley = painter.layout_no_wrap(text.to_string(), theme::semibold(10.0), Color32::WHITE);
	let size = galley.size() + Vec2::new(8.0, 2.0);
	let r = Rect::from_min_size(at - Vec2::new(size.x * 0.4, size.y * 0.6), Vec2::new(size.x.max(size.y), size.y));
	shapes.push(Shape::rect_filled(r, r.height() / 2.0, if pending { with_alpha(color, 0.75) } else { color }));
	shapes.push(Shape::galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE));
}

pub(super) fn pill(painter: &egui::Painter, at: Pos2, text: &str, color: Color32, strong: bool) {
	let galley = painter.layout_no_wrap(text.to_string(), theme::semibold(11.0), Color32::WHITE);
	let size = galley.size() + Vec2::new(12.0, 5.0);
	let r = Rect::from_center_size(at, size);
	painter.rect_filled(r.translate(Vec2::new(0.0, 1.0)), size.y / 2.0, Color32::from_black_alpha(50));
	painter.rect_filled(r, size.y / 2.0, if strong { color } else { with_alpha(color, 0.9) });
	painter.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
}
