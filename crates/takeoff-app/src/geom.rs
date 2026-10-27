//! Screen-space geometry for the canvas: concave polygon fills (with holes),
//! hatch lines clipped to a polygon, revision-cloud outlines, arrowheads.

use eframe::egui::{epaint::Mesh, Color32, Pos2, Shape, Stroke, Vec2};

/// Filled polygon with optional holes, triangulated (egui only fills convex paths).
pub fn fill_polygon(outer: &[Pos2], holes: &[Vec<Pos2>], color: Color32) -> Shape {
	if outer.len() < 3 {
		return Shape::Noop;
	}
	let mut coords: Vec<f64> = Vec::with_capacity((outer.len() + holes.iter().map(Vec::len).sum::<usize>()) * 2);
	let mut hole_idx = Vec::new();
	let mut verts: Vec<Pos2> = Vec::new();
	for p in outer {
		coords.extend([p.x as f64, p.y as f64]);
		verts.push(*p);
	}
	for h in holes.iter().filter(|h| h.len() >= 3) {
		hole_idx.push(verts.len());
		for p in h {
			coords.extend([p.x as f64, p.y as f64]);
			verts.push(*p);
		}
	}
	let Ok(tris) = earcutr::earcut(&coords, &hole_idx, 2) else { return Shape::Noop };
	let mut mesh = Mesh::default();
	for v in &verts {
		mesh.colored_vertex(*v, color);
	}
	for t in tris.chunks(3) {
		if t.len() == 3 {
			mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
		}
	}
	Shape::mesh(mesh)
}

/// Pieces of the horizontal line y inside the polygon (even-odd).
fn scan(y: f32, polys: &[&[Pos2]]) -> Vec<(f32, f32)> {
	let mut xs = Vec::new();
	for poly in polys {
		let n = poly.len();
		for i in 0..n {
			let (a, b) = (poly[i], poly[(i + 1) % n]);
			if (a.y > y) != (b.y > y) {
				xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
			}
		}
	}
	xs.sort_by(f32::total_cmp);
	xs.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

fn rotate(p: Pos2, c: Pos2, ang: f32) -> Pos2 {
	let (s, co) = ang.sin_cos();
	let d = p - c;
	Pos2::new(c.x + d.x * co - d.y * s, c.y + d.x * s + d.y * co)
}

/// Hatch lines at `angle_deg`, `gap` apart, clipped to outer minus holes.
pub fn hatch(outer: &[Pos2], holes: &[Vec<Pos2>], angle_deg: f32, gap: f32, stroke: Stroke, out: &mut Vec<Shape>) {
	if outer.len() < 3 || gap < 2.0 {
		return;
	}
	let c = outer.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / outer.len() as f32;
	let c = c.to_pos2();
	let ang = angle_deg.to_radians();
	// Rotate the shape so the hatch is horizontal, scan, rotate back.
	let ro: Vec<Pos2> = outer.iter().map(|p| rotate(*p, c, -ang)).collect();
	let rh: Vec<Vec<Pos2>> = holes.iter().map(|h| h.iter().map(|p| rotate(*p, c, -ang)).collect()).collect();
	let mut polys: Vec<&[Pos2]> = vec![&ro];
	polys.extend(rh.iter().map(|h| h.as_slice()));
	let (y0, y1) = ro.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
	let mut y = y0 + gap / 2.0;
	let mut guard = 0;
	while y < y1 && guard < 4000 {
		for (xa, xb) in scan(y, &polys) {
			out.push(Shape::line_segment([rotate(Pos2::new(xa, y), c, ang), rotate(Pos2::new(xb, y), c, ang)], stroke));
		}
		y += gap;
		guard += 1;
	}
}

/// Scalloped revision-cloud outline around a polygon (screen px).
pub fn cloud(poly: &[Pos2], arc: f32) -> Vec<Pos2> {
	if poly.len() < 3 {
		return poly.to_vec();
	}
	let area: f32 = (0..poly.len()).map(|i| {
		let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
		a.x * b.y - b.x * a.y
	}).sum();
	let sign = if area >= 0.0 { -1.0 } else { 1.0 };
	let mut out = Vec::new();
	for i in 0..poly.len() {
		let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
		let d = b - a;
		let len = d.length().max(0.001);
		let n = (len / arc).ceil().max(1.0) as usize;
		let normal = Vec2::new(-d.y, d.x) / len * sign;
		for k in 0..n {
			for s in 0..=8 {
				let t = (k as f32 + s as f32 / 8.0) / n as f32;
				let bulge = (s as f32 / 8.0 * std::f32::consts::PI).sin() * len / n as f32 * 0.42;
				out.push(a + d * t + normal * bulge);
			}
		}
	}
	out
}

pub fn arrow_head(from: Pos2, to: Pos2, size: f32, color: Color32) -> Shape {
	let d = (to - from).normalized();
	let n = Vec2::new(-d.y, d.x);
	Shape::convex_polygon(vec![to, to - d * size + n * size * 0.45, to - d * size - n * size * 0.45], color, Stroke::NONE)
}

pub fn dashed(points: &[Pos2], closed: bool, stroke: Stroke, dash: f32, gap: f32, out: &mut Vec<Shape>) {
	let mut pts = points.to_vec();
	if closed && pts.len() > 2 {
		pts.push(pts[0]);
	}
	out.extend(Shape::dashed_line(&pts, stroke, dash, gap));
}

pub fn ellipse_points(a: Pos2, b: Pos2, n: usize) -> Vec<Pos2> {
	let c = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
	let (rx, ry) = ((a.x - b.x).abs() / 2.0, (a.y - b.y).abs() / 2.0);
	(0..n).map(|i| {
		let t = i as f32 / n as f32 * std::f32::consts::TAU;
		Pos2::new(c.x + rx * t.cos(), c.y + ry * t.sin())
	}).collect()
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
	let ab = b - a;
	let len2 = ab.length_sq();
	let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
	(p - (a + ab * t)).length()
}

pub fn dist_to_polyline(p: Pos2, pts: &[Pos2], closed: bool) -> f32 {
	let mut best = f32::MAX;
	for w in pts.windows(2) {
		best = best.min(dist_to_segment(p, w[0], w[1]));
	}
	if closed && pts.len() > 2 {
		best = best.min(dist_to_segment(p, *pts.last().unwrap(), pts[0]));
	}
	if pts.len() == 1 {
		best = (p - pts[0]).length();
	}
	best
}

pub fn point_in_poly(p: Pos2, poly: &[Pos2]) -> bool {
	let mut inside = false;
	let n = poly.len();
	if n < 3 {
		return false;
	}
	let mut j = n - 1;
	for i in 0..n {
		let (a, b) = (poly[i], poly[j]);
		if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
			inside = !inside;
		}
		j = i;
	}
	inside
}
