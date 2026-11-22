//! The "Add an SSO session" pane: a form asking what `aws configure sso`
//! asks before sign-in, plus a drawer (top right, hidden by default) with
//! the terminal route and the exact CLI input Save will use.
//!
//! PREFER THE CLI: Save pipes the answers into `aws configure sso-session`
//! (see `lunge_core::sessions`), which writes the config; Lunge then runs
//! `aws sso login`. The account and role questions come after sign-in, in
//! the session's account list, because AWS only lists them once you're
//! signed in.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Align, Layout, RichText, Ui};
use lunge_core::login::AwsCli;
use lunge_core::sessions::{self, NewSession};

use crate::style;

/// The all-in-one terminal route, for people who'd rather answer prompts.
const SETUP_COMMAND: &str = "aws configure sso";

/// Below this pane width the drawer stacks above the form.
const SIDE_BY_SIDE_MIN_WIDTH: f32 = 640.0;

pub struct AddSessionForm {
    session: NewSession,
    saving: Option<Receiver<Result<String, String>>>,
    error: Option<String>,
}

/// Where the session would be written and what's already there.
pub struct Target {
    /// `None` when the config location can't be worked out (no home folder).
    pub config_path: Option<PathBuf>,
    /// The config exists but couldn't be read; the CLI can't safely add to
    /// it until it's fixed.
    pub config_broken: bool,
    /// Session names already in the config, complete or not.
    pub taken: Vec<String>,
}

impl Default for AddSessionForm {
    fn default() -> Self {
        Self {
            session: NewSession {
                scopes: sessions::DEFAULT_SCOPES.into(),
                ..NewSession::default()
            },
            saving: None,
            error: None,
        }
    }
}

impl AddSessionForm {
    /// The new session's name once the CLI has written it; the form resets.
    /// Call every frame so a save finishing off-screen isn't missed.
    pub fn poll(&mut self) -> Option<String> {
        let result = match self.saving.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("the save stopped unexpectedly; check your AWS config".into())
            }
        };
        self.saving = None;
        match result {
            Ok(name) => {
                *self = Self::default();
                Some(name)
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, show_cli: &mut bool, cli: &AwsCli, target: &Target) {
        ui.horizontal(|ui| {
            ui.heading("Add an SSO session");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, hover) = if *show_cli {
                    ("Hide CLI", "Hide the AWS CLI route")
                } else {
                    (
                        "Show CLI",
                        "Show the terminal command, and exactly what Save passes to the AWS CLI",
                    )
                };
                if ui
                    .selectable_label(*show_cli, label)
                    .on_hover_text(hover)
                    .clicked()
                {
                    *show_cli = !*show_cli;
                }
            });
        });
        ui.add_space(4.0);

        if !*show_cli {
            self.form(ui, cli, target);
            return;
        }
        let width = ui.available_width();
        if width < SIDE_BY_SIDE_MIN_WIDTH {
            self.cli_drawer(ui);
            ui.add_space(12.0);
            self.form(ui, cli, target);
            return;
        }
        let gap = 16.0;
        let drawer = (width * 0.42).clamp(280.0, 440.0);
        let form = width - drawer - gap - ui.spacing().item_spacing.x;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(form, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(form);
                self.form(ui, cli, target);
            });
            ui.add_space(gap);
            ui.allocate_ui_with_layout(
                egui::vec2(drawer, 0.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(drawer);
                    self.cli_drawer(ui);
                },
            );
        });
    }

    /// Why Save is unavailable, if it is.
    fn problem(&self, target: &Target) -> Option<String> {
        if target.config_path.is_none() {
            return Some("can't tell where your AWS config lives (no home folder found)".into());
        }
        if target.config_broken {
            return Some("fix the AWS config error above first".into());
        }
        if let Err(message) = self.session.validate() {
            return Some(message);
        }
        let [name, ..] = self.session.answers();
        if target.taken.contains(&name) {
            return Some(format!("an SSO session named \"{name}\" already exists"));
        }
        None
    }

    fn form(&mut self, ui: &mut Ui, cli: &AwsCli, target: &Target) {
        let busy = self.saving.is_some();
        ui.add(
            egui::Label::new(
                "Answer the questions aws configure sso asks. Lunge hands your answers to \
                 the AWS CLI, which writes your AWS config, then signs you in.",
            )
            .wrap(),
        );
        ui.add_space(10.0);

        let field_width = ui.available_width().min(460.0);
        let s = &mut self.session;
        field(
            ui,
            busy,
            "Session name",
            &mut s.name,
            "work",
            field_width,
            "Your name for this sign-in. Profiles refer to it, and you can use it as \
             --sso-session NAME.",
        );
        field(
            ui,
            busy,
            "Start URL",
            &mut s.start_url,
            "https://my-org.awsapps.com/start",
            field_width,
            "Your AWS access portal URL, from IAM Identity Center or your administrator.",
        );
        field(
            ui,
            busy,
            "SSO region",
            &mut s.region,
            "us-east-1",
            field_width.min(200.0),
            "Where your IAM Identity Center runs, which may differ from where you work.",
        );
        field(
            ui,
            busy,
            "Registration scopes",
            &mut s.scopes,
            sessions::DEFAULT_SCOPES,
            field_width,
            "Leave as sso:account:access unless your administrator says otherwise. \
             Separate several with commas.",
        );

        ui.add_space(6.0);
        let problem = self.problem(target);
        let dark = ui.visuals().dark_mode;
        ui.horizontal(|ui| {
            let can_save = problem.is_none() && !busy;
            let save = ui
                .add_enabled(can_save, egui::Button::new("Save and sign in"))
                .on_hover_text(
                    "Run aws configure sso-session with these answers, then aws sso login",
                );
            if busy {
                ui.spinner();
                ui.label("Running aws configure sso-session…");
            } else if let Some(problem) = &problem {
                ui.label(RichText::new(problem).weak());
            }
            if can_save
                && save.clicked()
                && let Some(config_path) = target.config_path.clone()
            {
                self.start(ui.ctx(), cli.clone(), config_path);
            }
        });
        if let Some(error) = &self.error {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(error).color(style::error_color(dark))).wrap());
        }

        ui.add_space(14.0);
        ui.label(RichText::new("AFTER YOU SAVE").small().weak().strong());
        for step in [
            "1.  The AWS CLI adds the session to your AWS config.",
            "2.  Lunge runs aws sso login; your browser opens to approve it.",
            "3.  Your accounts and roles appear here. Name one to create a profile \
             (profile name, default region, and output format).",
        ] {
            ui.add(egui::Label::new(RichText::new(step).weak()).wrap());
        }
    }

    fn start(&mut self, ctx: &egui::Context, cli: AwsCli, config_path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let new = self.session.clone();
        let ctx = ctx.clone();
        // PREFER THE CLI: `aws configure sso-session` writes the block. No
        // credentials are involved.
        std::thread::spawn(move || {
            let result = cli
                .create_sso_session(&config_path, &new)
                .map(|()| new.answers()[0].clone())
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.error = None;
        self.saving = Some(rx);
    }

    fn cli_drawer(&self, ui: &mut Ui) {
        egui::Frame::new()
            .inner_margin(12.0)
            .corner_radius(6.0)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .fill(ui.visuals().faint_bg_color)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Prefer a terminal?").strong());
                ui.add(
                    egui::Label::new(
                        "This asks the same questions, signs you in, and adds a profile for \
                         the account and role you pick.",
                    )
                    .wrap(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    code_block(ui, |ui| {
                        ui.label(RichText::new(SETUP_COMMAND).monospace().size(15.0));
                    });
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(SETUP_COMMAND.to_string());
                    }
                });
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Then press Refresh. Run it again to add more accounts or roles \
                             to a session.",
                        )
                        .weak(),
                    )
                    .wrap(),
                );

                ui.add_space(10.0);
                ui.separator();
                ui.label(RichText::new("What Save runs").strong());
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Lunge types your answers into the CLI's prompts, then checks \
                             your config holds exactly what you entered.",
                        )
                        .weak(),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
                let answers = self.session.answers();
                let name = if answers[0].is_empty() {
                    "NAME"
                } else {
                    answers[0].as_str()
                };
                code_block(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let line = |ui: &mut Ui, text: String| {
                        ui.add(egui::Label::new(RichText::new(text).monospace().small()).wrap());
                    };
                    line(ui, "$ aws configure sso-session".into());
                    for (prompt, answer) in sessions::PROMPTS.iter().zip(&answers) {
                        line(ui, format!("{prompt}: {answer}"));
                    }
                    line(ui, format!("$ aws sso login --sso-session {name}"));
                });
            });
    }
}

fn field(
    ui: &mut Ui,
    busy: bool,
    label: &str,
    value: &mut String,
    hint: &str,
    width: f32,
    help: &str,
) {
    ui.label(RichText::new(label).strong());
    ui.add_enabled(
        !busy,
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(width),
    );
    ui.add(egui::Label::new(RichText::new(help).weak().small()).wrap());
    ui.add_space(8.0);
}

fn code_block(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .corner_radius(4.0)
        .fill(ui.visuals().extreme_bg_color)
        .show(ui, add);
}
