//! The "Rename profile" dialog: previews the one-line change to the AWS
//! config as you type, and writes it only when confirmed (the checks, backup,
//! and atomic write live in `lunge_core::rename`).

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use lunge_core::rename::{self, RenamePlan};

use crate::style;

pub struct RenameDialog {
    config_path: PathBuf,
    old_name: String,
    input: String,
    /// The plan for `planned_for`; recomputed whenever the input changes.
    plan: Result<RenamePlan, String>,
    planned_for: Option<String>,
    /// Why the last attempt to write failed, if it did.
    write_error: Option<String>,
    focus_pending: bool,
}

pub enum Outcome {
    Open,
    Cancelled,
    Renamed {
        old_name: String,
        new_name: String,
        backup: PathBuf,
    },
}

impl RenameDialog {
    pub fn new(config_path: PathBuf, old_name: String) -> Self {
        Self {
            config_path,
            input: old_name.clone(),
            old_name,
            plan: Err(String::new()),
            planned_for: None,
            write_error: None,
            focus_pending: true,
        }
    }

    fn refresh_plan(&mut self) {
        if self.planned_for.as_ref() == Some(&self.input) {
            return;
        }
        self.plan = if self.input == self.old_name {
            Err(String::new())
        } else {
            // Reads the config file; nothing is written until confirmed.
            rename::plan_profile_rename(&self.config_path, &self.old_name, &self.input)
                .map_err(|e| e.to_string())
        };
        self.planned_for = Some(self.input.clone());
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        self.refresh_plan();
        let mut outcome = Outcome::Open;
        let modal = egui::Modal::new(egui::Id::unique("rename-profile")).show(ctx, |ui| {
            ui.set_width(420.0);
            let dark = ui.visuals().dark_mode;
            ui.heading("Rename profile");
            ui.add_space(6.0);
            ui.label(RichText::new(&self.old_name).weak());
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .hint_text("new profile name")
                    .desired_width(f32::INFINITY),
            );
            if self.focus_pending {
                edit.request_focus();
                self.focus_pending = false;
            }
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(8.0);
            match &self.plan {
                Ok(plan) => {
                    ui.label(format!("Changes one line in {}:", plan.target().display()));
                    egui::Frame::new()
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .fill(ui.visuals().extreme_bg_color)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(&plan.before_line)
                                    .monospace()
                                    .strikethrough()
                                    .weak(),
                            );
                            ui.label(RichText::new(&plan.after_line).monospace());
                        });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "A backup of the file is saved next to it first. Scripts or \
                             AWS_PROFILE settings that use the old name will need updating.",
                        )
                        .weak(),
                    );
                }
                Err(message) if !message.is_empty() => {
                    ui.colored_label(style::error_color(dark), message);
                }
                Err(_) => {
                    ui.label(
                        RichText::new(
                            "Type a new name. Lunge shows the change before writing anything.",
                        )
                        .weak(),
                    );
                }
            }
            if let Some(error) = &self.write_error {
                ui.add_space(4.0);
                ui.colored_label(style::error_color(dark), error);
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let can_rename = self.plan.is_ok();
                let clicked = ui
                    .add_enabled(can_rename, egui::Button::new("Rename"))
                    .clicked();
                if ui.button("Cancel").clicked() {
                    outcome = Outcome::Cancelled;
                }
                if can_rename && (clicked || enter) {
                    outcome = self.apply();
                }
            });
        });
        if modal.should_close() && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }

    fn apply(&mut self) -> Outcome {
        let Ok(plan) = &self.plan else {
            return Outcome::Open;
        };
        match plan.apply() {
            Ok(backup) => Outcome::Renamed {
                old_name: plan.old_name.clone(),
                new_name: plan.new_name.clone(),
                backup,
            },
            Err(e) => {
                self.write_error = Some(e.to_string());
                // Re-check against the file as it is now.
                self.planned_for = None;
                Outcome::Open
            }
        }
    }
}
