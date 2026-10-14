//! `cargo run --release -p takeoff-pdf --example layers -- plans.pdf <page> <out_dir> [layer-substring]`
//! Lists layers, then renders the page with every layer containing the substring hidden.
use std::collections::HashSet;
use std::time::Instant;

use takeoff_pdf::{Drawing, FracRect};

fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let page: u32 = a[1].parse().unwrap();
	let t = Instant::now();
	let mut d = Drawing::open(&a[0]).unwrap();
	let names = d.layer_names();
	println!("{} layers ({:?}): {:?}", names.len(), t.elapsed(), &names[..names.len().min(40)]);
	println!("default off: {:?}", d.default_off_layers());
	let before = d.render(page, 50.0, FracRect::FULL, None).unwrap();
	before.image.save(format!("{}/layers_all.png", a[2])).unwrap();
	let needle = a.get(3).cloned().unwrap_or_default().to_lowercase();
	let hide: HashSet<String> = names.iter().filter(|n| n.to_lowercase().contains(&needle)).cloned().collect();
	let t = Instant::now();
	d.set_hidden_layers(&hide).unwrap();
	println!("hid {} layers, reload {:?}", hide.len(), t.elapsed());
	let after = d.render(page, 50.0, FracRect::FULL, None).unwrap();
	after.image.save(format!("{}/layers_hidden.png", a[2])).unwrap();
	let diff = before.image.pixels().zip(after.image.pixels()).filter(|(x, y)| x != y).count();
	println!("pixels changed: {diff} of {}", before.image.pixels().len());
}
