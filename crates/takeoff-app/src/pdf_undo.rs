//! Undo for edits that rewrite the PDF itself (page tools, slip-sheets).
//! The takeoff's undo history holds a `pdf_state` with each step; this keeps
//! the file that goes with each state for the session (a hard link, so it
//! costs nothing: the tools replace the file by renaming over it, never in
//! place), and puts the right file back when undo or redo moves the takeoff
//! to another state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct PdfVersions {
	dir: Option<PathBuf>,
	files: HashMap<u64, PathBuf>,
	/// The state of the file on disk now.
	pub on_disk: u64,
	next: u64,
}

impl Default for PdfVersions {
	fn default() -> Self {
		Self { dir: None, files: HashMap::new(), on_disk: 0, next: 1 }
	}
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl PdfVersions {
	fn dir(&mut self) -> std::io::Result<PathBuf> {
		if let Some(d) = &self.dir {
			return Ok(d.clone());
		}
		let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
		let d = std::env::temp_dir().join(format!("takeoff-pdf-undo-{}-{n}", std::process::id()));
		std::fs::create_dir_all(&d)?;
		self.dir = Some(d.clone());
		Ok(d)
	}

	/// Keep the file as it is now under its state.
	fn keep(&mut self, path: &Path, state: u64) -> std::io::Result<()> {
		if self.files.contains_key(&state) {
			return Ok(());
		}
		let dst = self.dir()?.join(format!("{state}.pdf"));
		let _ = std::fs::remove_file(&dst);
		if std::fs::hard_link(path, &dst).is_err() {
			std::fs::copy(path, &dst)?;
		}
		self.files.insert(state, dst);
		Ok(())
	}

	/// Before rewriting the PDF: keep the file as it is, so undo can put it back.
	pub fn before_edit(&mut self, path: &Path) -> std::io::Result<()> {
		self.keep(path, self.on_disk)
	}

	/// After rewriting it: the new file's state, to store in the takeoff with the edit.
	pub fn after_edit(&mut self, path: &Path) -> u64 {
		let state = self.next;
		self.next += 1;
		self.on_disk = state;
		if let Err(e) = self.keep(path, state) {
			eprintln!("takeoff: couldn't keep the edited PDF for redo: {e}");
		}
		state
	}

	/// The takeoff now wants the file of `want` (after an undo or redo): put it
	/// back. True when the file on disk changed (reload it).
	pub fn sync(&mut self, path: &Path, want: u64) -> Result<bool, String> {
		if want == self.on_disk {
			return Ok(false);
		}
		let Some(src) = self.files.get(&want).cloned() else {
			// Not a state of this session (shouldn't happen): leave the file alone.
			self.on_disk = want;
			return Err("this version of the PDF is no longer available; the page edit can't be undone".into());
		};
		// Link next to the drawing, then rename over it: atomic, like the tools.
		let tmp = path.with_extension("pdf.undo");
		let _ = std::fs::remove_file(&tmp);
		if std::fs::hard_link(&src, &tmp).is_err() {
			std::fs::copy(&src, &tmp).map_err(|e| e.to_string())?;
		}
		std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
		self.on_disk = want;
		Ok(true)
	}
}

impl Drop for PdfVersions {
	fn drop(&mut self) {
		if let Some(d) = &self.dir {
			let _ = std::fs::remove_dir_all(d);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn undo_and_redo_put_the_file_back() {
		let dir = std::env::temp_dir().join(format!("takeoff-pdf-undo-test-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let pdf = dir.join("plan.pdf");
		std::fs::write(&pdf, b"rev A").unwrap();
		let mut v = PdfVersions::default();
		v.before_edit(&pdf).unwrap();
		// The tools write a new file and rename it over the old one.
		std::fs::write(dir.join("t"), b"rev B").unwrap();
		std::fs::rename(dir.join("t"), &pdf).unwrap();
		let b = v.after_edit(&pdf);
		assert_eq!(v.sync(&pdf, b), Ok(false));
		assert_eq!(v.sync(&pdf, 0), Ok(true));
		assert_eq!(std::fs::read(&pdf).unwrap(), b"rev A");
		assert_eq!(v.sync(&pdf, b), Ok(true));
		assert_eq!(std::fs::read(&pdf).unwrap(), b"rev B");
		assert!(v.sync(&pdf, 99).is_err());
		let kept = v.dir.clone().unwrap();
		drop(v);
		assert!(!kept.exists());
		std::fs::remove_dir_all(dir).ok();
	}
}
