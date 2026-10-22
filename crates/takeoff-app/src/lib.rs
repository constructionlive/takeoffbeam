//! Takeoff Desk desktop app (egui). The binary is a thin `main.rs`.

pub mod ai_panel;
pub mod app;
pub mod canvas;
pub mod docstate;
pub mod geom;
pub mod graph_view;
pub mod icons;
pub mod left_panel;
pub mod map_panel;
pub mod markups;
pub mod right_panel;
pub mod settings;
pub mod theme;
pub mod tools;
pub mod updates;
pub mod widgets;
pub mod worker;

use eframe::egui;

pub struct EframeApp(pub app::TakeoffApp);

impl eframe::App for EframeApp {
	fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
		self.0.frame(ui);
	}

	fn on_exit(&mut self) {
		self.0.on_exit();
	}

	fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
		visuals.panel_fill.to_normalized_gamma_f32()
	}
}
