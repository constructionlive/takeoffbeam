//! Resumable model downloads from Hugging Face.
//!
//! Each file streams into `<models_dir>/<id>/<file>.part`; an interrupted
//! download (quit, network drop, cancel) resumes from the `.part` length with
//! an HTTP Range request. The `.part` is renamed only after its size matches
//! the server's, so a half file is never mistaken for an installed model.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::catalog::CatalogModel;

#[derive(Clone, Debug, PartialEq)]
pub enum DownloadState {
	Running,
	Done,
	Failed(String),
	Cancelled,
}

#[derive(Clone, Debug)]
pub struct DownloadProgress {
	pub done: u64,
	/// 0 until the sizes are known.
	pub total: u64,
	pub state: DownloadState,
	pub bytes_per_sec: f64,
	/// File currently downloading.
	pub file: String,
	/// Bytes of the current file that were resumed from an earlier `.part` (0 = fresh).
	pub resumed_from: u64,
}

struct Inner {
	progress: DownloadProgress,
	window: Vec<(Instant, u64)>,
}

#[derive(Clone)]
pub struct DownloadHandle {
	inner: Arc<Mutex<Inner>>,
	cancel: Arc<AtomicBool>,
	pub model_id: String,
	pub dir: PathBuf,
}

impl DownloadHandle {
	pub fn progress(&self) -> DownloadProgress {
		self.inner.lock().progress.clone()
	}
	/// Stops after the current chunk; the `.part` stays, so a later `start` resumes.
	pub fn cancel(&self) {
		self.cancel.store(true, Ordering::Relaxed);
	}
	pub fn is_finished(&self) -> bool {
		self.inner.lock().progress.state != DownloadState::Running
	}
}

/// Download a catalog model (projector first, it is small) into `models_dir()/<id>/`.
pub fn start(model: &CatalogModel) -> DownloadHandle {
	let mut files = Vec::new();
	if let Some(mm) = &model.mmproj {
		files.push((model.url(mm), mm.clone()));
	}
	files.push((model.url(&model.file), model.file.clone()));
	start_urls(&model.id, files, crate::store::models_dir().join(&model.id))
}

/// Download arbitrary `(url, file name)` pairs into `dir` (used by `start`; handy for tests).
pub fn start_urls(model_id: &str, files: Vec<(String, String)>, dir: PathBuf) -> DownloadHandle {
	let handle = DownloadHandle {
		inner: Arc::new(Mutex::new(Inner {
			progress: DownloadProgress { done: 0, total: 0, state: DownloadState::Running, bytes_per_sec: 0.0, file: String::new(), resumed_from: 0 },
			window: Vec::new(),
		})),
		cancel: Arc::new(AtomicBool::new(false)),
		model_id: model_id.to_string(),
		dir: dir.clone(),
	};
	let h = handle.clone();
	std::thread::Builder::new()
		.name(format!("download-{model_id}"))
		.spawn(move || {
			let result = run(&h, &files, &dir);
			let mut g = h.inner.lock();
			g.progress.state = match result {
				Ok(()) => DownloadState::Done,
				Err(_) if h.cancel.load(Ordering::Relaxed) => DownloadState::Cancelled,
				Err(e) => DownloadState::Failed(e),
			};
			g.progress.bytes_per_sec = 0.0;
		})
		.expect("spawn download thread");
	handle
}

fn client() -> Result<reqwest::blocking::Client, String> {
	reqwest::blocking::Client::builder()
		.user_agent(concat!("TakeoffDesk/", env!("CARGO_PKG_VERSION")))
		.connect_timeout(Duration::from_secs(20))
		// No overall timeout: multi-GB bodies take a while. Reads stall-check below.
		.timeout(None)
		.build()
		.map_err(|e| e.to_string())
}

/// Content length of `url` after redirects.
fn remote_size(c: &reqwest::blocking::Client, url: &str) -> Result<u64, String> {
	let r = c.head(url).send().map_err(|e| e.to_string())?;
	if !r.status().is_success() {
		return Err(format!("{url}: HTTP {}", r.status()));
	}
	r.headers()
		.get(reqwest::header::CONTENT_LENGTH)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.parse().ok())
		.ok_or_else(|| format!("{url}: server did not report a size"))
}

fn run(h: &DownloadHandle, files: &[(String, String)], dir: &PathBuf) -> Result<(), String> {
	std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
	let c = client()?;
	let mut sizes = Vec::new();
	for (url, _) in files {
		sizes.push(remote_size(&c, url)?);
	}
	let total: u64 = sizes.iter().sum();
	let mut base = 0u64;
	{
		let mut g = h.inner.lock();
		g.progress.total = total;
	}
	for ((url, name), size) in files.iter().zip(&sizes) {
		let dest = dir.join(name);
		if dest.is_file() && std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) == *size {
			base += size;
			set_done(h, base);
			continue;
		}
		h.inner.lock().progress.file = name.clone();
		let part = dir.join(format!("{name}.part"));
		let mut attempts = 0;
		loop {
			match fetch(h, &c, url, &part, *size, base) {
				Ok(()) => break,
				Err(e) if h.cancel.load(Ordering::Relaxed) => return Err(e),
				Err(e) => {
					attempts += 1;
					if attempts >= 6 {
						return Err(format!("{name}: {e}"));
					}
					std::thread::sleep(Duration::from_secs(2u64.pow(attempts.min(4))));
				}
			}
		}
		let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
		if got != *size {
			return Err(format!("{name}: downloaded {got} bytes, expected {size}"));
		}
		std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
		base += size;
		set_done(h, base);
	}
	Ok(())
}

fn set_done(h: &DownloadHandle, done: u64) {
	let mut g = h.inner.lock();
	g.progress.done = done;
	let now = Instant::now();
	// Went back (the server ignored Range, the file restarted): the speed starts over too.
	if g.window.last().is_some_and(|w| w.1 > done) {
		g.window.clear();
	}
	g.window.push((now, done));
	g.window.retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(5));
	if let (Some(first), Some(last)) = (g.window.first().copied(), g.window.last().copied()) {
		let dt = last.0.duration_since(first.0).as_secs_f64();
		if dt > 0.3 {
			g.progress.bytes_per_sec = (last.1 - first.1) as f64 / dt;
		}
	}
}

/// Append the rest of `url` to `part`, resuming from its current length.
fn fetch(h: &DownloadHandle, c: &reqwest::blocking::Client, url: &str, part: &PathBuf, size: u64, base: u64) -> Result<(), String> {
	let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
	if have > size {
		std::fs::remove_file(part).map_err(|e| e.to_string())?;
		return Err("partial file larger than the remote; restarting".into());
	}
	if have == size {
		return Ok(());
	}
	let mut req = c.get(url);
	if have > 0 {
		req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
	}
	let mut resp = req.send().map_err(|e| e.to_string())?;
	let status = resp.status();
	let mut file = OpenOptions::new().create(true).append(true).open(part).map_err(|e| e.to_string())?;
	let mut written = have;
	if have > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
		// Server ignored Range: start over.
		file.set_len(0).map_err(|e| e.to_string())?;
		written = 0;
	} else if have > 0 {
		h.inner.lock().progress.resumed_from = have;
	}
	if !status.is_success() {
		return Err(format!("HTTP {status}"));
	}
	let mut buf = vec![0u8; 1 << 20];
	loop {
		if h.cancel.load(Ordering::Relaxed) {
			file.flush().ok();
			return Err("cancelled".into());
		}
		let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
		if n == 0 {
			break;
		}
		file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
		written += n as u64;
		set_done(h, base + written);
	}
	file.flush().map_err(|e| e.to_string())?;
	if written < size {
		return Err(format!("connection closed at {written} of {size} bytes"));
	}
	Ok(())
}
