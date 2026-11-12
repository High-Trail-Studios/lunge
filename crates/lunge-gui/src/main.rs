//! Lunge desktop app: a thin egui front end over `lunge-core`.

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lunge")
            // Matches StartupWMClass in assets/lunge.desktop.
            .with_app_id("lunge")
            .with_inner_size([480.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native("Lunge", options, Box::new(|_cc| Ok(Box::new(LungeApp))))
}

struct LungeApp;

impl eframe::App for LungeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |_ui| {});
    }
}
