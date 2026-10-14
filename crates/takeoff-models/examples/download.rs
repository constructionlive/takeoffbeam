//! Live check of resumable downloads with a ~1 MB file: a pre-seeded `.part`
//! must be resumed with a Range request and end byte-identical.
//! `cargo run -p takeoff-models --example download -- <dir> <reference-file>`
use std::time::Duration;

use takeoff_models::download::{start_urls, DownloadState};

fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let dir = std::path::PathBuf::from(&a[0]);
	let url = "https://huggingface.co/ggml-org/models/resolve/main/tinyllamas/stories260K.gguf".to_string();
	let h = start_urls("tiny", vec![(url, "stories260K.gguf".into())], dir.clone());
	loop {
		let p = h.progress();
		if p.state != DownloadState::Running {
			println!("state {:?} done {} / {} resumed_from {}", p.state, p.done, p.total, p.resumed_from);
			break;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	let got = std::fs::read(dir.join("stories260K.gguf")).unwrap();
	let want = std::fs::read(&a[1]).unwrap();
	println!("identical: {} ({} bytes), part left: {}", got == want, got.len(), dir.join("stories260K.gguf.part").exists());
}
