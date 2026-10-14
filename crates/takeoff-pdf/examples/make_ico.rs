//! Dev helper: `cargo run -p takeoff-pdf --example make_ico -- assets/icon.svg assets/icon.ico`
use image::codecs::ico::{IcoEncoder, IcoFrame};

fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let svg = std::fs::read_to_string(&a[0]).unwrap();
	let tree = resvg::usvg::Tree::from_str(&svg, &Default::default()).unwrap();
	let frames: Vec<IcoFrame> = [16u32, 24, 32, 48, 64, 128, 256]
		.iter()
		.map(|&s| {
			let mut pix = resvg::tiny_skia::Pixmap::new(s, s).unwrap();
			let k = s as f32 / tree.size().width();
			resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pix.as_mut());
			// tiny-skia is premultiplied; ICO wants straight alpha.
			let img = image::RgbaImage::from_fn(s, s, |x, y| {
				let c = pix.pixel(x, y).unwrap().demultiply();
				image::Rgba([c.red(), c.green(), c.blue(), c.alpha()])
			});
			IcoFrame::as_png(img.as_raw(), s, s, image::ExtendedColorType::Rgba8).unwrap()
		})
		.collect();
	IcoEncoder::new(std::fs::File::create(&a[1]).unwrap()).encode_images(&frames).unwrap();
}
