//! Crash reports. A panic hook writes one (message, place, backtrace, version, OS) to the logs
//! folder; the next start offers to email it through Feedback, or to dismiss it. Nothing is
//! sent unless the user clicks.

use std::path::{Path, PathBuf};

/// The report waiting to be offered; archived as `crash-<time>.txt` once sent or dismissed.
const PENDING: &str = "crash.txt";

/// `<data dir>/Takeoff Desk/logs` (on macOS ~/Library/Application Support/Takeoff Desk/logs).
pub fn log_dir() -> PathBuf {
	dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("Takeoff Desk").join("logs")
}

/// Write a report for every panic into [`log_dir`], then run the default hook (stderr).
pub fn install() {
	install_in(log_dir());
}

pub fn install_in(dir: PathBuf) {
	let prev = std::panic::take_hook();
	std::panic::set_hook(Box::new(move |info| {
		let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "(no message)".into());
		let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
		let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
		let bt = std::backtrace::Backtrace::force_capture().to_string();
		let _ = write(&dir, &report(&msg, &at, &thread, &bt));
		prev(info);
	}));
}

/// "Windows 11 (26100) x86_64" or "macOS 15.6 aarch64".
pub fn os() -> String {
	format!("{} {}", sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()), std::env::consts::ARCH)
}

pub fn report(msg: &str, location: &str, thread: &str, backtrace: &str) -> String {
	format!(
		"Takeoff Desk {} crashed\nTime: {}\nOS: {}\nThread: {thread}\nPanic: {msg}\nAt: {location}\n\nBacktrace:\n{backtrace}\n",
		env!("CARGO_PKG_VERSION"),
		takeoff_core::time::iso_now(),
		os()
	)
}

/// Append to the pending report (a crash can panic more than one thread).
pub fn write(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
	use std::io::Write;
	std::fs::create_dir_all(dir)?;
	let path = dir.join(PENDING);
	let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
	if f.metadata()?.len() > 0 {
		f.write_all(b"\n----\n\n")?;
	}
	f.write_all(text.as_bytes())?;
	Ok(path)
}

/// The report from a crash not yet offered, if any.
pub fn pending(dir: &Path) -> Option<String> {
	std::fs::read_to_string(dir.join(PENDING)).ok().filter(|s| !s.trim().is_empty())
}

/// Keep the report in the logs folder but stop offering it.
pub fn archive(dir: &Path) {
	let to = dir.join(format!("crash-{}.txt", takeoff_core::time::iso_now().replace(':', "-")));
	if std::fs::rename(dir.join(PENDING), &to).is_err() {
		let _ = std::fs::remove_file(dir.join(PENDING));
	}
}

/// The report's head and as much backtrace as fits `budget` bytes of URL-encoded text, so a
/// `mailto:` stays under the limits of Windows and mail apps. The full file is in the logs folder.
pub fn excerpt(report: &str, budget: usize) -> String {
	let mut out = String::new();
	let mut used = 0;
	for line in report.lines() {
		let cost = line.bytes().map(|b| if b.is_ascii_alphanumeric() { 1 } else { 3 }).sum::<usize>() + 3;
		if used + cost > budget {
			out.push_str("…\n");
			break;
		}
		used += cost;
		out.push_str(line);
		out.push('\n');
	}
	out
}

/// For Help → Copy diagnostics: version, OS, model, where the logs are, and the last crash.
pub fn diagnostics(model: &str) -> String {
	let dir = log_dir();
	let last = pending(&dir).or_else(|| {
		let mut old: Vec<PathBuf> = std::fs::read_dir(&dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("crash-"))).collect();
		old.sort();
		std::fs::read_to_string(old.last()?).ok()
	});
	let last = last.map(|r| r.lines().take(6).collect::<Vec<_>>().join("\n")).unwrap_or_else(|| "none".into());
	format!("Takeoff Desk {}\nOS: {}\nModel: {}\nLogs: {}\nLast crash: {last}\n", env!("CARGO_PKG_VERSION"), os(), if model.is_empty() { "none" } else { model }, dir.display())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn write_pending_archive() {
		let dir = std::env::temp_dir().join(format!("takeoff-crash-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		assert!(pending(&dir).is_none());
		let r = report("index out of bounds", "crates/x.rs:1:2", "main", "0: takeoff_app::x");
		assert!(r.contains(env!("CARGO_PKG_VERSION")) && r.contains("Panic: index out of bounds") && r.contains("At: crates/x.rs:1:2") && r.contains("0: takeoff_app::x"));
		write(&dir, &r).unwrap();
		write(&dir, "second").unwrap();
		let p = pending(&dir).unwrap();
		assert!(p.starts_with("Takeoff Desk") && p.ends_with("second"));
		archive(&dir);
		assert!(pending(&dir).is_none());
		assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn panic_hook_writes_report() {
		let dir = std::env::temp_dir().join(format!("takeoff-crash-hook-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		install_in(dir.clone());
		let _ = std::thread::Builder::new().name("render".into()).spawn(|| panic!("boom 42")).unwrap().join();
		let _ = std::panic::take_hook();
		let p = pending(&dir).unwrap();
		assert!(p.contains("Panic: boom 42") && p.contains("Thread: render") && p.contains("crash.rs"));
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn excerpt_fits() {
		let r = report("boom", "a.rs:1:1", "main", &"   12: some::deep::frame\n".repeat(400));
		let e = excerpt(&r, 1200);
		assert!(e.contains("Panic: boom") && e.ends_with("…\n"));
		assert!(e.len() < 1200);
	}
}
