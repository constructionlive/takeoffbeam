//! The agent's workspace folder and what it does there: `bash` runs shell
//! commands in it, and overlays (SVG, images, HTML rendered to an image) are
//! laid over the drawing at some opacity to show maps, diagrams and
//! correlations on the sheet itself.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use takeoff_core::{new_id, Overlay, Source};

use super::items::ai_meta;
use super::render::{evidence, RegionArg};
use super::{view_box_to_page, ToolCtx, ToolOutput};
use crate::AgentEvent;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const STREAM_LIMIT: usize = 12_000;

fn workspace(ctx: &ToolCtx) -> Result<&Path, String> {
	ctx.workspace.as_deref().ok_or_else(|| "the AI workspace folder is turned off (Takeoff Desk → Settings → AI workspace)".to_string())
}

// ── bash ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct BashArgs {
	pub command: String,
	pub timeout_secs: Option<u64>,
}

/// The shell the command runs in, with a PATH that finds Homebrew, pip and
/// npm tools even when the app was opened from the Finder / Start menu.
fn shell(command: &str) -> Command {
	let mut c = if cfg!(windows) {
		let mut c = Command::new("powershell.exe");
		c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
		c
	} else {
		let mut c = Command::new("/bin/bash");
		c.args(["-c", command]);
		c
	};
	if let Ok(p) = std::env::join_paths(crate::channel::search_dirs()) {
		c.env("PATH", p);
	}
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
	}
	c
}

/// Keep the tail of a long stream (errors and results come last).
fn tail(s: &str) -> String {
	if s.len() <= STREAM_LIMIT {
		return s.to_string();
	}
	let cut = s.char_indices().map(|(i, _)| i).find(|&i| i >= s.len() - STREAM_LIMIT).unwrap_or(0);
	format!("… ({} bytes cut)\n{}", cut, &s[cut..])
}

pub fn bash(ctx: &mut ToolCtx, a: BashArgs) -> Result<ToolOutput, String> {
	let dir = workspace(ctx)?.to_path_buf();
	std::fs::create_dir_all(&dir).map_err(|e| format!("could not create the workspace {}: {e}", dir.display()))?;
	let timeout = Duration::from_secs(a.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS));
	let mut cmd = shell(&a.command);
	cmd.current_dir(&dir)
		.env("TAKEOFF_WORKSPACE", &dir)
		.env("TAKEOFF_PDF", ctx.drawing.path())
		.env("TAKEOFF_PAGE", ctx.current_page.to_string())
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());
	let mut child = cmd.spawn().map_err(|e| format!("could not start the shell: {e}"))?;
	let pipe = |r: Option<Box<dyn Read + Send>>| {
		std::thread::spawn(move || {
			let mut buf = Vec::new();
			if let Some(mut r) = r {
				let _ = r.read_to_end(&mut buf);
			}
			String::from_utf8_lossy(&buf).into_owned()
		})
	};
	let out = pipe(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
	let err = pipe(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
	let started = Instant::now();
	let (status, stopped) = loop {
		if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
			break (Some(s), None);
		}
		if ctx.cancelled() {
			let _ = child.kill();
			let _ = child.wait();
			break (None, Some("stopped by the user"));
		}
		if started.elapsed() > timeout {
			let _ = child.kill();
			let _ = child.wait();
			break (None, Some("timed out"));
		}
		std::thread::sleep(Duration::from_millis(50));
	};
	let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
	let head = match (status, stopped) {
		(_, Some(why)) => format!("Command {why} after {}s.", started.elapsed().as_secs()),
		(Some(s), None) => format!("Exit code {}.", s.code().map(|c| c.to_string()).unwrap_or_else(|| "none (killed by a signal)".into())),
		(None, None) => unreachable!(),
	};
	let mut text = head;
	if !stdout.trim().is_empty() {
		text.push_str(&format!("\n--- stdout ---\n{}", tail(stdout.trim_end())));
	}
	if !stderr.trim().is_empty() {
		text.push_str(&format!("\n--- stderr ---\n{}", tail(stderr.trim_end())));
	}
	if stdout.trim().is_empty() && stderr.trim().is_empty() {
		text.push_str(" (no output)");
	}
	Ok(ToolOutput::text(text))
}

// ── Overlays ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddOverlayArgs {
	pub page: Option<u32>,
	pub region: Option<RegionArg>,
	pub view: Option<String>,
	#[serde(rename = "box")]
	pub bbox: Option<Value>,
	pub svg: Option<String>,
	pub html: Option<String>,
	pub file: Option<String>,
	pub opacity: Option<f64>,
	pub label: Option<String>,
	/// Long edge, in px, an HTML overlay is rendered at.
	pub width_px: Option<u32>,
}

/// A path the model gave: relative ones are in the workspace.
fn resolve(ctx: &ToolCtx, p: &str) -> Result<PathBuf, String> {
	let path = PathBuf::from(p);
	let path = if path.is_absolute() { path } else { workspace(ctx)?.join(path) };
	if !path.is_file() {
		return Err(format!("no file at {}", path.display()));
	}
	Ok(path)
}

pub fn add_overlay(ctx: &mut ToolCtx, a: AddOverlayArgs) -> Result<ToolOutput, String> {
	let (page, (x, y, w, h)) = match (&a.view, &a.bbox) {
		(Some(v), Some(b)) => {
			let v = ctx.view(v)?;
			(v.page, view_box_to_page(&v, b)?)
		}
		// No box: cover what that view shows.
		(Some(v), None) => {
			let v = ctx.view(v)?;
			(v.page, (v.region.x, v.region.y, v.region.w, v.region.h))
		}
		_ => {
			let page = ctx.page_or_current(a.page)?;
			let r = a.region.unwrap_or(RegionArg { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
			if r.w <= 0.0 || r.h <= 0.0 {
				return Err("region must have positive w and h (page fractions 0-1)".into());
			}
			(page, (r.x, r.y, r.w, r.h))
		}
	};
	let sources = [a.svg.is_some(), a.html.is_some(), a.file.is_some()].iter().filter(|b| **b).count();
	if sources != 1 {
		return Err("give exactly one of svg (markup), html (markup) or file (a .svg, .png, .jpg or .html in the workspace)".into());
	}
	let id = new_id("ov");
	let (svg, image) = if let Some(svg) = a.svg {
		(Some(svg), None)
	} else if let Some(html) = a.html {
		let dir = workspace(ctx)?.join("overlays");
		std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
		let src = dir.join(format!("{id}.html"));
		std::fs::write(&src, html).map_err(|e| e.to_string())?;
		(None, Some(render_html(ctx, &src, &id, page, (w, h), a.width_px)?))
	} else {
		let path = resolve(ctx, a.file.as_deref().unwrap_or_default())?;
		match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
			Some("svg") => (Some(std::fs::read_to_string(&path).map_err(|e| e.to_string())?), None),
			Some("png" | "jpg" | "jpeg") => (None, Some(path.to_string_lossy().into_owned())),
			Some("html" | "htm") => (None, Some(render_html(ctx, &path, &id, page, (w, h), a.width_px)?)),
			_ => return Err("file must be .svg, .png, .jpg or .html".into()),
		}
	};
	let overlay = Overlay {
		id: id.clone(),
		page,
		x,
		y,
		w,
		h,
		svg,
		image,
		opacity: a.opacity.unwrap_or(0.6).clamp(0.05, 1.0),
		label: a.label,
		hidden: false,
		source: Some(Source::Ai),
		meta: ai_meta(),
	};
	// Fail now, not silently in the viewer.
	takeoff_pdf::overlay::rasterize(&overlay, 64, 64)?;
	ctx.doc.lock().edit("AI: add overlay", |d| d.overlays.push(overlay));
	ctx.emit(AgentEvent::DocChanged);
	let mut out = ToolOutput::text(format!(
		"Added overlay {id} on page {page} at x={x:.3} y={y:.3} w={w:.3} h={h:.3}. The estimator sees it over the sheet and can hide it or change its opacity; change or remove it with update_overlays."
	));
	if let Some(img) = evidence(ctx, page, std::slice::from_ref(&id), "add_overlay") {
		out.images.push(img);
	}
	Ok(out)
}

#[derive(Deserialize)]
pub struct OverlayChange {
	pub id: String,
	pub opacity: Option<f64>,
	pub hidden: Option<bool>,
	pub label: Option<String>,
	pub delete: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateOverlaysArgs {
	pub items: Vec<OverlayChange>,
}

pub fn update_overlays(ctx: &mut ToolCtx, a: UpdateOverlaysArgs) -> Result<ToolOutput, String> {
	let mut done = Vec::new();
	let mut missing = Vec::new();
	ctx.doc.lock().edit("AI: update overlays", |d| {
		for c in &a.items {
			if c.delete == Some(true) {
				let before = d.overlays.len();
				d.overlays.retain(|o| o.id != c.id);
				if d.overlays.len() < before { done.push(format!("{} deleted", c.id)) } else { missing.push(c.id.clone()) }
				continue;
			}
			let Some(o) = d.overlays.iter_mut().find(|o| o.id == c.id) else {
				missing.push(c.id.clone());
				continue;
			};
			if let Some(v) = c.opacity {
				o.opacity = v.clamp(0.0, 1.0);
			}
			if let Some(h) = c.hidden {
				o.hidden = h;
			}
			if let Some(l) = &c.label {
				o.label = Some(l.clone());
			}
			done.push(format!("{} updated", c.id));
		}
	});
	ctx.emit(AgentEvent::DocChanged);
	let mut text = if done.is_empty() { "Nothing changed.".to_string() } else { done.join(", ") + "." };
	if !missing.is_empty() {
		text.push_str(&format!(" Unknown overlay ids: {} (get_drawing lists them).", missing.join(", ")));
	}
	Ok(ToolOutput::text(text))
}

// ── HTML → PNG (headless Chrome / Edge) ────────────────────────────────────

/// A Chromium browser to render HTML with, if one is installed.
pub fn find_browser() -> Option<PathBuf> {
	let mut c: Vec<PathBuf> = Vec::new();
	if cfg!(target_os = "macos") {
		for app in ["Google Chrome", "Microsoft Edge", "Chromium", "Brave Browser", "Google Chrome Canary"] {
			let rel = format!("{app}.app/Contents/MacOS/{app}");
			c.push(PathBuf::from("/Applications").join(&rel));
			if let Some(h) = dirs::home_dir() {
				c.push(h.join("Applications").join(&rel));
			}
		}
	} else if cfg!(windows) {
		for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
			if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
				c.push(base.join("Google/Chrome/Application/chrome.exe"));
				c.push(base.join("Microsoft/Edge/Application/msedge.exe"));
			}
		}
	} else {
		for d in crate::channel::search_dirs() {
			for exe in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"] {
				c.push(d.join(exe));
			}
		}
	}
	c.into_iter().find(|p| p.is_file())
}

/// Render `html` to a transparent PNG sized to the overlay box's shape.
fn render_html(ctx: &ToolCtx, html: &Path, id: &str, page: u32, (fw, fh): (f64, f64), long_edge: Option<u32>) -> Result<String, String> {
	let browser = find_browser().ok_or("HTML overlays need Google Chrome or Microsoft Edge installed (headless rendering). Use svg instead.")?;
	let (pw, ph) = ctx.doc.lock().page_size_pt(page);
	let (bw, bh) = (fw * pw, fh * ph);
	let long = long_edge.unwrap_or(1600).clamp(200, 4096) as f64;
	let s = long / bw.max(bh);
	let (w, h) = ((bw * s).round().max(1.0) as u32, (bh * s).round().max(1.0) as u32);
	let out = html.with_file_name(format!("{id}.png"));
	let profile = std::env::temp_dir().join(format!("takeoff-desk-headless-{}", std::process::id()));
	let url = format!("file://{}", html.canonicalize().map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/").trim_start_matches("//?/"));
	let mut cmd = Command::new(&browser);
	cmd.args(["--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run", "--no-default-browser-check", "--force-device-scale-factor=1", "--default-background-color=00000000"])
		.arg(format!("--user-data-dir={}", profile.display()))
		.arg(format!("--window-size={w},{h}"))
		.arg(format!("--screenshot={}", out.display()))
		.arg(url)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	let _ = std::fs::remove_file(&out);
	let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", browser.display()))?;
	let started = Instant::now();
	loop {
		if child.try_wait().map_err(|e| e.to_string())?.is_some() {
			break;
		}
		if started.elapsed() > Duration::from_secs(60) || ctx.cancelled() {
			let _ = child.kill();
			let _ = child.wait();
			return Err("the headless browser did not finish rendering the HTML within 60s".into());
		}
		std::thread::sleep(Duration::from_millis(100));
	}
	let _ = std::fs::remove_dir_all(&profile);
	if !out.is_file() {
		return Err(format!("{} produced no image from {}; check the HTML opens in a browser.", browser.display(), html.display()));
	}
	Ok(out.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tail_keeps_the_end() {
		let s = "a".repeat(STREAM_LIMIT) + "END";
		let t = tail(&s);
		assert!(t.ends_with("END"));
		assert!(t.len() < s.len() + 40);
	}
}
