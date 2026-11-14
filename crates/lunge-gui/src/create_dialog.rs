//! The "Name this account" dialog: creates a profile for an account role,
//! pre-filled with a suggested name. The AWS CLI does the writing
//! (`aws configure set`, via `lunge_core::profiles`); the dialog shows the
//! exact commands first.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, RichText};
use lunge_core::config::{self, SsoSession};
use lunge_core::login::AwsCli;
use lunge_core::profiles::NewProfile;
use lunge_core::{naming, rename};

use crate::style;

pub struct CreateProfileDialog {
    cli: AwsCli,
    config_path: PathBuf,
    credentials_path: Option<PathBuf>,
    session: String,
    account_id: String,
    account_name: String,
    role_name: String,
    name: String,
    region: String,
    /// Profile names already in use when the dialog opened.
    taken: Vec<String>,
    creating: Option<Receiver<Result<(), String>>>,
    error: Option<String>,
    focus_pending: bool,
}

pub enum Outcome {
    Open,
    Cancelled,
    Created { name: String },
}

impl CreateProfileDialog {
    pub fn new(
        cli: AwsCli,
        config_path: PathBuf,
        credentials_path: Option<PathBuf>,
        session: &SsoSession,
        account_id: String,
        account_name: String,
        role_name: String,
    ) -> Self {
        let taken = config::all_profile_names(&config_path, credentials_path.as_deref())
            .unwrap_or_default();
        let name = naming::suggest_profile_name(&session.name, &account_name, &role_name, |n| {
            taken.iter().any(|t| t == n)
        });
        Self {
            cli,
            config_path,
            credentials_path,
            session: session.name.clone(),
            account_id,
            account_name,
            role_name,
            name,
            region: session.region.clone(),
            taken,
            creating: None,
            error: None,
            focus_pending: true,
        }
    }

    /// Why the current input can't be used, if it can't.
    fn problem(&self) -> Option<String> {
        if let Err(e) = rename::validate_name(&self.name) {
            return Some(e.to_string());
        }
        if self.taken.contains(&self.name) {
            return Some(format!("a profile named \"{}\" already exists", self.name));
        }
        let region_ok = self
            .region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !region_ok {
            return Some("region should look like us-east-1 (or leave it empty)".into());
        }
        None
    }

    fn commands(&self) -> Vec<String> {
        let mut settings = vec![
            ("sso_account_id", self.account_id.as_str()),
            ("sso_role_name", self.role_name.as_str()),
        ];
        if !self.region.is_empty() {
            settings.push(("region", self.region.as_str()));
        }
        settings.push(("sso_session", self.session.as_str()));
        settings
            .into_iter()
            .map(|(k, v)| format!("aws configure set {k} {v} --profile {}", self.name))
            .collect()
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let cli = self.cli.clone();
        let config_path = self.config_path.clone();
        let credentials_path = self.credentials_path.clone();
        let new = NewProfile {
            name: self.name.clone(),
            session: self.session.clone(),
            account_id: self.account_id.clone(),
            role_name: self.role_name.clone(),
            region: Some(self.region.clone()).filter(|r| !r.is_empty()),
        };
        let ctx = ctx.clone();
        // PREFER THE CLI: `aws configure set` writes the profile.
        std::thread::spawn(move || {
            let result = cli
                .create_sso_profile(&config_path, credentials_path.as_deref(), &new)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.error = None;
        self.creating = Some(rx);
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        if let Some(rx) = &self.creating
            && let Ok(result) = rx.try_recv()
        {
            self.creating = None;
            match result {
                Ok(()) => {
                    return Outcome::Created {
                        name: self.name.clone(),
                    };
                }
                Err(e) => self.error = Some(e),
            }
        }

        let busy = self.creating.is_some();
        let mut outcome = Outcome::Open;
        let modal = egui::Modal::new(egui::Id::unique("create-profile")).show(ctx, |ui| {
            ui.set_width(460.0);
            let dark = ui.visuals().dark_mode;
            ui.heading("Name this account");
            ui.label(
                RichText::new(format!(
                    "{}  ·  {}  ·  {}",
                    self.account_name, self.account_id, self.role_name
                ))
                .weak(),
            );
            ui.add_space(8.0);

            ui.label("Profile name");
            let edit = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY),
            );
            if self.focus_pending {
                edit.request_focus();
                self.focus_pending = false;
            }
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
            ui.label("Default region (optional)");
            ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.region).desired_width(160.0),
            );

            ui.add_space(8.0);
            let problem = self.problem();
            match &problem {
                Some(message) => {
                    ui.colored_label(style::error_color(dark), message);
                }
                None => {
                    ui.label("Lunge will run:");
                    egui::Frame::new()
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .fill(ui.visuals().extreme_bg_color)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            for command in self.commands() {
                                ui.add(
                                    egui::Label::new(RichText::new(command).monospace().small())
                                        .wrap(),
                                );
                            }
                        });
                    ui.label(
                        RichText::new(
                            "The AWS CLI adds the profile to your AWS config; Lunge doesn't edit the file.",
                        )
                        .weak(),
                    );
                }
            }
            if let Some(error) = &self.error {
                ui.add_space(4.0);
                ui.colored_label(style::error_color(dark), error);
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let can_create = problem.is_none() && !busy;
                let clicked = ui
                    .add_enabled(can_create, egui::Button::new("Create profile"))
                    .clicked();
                if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                    outcome = Outcome::Cancelled;
                }
                if busy {
                    ui.spinner();
                    ui.label("Running aws configure set…");
                }
                if can_create && (clicked || enter) {
                    self.start(ui.ctx());
                }
            });
        });
        if !busy && modal.should_close() && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }
}
