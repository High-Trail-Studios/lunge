//! Lunge desktop app: a thin egui front end over `lunge-core`.

mod app;
mod style;
mod theme;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lunge")
            // Matches StartupWMClass in assets/lunge.desktop.
            .with_app_id("lunge")
            .with_inner_size([560.0, 680.0])
            .with_min_inner_size([380.0, 300.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lunge",
        options,
        Box::new(|cc| Ok(Box::new(app::LungeApp::new(&cc.egui_ctx)))),
    )
}
