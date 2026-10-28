//! Update tracker: the app checks a small JSON feed we publish
//! (`scripts/publish-update.sh`) and tells the user when a newer version is
//! out, with what's new. One click downloads the installer; a second quits,
//! swaps the new version in and starts it again (see [`install_on_exit`]).
//!
//! Feed (`latest.json`):
//! { "version": "0.0.2", "date": "2026-11-01", "notes": "…", "critical": false,
//!   "downloads": { "macos-arm64": "https://…dmg", "windows-x64": "https://…exe" },
//!   "page": "https://github.com/constructionlive/takeoffbeam/releases" }

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;

/// Where we publish updates. `TAKEOFF_UPDATE_URL` overrides (testing, self-hosting).
pub const FEED_URL: &str = "https://pub-061018a047d54079b309fb16a26a1ce0.r2.dev/takeoff-desk/latest.json";
/// Fallback when the feed is unreachable: the newest GitHub release.
pub const GITHUB_LATEST: &str = "https://api.github.com/repos/constructionlive/takeoffbeam/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/constructionlive/takeoffbeam/releases";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Release {
	pub version: String,
	#[serde(default)]
	pub date: String,
	#[serde(default)]
	pub notes: String,
	/// Security or data-loss fix: shown more prominently and can't be skipped.
	#[serde(default)]
	pub critical: bool,
	#[serde(default)]
	pub downloads: std::collections::BTreeMap<String, String>,
	#[serde(default)]
	pub page: String,
}

impl Release {
	pub fn download_for_this_platform(&self) -> Option<&String> {
		self.downloads.get(platform_key())
	}
}

pub fn platform_key() -> &'static str {
	match (std::env::consts::OS, std::env::consts::ARCH) {
		("macos", "aarch64") => "macos-arm64",
		("macos", _) => "macos-x64",
		("windows", _) => "windows-x64",
		("linux", "aarch64") => "linux-arm64",
		_ => "linux-x64",
	}
}

/// "0.10.2" > "0.9.9"; non-numeric parts compare as 0.
pub fn is_newer(candidate: &str, current: &str) -> bool {
	let parse = |v: &str| -> Vec<u64> { v.trim().trim_start_matches('v').split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect() };
	let (a, b) = (parse(candidate), parse(current));
	for i in 0..3 {
		let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
		if x != y {
			return x > y;
		}
	}
	false
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadState {
	Idle,
	Downloading { done: u64, total: u64 },
	Done(PathBuf),
	Failed(String),
}

#[derive(Default)]
struct Shared {
	latest: Option<Release>,
	checking: bool,
	last_error: Option<String>,
	checked_once: bool,
	download: Option<DownloadState>,
}

pub struct Updater {
	shared: Arc<Mutex<Shared>>,
	last_check: Option<Instant>,
	/// The user asked from the menu: say "you're up to date" too.
	pub manual: bool,
	pub dismissed: bool,
	/// Why the last in-place install could not start (the banner offers the installer instead).
	pub install_error: Option<String>,
}

fn client() -> reqwest::blocking::Client {
	reqwest::blocking::Client::builder()
		.user_agent(concat!("TakeoffDesk/", env!("CARGO_PKG_VERSION")))
		.timeout(Duration::from_secs(20))
		.build()
		.expect("http client")
}

fn fetch() -> Result<Release, String> {
	let url = std::env::var("TAKEOFF_UPDATE_URL").unwrap_or_else(|_| FEED_URL.into());
	let feed = client().get(&url).send().and_then(|r| r.error_for_status()).and_then(|r| r.json::<Release>());
	match feed {
		Ok(r) => Ok(r),
		Err(feed_err) => {
			// Fallback: GitHub's latest release (tag + attached installers).
			#[derive(Deserialize)]
			struct Asset {
				name: String,
				browser_download_url: String,
			}
			#[derive(Deserialize)]
			struct Gh {
				tag_name: String,
				#[serde(default)]
				body: Option<String>,
				#[serde(default)]
				published_at: Option<String>,
				html_url: String,
				#[serde(default)]
				assets: Vec<Asset>,
			}
			let gh: Gh = client()
				.get(GITHUB_LATEST)
				.send()
				.and_then(|r| r.error_for_status())
				.and_then(|r| r.json())
				.map_err(|e| format!("update feed: {feed_err}; GitHub: {e}"))?;
			let mut downloads = std::collections::BTreeMap::new();
			for a in gh.assets {
				let key = if a.name.ends_with("-arm64.dmg") {
					"macos-arm64"
				} else if a.name.ends_with(".dmg") {
					"macos-x64"
				} else if a.name.ends_with("-setup.exe") {
					"windows-x64"
				} else {
					continue;
				};
				downloads.insert(key.to_string(), a.browser_download_url);
			}
			Ok(Release {
				version: gh.tag_name.trim_start_matches('v').to_string(),
				date: gh.published_at.unwrap_or_default().chars().take(10).collect(),
				notes: gh.body.unwrap_or_default(),
				critical: false,
				downloads,
				page: gh.html_url,
			})
		}
	}
}

impl Updater {
	pub fn new() -> Self {
		Self { shared: Arc::new(Mutex::new(Shared::default())), last_check: None, manual: false, dismissed: false, install_error: None }
	}

	/// Kick off a background check now.
	pub fn check(&mut self, ctx: &eframe::egui::Context) {
		if self.shared.lock().checking {
			return;
		}
		self.last_check = Some(Instant::now());
		self.shared.lock().checking = true;
		let shared = self.shared.clone();
		let ctx = ctx.clone();
		std::thread::spawn(move || {
			let r = fetch();
			let mut s = shared.lock();
			s.checking = false;
			s.checked_once = true;
			match r {
				Ok(rel) => {
					s.latest = Some(rel);
					s.last_error = None;
				}
				Err(e) => s.last_error = Some(e),
			}
			ctx.request_repaint();
		});
	}

	/// Call every frame: checks on start and every few hours.
	pub fn tick(&mut self, ctx: &eframe::egui::Context) {
		if std::env::var("TAKEOFF_NO_UPDATE_CHECK").is_ok() {
			return;
		}
		if self.last_check.is_none_or(|t| t.elapsed() > CHECK_EVERY) {
			self.check(ctx);
		}
		if matches!(self.shared.lock().download, Some(DownloadState::Downloading { .. })) {
			ctx.request_repaint_after(Duration::from_millis(200));
		}
	}

	pub fn checking(&self) -> bool {
		self.shared.lock().checking
	}

	pub fn checked_once(&self) -> bool {
		self.shared.lock().checked_once
	}

	pub fn error(&self) -> Option<String> {
		self.shared.lock().last_error.clone()
	}

	/// A newer release than this build, if any (and not skipped).
	pub fn available(&self, skipped: &str) -> Option<Release> {
		let s = self.shared.lock();
		let r = s.latest.as_ref()?;
		(is_newer(&r.version, env!("CARGO_PKG_VERSION")) && (r.critical || r.version != skipped)).then(|| r.clone())
	}

	pub fn download_state(&self) -> DownloadState {
		self.shared.lock().download.clone().unwrap_or(DownloadState::Idle)
	}

	/// Download the installer into ~/Downloads, ready for [`install_on_exit`].
	/// Falls back to the browser when there is no direct link.
	pub fn download_and_open(&mut self, rel: &Release, ctx: &eframe::egui::Context) {
		let Some(url) = rel.download_for_this_platform().cloned() else {
			let _ = open::that(if rel.page.is_empty() { RELEASES_PAGE } else { &rel.page });
			return;
		};
		let shared = self.shared.clone();
		shared.lock().download = Some(DownloadState::Downloading { done: 0, total: 0 });
		let ctx = ctx.clone();
		std::thread::spawn(move || {
			let result = (|| -> Result<PathBuf, String> {
				use std::io::{Read, Write};
				let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("TakeoffDesk-update").to_string();
				let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
				let dst = dir.join(&name);
				let tmp = dir.join(format!("{name}.part"));
				let mut resp = reqwest::blocking::Client::builder()
					.user_agent(concat!("TakeoffDesk/", env!("CARGO_PKG_VERSION")))
					.build()
					.map_err(|e| e.to_string())?
					.get(&url)
					.send()
					.and_then(|r| r.error_for_status())
					.map_err(|e| e.to_string())?;
				let total = resp.content_length().unwrap_or(0);
				let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
				let mut buf = vec![0u8; 1 << 16];
				let mut done = 0u64;
				loop {
					let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
					if n == 0 {
						break;
					}
					f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
					done += n as u64;
					shared.lock().download = Some(DownloadState::Downloading { done, total });
				}
				f.sync_all().map_err(|e| e.to_string())?;
				drop(f);
				std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
				Ok(dst)
			})();
			let state = match result {
				Ok(p) => DownloadState::Done(p),
				Err(e) => DownloadState::Failed(e),
			};
			shared.lock().download = Some(state);
			ctx.request_repaint();
		});
	}
}

/// Hand the downloaded installer to a helper that waits for this process to
/// exit, installs it over this copy and starts the new version. `Ok` means the
/// helper is waiting and the app should close; on `Err` nothing has changed and
/// the caller can open the installer for the user instead.
pub fn install_on_exit(installer: &std::path::Path) -> Result<(), String> {
	stage(installer)
}

/// macOS: mount the dmg, check the new app is signed by the same team as this
/// one, copy it next to this bundle, then swap the two once we have quit.
#[cfg(target_os = "macos")]
fn stage(installer: &std::path::Path) -> Result<(), String> {
	use std::path::Path;
	use std::process::{Command, Stdio};

	fn run(cmd: &str, args: &[&std::ffi::OsStr]) -> Result<std::process::Output, String> {
		let out = Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
		if out.status.success() { Ok(out) } else { Err(format!("{cmd}: {}", String::from_utf8_lossy(&out.stderr).trim())) }
	}
	// "TeamIdentifier=…" from codesign -dv; None for ad-hoc (local) builds.
	fn team(app: &Path) -> Option<String> {
		let out = Command::new("codesign").arg("-dv").arg(app).output().ok()?;
		let text = String::from_utf8_lossy(&out.stderr).into_owned();
		text.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).filter(|t| *t != "not set").map(str::to_string)
	}

	let exe = std::env::current_exe().map_err(|e| e.to_string())?;
	let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).ok_or("not running from an .app bundle")?.to_path_buf();
	if bundle.to_string_lossy().contains("/AppTranslocation/") {
		return Err("Takeoff Desk is running from the disk image or Downloads; move it to Applications first".into());
	}
	let parent = bundle.parent().ok_or("no folder above the app")?;
	let staged = parent.join(".Takeoff Desk.app.update");
	let mnt = std::env::temp_dir().join(format!("takeoff-update-{}", std::process::id()));
	std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
	run("hdiutil", &["attach".as_ref(), installer.as_os_str(), "-nobrowse".as_ref(), "-readonly".as_ref(), "-noautoopen".as_ref(), "-mountpoint".as_ref(), mnt.as_os_str()])?;
	let copied = (|| -> Result<(), String> {
		let new_app = std::fs::read_dir(&mnt)
			.map_err(|e| e.to_string())?
			.filter_map(|e| e.ok().map(|e| e.path()))
			.find(|p| p.extension().is_some_and(|e| e == "app"))
			.ok_or("no app in the disk image")?;
		run("codesign", &["--verify".as_ref(), "--deep".as_ref(), "--strict".as_ref(), new_app.as_os_str()]).map_err(|e| format!("the downloaded app's signature is not valid ({e})"))?;
		if let Some(ours) = team(&bundle) {
			if team(&new_app).as_deref() != Some(ours.as_str()) {
				return Err("the downloaded app is not signed by the same developer".into());
			}
		}
		let _ = std::fs::remove_dir_all(&staged);
		run("ditto", &[new_app.as_os_str(), staged.as_os_str()])?;
		Ok(())
	})();
	let _ = run("hdiutil", &["detach".as_ref(), mnt.as_os_str(), "-force".as_ref()]);
	let _ = std::fs::remove_dir(&mnt);
	if let Err(e) = copied {
		let _ = std::fs::remove_dir_all(&staged);
		return Err(e);
	}
	// $1 pid, $2 this bundle, $3 the staged copy. Puts the old one back if the swap fails.
	const SWAP: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
old="$2.old.$$"
if mv "$2" "$old"; then
	if mv "$3" "$2"; then rm -rf "$old"; else mv "$old" "$2"; fi
fi
open "$2""#;
	Command::new("/bin/sh")
		.args(["-c", SWAP, "takeoff-update"])
		.arg(std::process::id().to_string())
		.arg(&bundle)
		.arg(&staged)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.map_err(|e| e.to_string())?;
	Ok(())
}

/// Windows: once we have quit, run the setup silently (it keeps the install
/// mode and folder of the copy already there), then start the app again.
#[cfg(windows)]
fn stage(installer: &std::path::Path) -> Result<(), String> {
	use std::os::windows::process::CommandExt;
	const CREATE_NO_WINDOW: u32 = 0x0800_0000;
	let exe = std::env::current_exe().map_err(|e| e.to_string())?;
	let quote = |p: &std::path::Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
	let script = format!(
		"Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Process -FilePath {} -ArgumentList '/S' -Wait; Start-Process -FilePath {}",
		std::process::id(),
		quote(installer),
		quote(&exe)
	);
	std::process::Command::new("powershell")
		.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
		.creation_flags(CREATE_NO_WINDOW)
		.spawn()
		.map_err(|e| e.to_string())?;
	Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn stage(_installer: &std::path::Path) -> Result<(), String> {
	Err("updating in place is not supported on this system".into())
}

/// mailto: link for the Feedback button, with version and system details filled in.
pub fn feedback_mailto(model: &str) -> String {
	let subject = format!("Takeoff Desk feedback (v{})", env!("CARGO_PKG_VERSION"));
	let body = format!(
		"What happened, or what would you like to see?\n\n\n\n---\nTakeoff Desk {}\n{} {}\nModel: {}\n",
		env!("CARGO_PKG_VERSION"),
		std::env::consts::OS,
		std::env::consts::ARCH,
		if model.is_empty() { "none" } else { model }
	);
	format!("mailto:rahul@construction.live?subject={}&body={}", enc(&subject), enc(&body))
}

/// Feedback with a crash report in it: the head of the report and where the whole file is.
pub fn crash_mailto(model: &str, report: &str, file: &std::path::Path) -> String {
	let subject = format!("Takeoff Desk crash report (v{})", env!("CARGO_PKG_VERSION"));
	// Windows hands mailto: to the mail app through a ~2 KB command line.
	let budget = if cfg!(windows) { 1100 } else { 6000 };
	let body = format!(
		"What were you doing when it closed?\n\n\n\n---\n{}\nThe full report is {} (attach it if you can).\nModel: {}\n",
		crate::crash::excerpt(report, budget),
		file.display(),
		if model.is_empty() { "none" } else { model }
	);
	format!("mailto:rahul@construction.live?subject={}&body={}", enc(&subject), enc(&body))
}

fn enc(s: &str) -> String {
	let mut out = String::new();
	for b in s.bytes() {
		match b {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
			_ => out.push_str(&format!("%{b:02X}")),
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn versions() {
		assert!(is_newer("0.0.2", "0.0.1"));
		assert!(is_newer("v0.10.0", "0.9.9"));
		assert!(!is_newer("0.0.1", "0.0.1"));
		assert!(!is_newer("0.0.1", "0.1.0"));
		assert!(is_newer("1.0.0", "0.99.99"));
	}

	#[test]
	fn mailto_is_encoded() {
		let m = feedback_mailto("local:Qwen 27B");
		assert!(m.starts_with("mailto:rahul@construction.live?subject=Takeoff%20Desk%20feedback"));
		assert!(m.contains("Model%3A%20local%3AQwen%2027B"));
	}

	#[test]
	fn crash_mail() {
		let r = crate::crash::report("boom", "a.rs:1:1", "main", &"   12: some::deep::frame\n".repeat(400));
		let m = crash_mailto("", &r, std::path::Path::new("/tmp/logs/crash.txt"));
		assert!(m.starts_with("mailto:rahul@construction.live?subject=Takeoff%20Desk%20crash%20report"));
		assert!(m.contains("Panic%3A%20boom") && m.contains("crash.txt"));
		assert!(m.len() < 8000);
	}

	#[test]
	fn feed_parses() {
		let r: Release = serde_json::from_str(r#"{"version":"0.0.2","notes":"Faster","downloads":{"macos-arm64":"https://x/a.dmg"}}"#).unwrap();
		assert_eq!(r.version, "0.0.2");
		assert!(!r.critical);
	}
}
