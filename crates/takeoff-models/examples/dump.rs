//! Prints the non-vocab metadata keys of a GGUF file: `cargo run --example dump -- model.gguf`
fn main() {
	let path = std::env::args().nth(1).expect("usage: dump <file.gguf>");
	let meta = takeoff_models::gguf::read_meta(path.as_ref()).expect("read gguf");
	println!("version {} tensors {}", meta.version, meta.tensor_count);
	for (k, v) in &meta.kv {
		let s = format!("{v:?}");
		println!("{k} = {}", if s.len() > 160 { &s[..160] } else { &s });
	}
}
