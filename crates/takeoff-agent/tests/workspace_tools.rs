//! The workspace tools against a real one-page PDF: `bash` runs in the
//! workspace with the drawing's path in its environment, and overlays land in
//! the takeoff, show up in the agent's own renders, and can be removed.

use std::sync::Arc;

use parking_lot::Mutex;
use pdfium_render::prelude::*;
use serde_json::json;
use takeoff_agent::tools::{self, ToolCtx};
use takeoff_core::{Document, TakeoffData};

fn blank_pdf(dir: &std::path::Path) -> std::path::PathBuf {
	let pdfium = takeoff_pdf::pdfium().unwrap();
	let mut doc = pdfium.create_new_pdf().unwrap();
	doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(PdfPoints::new(792.0), PdfPoints::new(612.0))).unwrap();
	let path = dir.join("drawing.pdf");
	doc.save_to_file(&path).unwrap();
	path
}

fn setup(name: &str) -> (ToolCtx, std::path::PathBuf) {
	let root = std::env::temp_dir().join(format!("takeoff-ws-{name}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&root);
	std::fs::create_dir_all(&root).unwrap();
	let pdf = blank_pdf(&root);
	let ws = root.join("workspace");
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), vec![(792.0, 612.0)], "t")));
	let mut ctx = ToolCtx::new(doc, takeoff_pdf::Drawing::open(&pdf).unwrap(), "claude-test", None);
	ctx.workspace = Some(ws.clone());
	(ctx, ws)
}

#[test]
fn bash_runs_in_the_workspace() {
	let (mut ctx, ws) = setup("bash");
	let cmd = if cfg!(windows) { "Set-Content out.txt hi; (Get-Location).Path; $env:TAKEOFF_PDF" } else { "echo hi > out.txt && pwd && echo \"$TAKEOFF_PDF\" && exit 3" };
	let out = tools::call(&mut ctx, "bash", json!({ "command": cmd })).unwrap();
	assert!(ws.join("out.txt").is_file(), "file written in the workspace: {}", out.text);
	assert!(out.text.contains("drawing.pdf"), "TAKEOFF_PDF is set: {}", out.text);
	if !cfg!(windows) {
		assert!(out.text.starts_with("Exit code 3."), "{}", out.text);
	}
}

#[test]
fn bash_times_out() {
	let (mut ctx, _) = setup("timeout");
	let cmd = if cfg!(windows) { "Start-Sleep 5" } else { "sleep 5" };
	let out = tools::call(&mut ctx, "bash", json!({ "command": cmd, "timeout_secs": 1 })).unwrap();
	assert!(out.text.contains("timed out"), "{}", out.text);
}

#[test]
fn no_workspace_no_bash() {
	let (mut ctx, _) = setup("off");
	ctx.workspace = None;
	assert!(tools::call(&mut ctx, "bash", json!({ "command": "echo hi" })).is_err());
	let names: Vec<String> = tools::openai_tools(false).iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
	assert!(!names.contains(&"bash".to_string()));
	assert!(names.contains(&"add_overlay".to_string()), "SVG overlays work without a workspace");
}

#[test]
fn svg_overlay_is_saved_seen_and_removable() {
	let (mut ctx, _) = setup("svg");
	let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></svg>"##;
	let out = tools::call(&mut ctx, "add_overlay", json!({ "svg": svg, "region": {"x": 0.5, "y": 0.0, "w": 0.5, "h": 1.0}, "opacity": 0.5, "label": "Zone A" })).unwrap();
	assert_eq!(out.images.len(), 1, "check render attached: {}", out.text);
	let id = ctx.doc.lock().data.overlays[0].id.clone();
	assert_eq!(ctx.doc.lock().data.overlays[0].label.as_deref(), Some("Zone A"));

	// The agent's own view shows it: right half pink, left half white.
	let v = tools::call(&mut ctx, "view", json!({})).unwrap();
	let img = image::load_from_memory(&v.images[0].png).unwrap().to_rgba8();
	let (w, h) = img.dimensions();
	let left = img.get_pixel(w / 4, h / 2).0;
	let right = img.get_pixel(3 * w / 4, h / 2).0;
	assert!(left[1] > 240, "left untouched: {left:?}");
	assert!(right[0] > 240 && right[1] < 160 && right[1] > 100, "right half-transparent red: {right:?}");

	// And can leave it out.
	let v = tools::call(&mut ctx, "view", json!({ "show_overlays": false })).unwrap();
	let img = image::load_from_memory(&v.images[0].png).unwrap().to_rgba8();
	assert!(img.get_pixel(3 * w / 4, h / 2).0[1] > 240);

	tools::call(&mut ctx, "update_overlays", json!({ "items": [{ "id": id, "delete": true }] })).unwrap();
	assert!(ctx.doc.lock().data.overlays.is_empty());
}

#[test]
fn overlay_from_a_workspace_file_and_bad_input() {
	let (mut ctx, ws) = setup("file");
	std::fs::create_dir_all(&ws).unwrap();
	std::fs::write(ws.join("zones.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 3"><circle cx="2" cy="1.5" r="1" fill="blue"/></svg>"#).unwrap();
	tools::call(&mut ctx, "add_overlay", json!({ "file": "zones.svg" })).unwrap();
	assert!(ctx.doc.lock().data.overlays[0].svg.as_deref().unwrap().contains("circle"));
	assert!(tools::call(&mut ctx, "add_overlay", json!({ "svg": "<not svg" })).is_err(), "invalid SVG is refused");
	assert!(tools::call(&mut ctx, "add_overlay", json!({ "svg": "<svg/>", "file": "zones.svg" })).is_err(), "two sources are refused");
	assert_eq!(ctx.doc.lock().data.overlays.len(), 1);

	// A view with no box: the overlay covers what that view shows.
	let v = tools::call(&mut ctx, "view", json!({ "region": {"x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5} })).unwrap().images[0].view_id.clone();
	tools::call(&mut ctx, "add_overlay", json!({ "view": v, "svg": "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 1'/>" })).unwrap();
	let o = ctx.doc.lock().data.overlays[1].clone();
	assert!(o.x > 0.0 && o.w < 1.0, "placed on the view's region: {o:?}");
}

#[test]
fn html_overlay_renders_with_a_local_browser() {
	if tools::workspace::find_browser().is_none() {
		eprintln!("no Chrome/Edge here; skipping");
		return;
	}
	let (mut ctx, _) = setup("html");
	let out = tools::call(&mut ctx, "add_overlay", json!({ "html": "<body style='margin:0'><div style='width:100vw;height:100vh;background:#00ff00'></div></body>", "region": {"x": 0.0, "y": 0.0, "w": 0.5, "h": 0.5}, "opacity": 1.0 }));
	let out = out.unwrap();
	let o = ctx.doc.lock().data.overlays[0].clone();
	let png = o.image.expect("rendered to an image");
	let img = image::open(&png).unwrap().to_rgba8();
	let c = img.get_pixel(img.width() / 2, img.height() / 2).0;
	assert!(c[1] > 200 && c[0] < 60, "rendered the HTML: {c:?} ({})", out.text);
}
