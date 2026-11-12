//! Lunge desktop app: a thin egui front end over `lunge-core`.

mod app;
mod rename_dialog;
mod style;
mod theme;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lunge")
            // Matches StartupWMClass in assets/lunge.desktop.
            .with_app_id("lunge")
            .with_inner_size([880.0, 600.0])
            .with_min_inner_size([600.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lunge",
        options,
        Box::new(|cc| Ok(Box::new(app::LungeApp::new(cc)))),
    )
}
