//! Dev helper: `cargo run -p takeoff-pdf --example svg2png -- in.svg out.png [size]`
fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let size: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(512);
	let svg = std::fs::read_to_string(&a[0]).unwrap();
	let tree = resvg::usvg::Tree::from_str(&svg, &Default::default()).unwrap();
	let mut pix = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
	let s = size as f32 / tree.size().width();
	resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pix.as_mut());
	pix.save_png(&a[1]).unwrap();
}
