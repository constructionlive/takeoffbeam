//! Takeoff Desk: Bluebeam-style takeoff with a native AI agent.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use takeoff_app::{app::TakeoffApp, settings::Settings, EframeApp};

fn main() -> eframe::Result {
	// Claude Code / Codex start us as their MCP server: pipe to the running app.
	let args: Vec<String> = std::env::args().collect();
	if args.get(1).map(String::as_str) == Some(takeoff_agent::channel::BRIDGE_ARG) {
		std::process::exit(takeoff_agent::channel::run_bridge(args.get(2).map(String::as_str).unwrap_or("")));
	}
	let settings = Settings::load();
	let files: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).filter(|p: &std::path::PathBuf| p.exists()).collect();
	let icon = image::load_from_memory(include_bytes!("../../../assets/icon.png")).ok().map(|i| {
		let i = i.to_rgba8();
		egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
	});
	let mut viewport = egui::ViewportBuilder::default().with_title("Takeoff Desk").with_inner_size([1440.0, 900.0]).with_min_inner_size([960.0, 600.0]).with_drag_and_drop(true);
	if let Some(icon) = icon {
		viewport = viewport.with_icon(icon);
	}
	let options = eframe::NativeOptions { viewport, persist_window: true, ..Default::default() };
	eframe::run_native(
		"Takeoff Desk",
		options,
		Box::new(move |cc| {
			let mut app = TakeoffApp::new(&cc.egui_ctx, settings);
			for f in files {
				app.open(f, &cc.egui_ctx);
			}
			Ok(Box::new(EframeApp(app)))
		}),
	)
}
