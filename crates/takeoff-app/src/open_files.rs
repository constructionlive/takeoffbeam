//! Drawings the OS asks us to open. Windows and Linux pass them as arguments (`main.rs`).
//! macOS sends Finder's double-click and *Open With* as Apple Events, at launch and while
//! running, and winit doesn't pass those on: [`install`] teaches winit's app delegate
//! `application:openURLs:`, which queues the paths here for the next frame.

use std::path::PathBuf;
use std::sync::OnceLock;

use eframe::egui;
use parking_lot::Mutex;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Queue a file and wake the UI if it is up.
pub fn push(path: PathBuf) {
	PENDING.lock().push(path);
	if let Some(ctx) = CTX.get() {
		ctx.request_repaint();
	}
}

/// Files queued since the last call.
pub fn take() -> Vec<PathBuf> {
	std::mem::take(&mut *PENDING.lock())
}

/// The context to wake when a file arrives while the app is idle.
pub fn set_context(ctx: &egui::Context) {
	let _ = CTX.set(ctx.clone());
}

/// Add `application:openURLs:` to the class of the app's delegate (winit's). Call after the
/// event loop is built (it sets the delegate) and before it runs (launch files arrive in
/// `finishLaunching`).
#[cfg(target_os = "macos")]
pub fn install() {
	use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
	use objc2::ffi::objc_class;
	use objc2::{class, msg_send, sel};

	unsafe extern "C" fn open_urls(_this: *mut AnyObject, _cmd: Sel, app: *mut AnyObject, urls: *mut AnyObject) {
		if urls.is_null() {
			return;
		}
		// egui only runs frames for a visible window: bring it back if hidden (⌘H) or in the Dock.
		if !app.is_null() {
			let none: *mut AnyObject = std::ptr::null_mut();
			let _: () = unsafe { msg_send![app, unhide: none] };
			let windows: *mut AnyObject = unsafe { msg_send![app, windows] };
			let n: usize = if windows.is_null() { 0 } else { unsafe { msg_send![windows, count] } };
			for k in 0..n {
				let w: *mut AnyObject = unsafe { msg_send![windows, objectAtIndex: k] };
				if !w.is_null() && unsafe { msg_send![w, isMiniaturized] } {
					let _: () = unsafe { msg_send![w, deminiaturize: none] };
				}
			}
		}
		let n: usize = unsafe { msg_send![urls, count] };
		for k in 0..n {
			let url: *mut AnyObject = unsafe { msg_send![urls, objectAtIndex: k] };
			if url.is_null() {
				continue;
			}
			let file: bool = unsafe { msg_send![url, isFileURL] };
			let path: *mut AnyObject = if file { unsafe { msg_send![url, path] } } else { std::ptr::null_mut() };
			if path.is_null() {
				continue;
			}
			let s: *const std::ffi::c_char = unsafe { msg_send![path, UTF8String] };
			if !s.is_null() {
				push(PathBuf::from(unsafe { std::ffi::CStr::from_ptr(s) }.to_string_lossy().into_owned()));
			}
		}
	}

	unsafe {
		let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
		let delegate: *mut AnyObject = msg_send![app, delegate];
		let Some(delegate) = delegate.as_ref() else { return };
		let cls: *const AnyClass = delegate.class();
		let imp: Imp = std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject), Imp>(open_urls);
		// Returns NO if winit ever implements it itself; then winit's wins.
		objc2::ffi::class_addMethod(cls as *mut objc_class, sel!(application:openURLs:).as_ptr(), Some(imp), c"v@:@@".as_ptr());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn queue_drains() {
		push("/tmp/a.pdf".into());
		push("/tmp/b.pdf".into());
		assert_eq!(take(), vec![PathBuf::from("/tmp/a.pdf"), PathBuf::from("/tmp/b.pdf")]);
		assert!(take().is_empty());
	}
}
