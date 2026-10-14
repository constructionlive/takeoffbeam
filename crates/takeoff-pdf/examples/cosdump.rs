use takeoff_pdf::cos::{key, CosFile};
fn main() {
	let path = std::env::args().nth(1).unwrap();
	let f = match CosFile::open(path.as_ref()) { Ok(f) => f, Err(e) => { println!("open error: {e}"); return; } };
	println!("startxref {} stream {} size {} trailer keys {:?}", f.startxref, f.xref_is_stream, f.size(), f.trailer.keys().map(|k| String::from_utf8_lossy(k).into_owned()).collect::<Vec<_>>());
	let root = f.root_ref().unwrap();
	let cat = f.get(root.0);
	match &cat { Ok(o) => println!("catalog {root:?}: {:?}", o.as_dict().map(|d| d.keys().map(|k| String::from_utf8_lossy(k).into_owned()).collect::<Vec<_>>())), Err(e) => println!("catalog error {e}") }
	if let Ok(c) = cat { if let Some(oc) = c.as_dict().and_then(|d| key(d, "OCProperties")) { println!("oc: {:?}", f.resolve(oc).map(|o| format!("{o:?}").chars().take(400).collect::<String>())); } }
	match takeoff_pdf::layers::LayerInfo::read(path.as_ref()) { Ok(Some(l)) => println!("{} layers", l.layers.len()), Ok(None) => println!("none"), Err(e) => println!("layers error {e}") }
}
