//! Light/dark theme: follows the OS by default, with a manual override that
//! lasts for the session (Lunge persists no settings).

use std::sync::mpsc::{self, Receiver};

use dark_light::Mode;
use eframe::egui::{self, ThemePreference};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    System,
    Light,
    Dark,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

pub struct Theme {
    choice: Choice,
    /// The OS preference, if it could be detected.
    os_mode: Option<Mode>,
    os_changes: Option<Receiver<Mode>>,
}

impl Theme {
    pub fn new(ctx: &egui::Context) -> Self {
        let theme = Self {
            choice: Choice::System,
            os_mode: dark_light::detect().ok(),
            os_changes: watch_os_theme(ctx.clone()),
        };
        theme.apply(ctx);
        theme
    }

    /// Picks up OS theme changes reported since the last frame.
    pub fn update(&mut self, ctx: &egui::Context) {
        let Some(changes) = &self.os_changes else {
            return;
        };
        if let Some(mode) = changes.try_iter().last() {
            self.os_mode = Some(mode);
            self.apply(ctx);
        }
    }

    pub fn picker(&mut self, ui: &mut egui::Ui) {
        let before = self.choice;
        egui::ComboBox::from_id_salt("theme")
            .selected_text(format!("Theme: {}", self.choice.label()))
            .show_ui(ui, |ui| {
                for choice in [Choice::System, Choice::Light, Choice::Dark] {
                    ui.selectable_value(&mut self.choice, choice, choice.label());
                }
            });
        if self.choice != before {
            self.apply(ui.ctx());
        }
    }

    fn apply(&self, ctx: &egui::Context) {
        ctx.set_theme(match self.choice {
            Choice::Light => ThemePreference::Light,
            Choice::Dark => ThemePreference::Dark,
            Choice::System => match self.os_mode {
                Some(Mode::Dark) => ThemePreference::Dark,
                // "No preference" from the desktop means the app default:
                // light, as on most desktops.
                Some(Mode::Light | Mode::Unspecified) => ThemePreference::Light,
                // Detection failed; let egui use whatever the windowing
                // system reports.
                None => ThemePreference::System,
            },
        });
    }
}

/// Forwards OS theme changes and wakes the UI so they apply immediately.
fn watch_os_theme(ctx: egui::Context) -> Option<Receiver<Mode>> {
    let watcher = dark_light::subscribe().ok()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mode in watcher.iter() {
            if tx.send(mode).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    Some(rx)
}
