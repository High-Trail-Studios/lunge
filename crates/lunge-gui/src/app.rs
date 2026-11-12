//! The main window: SSO sessions with their state, and the profiles under each.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Layout, RichText, Ui};
use lunge_core::cache::SessionState;
use lunge_core::config::SsoProfile;
use lunge_core::login::{AwsCli, LoginError, LoginProcess};
use lunge_core::time::OffsetDateTime;
use lunge_core::{Inventory, SessionEntry};

use crate::style;

/// Re-read the config and token cache this often, so states and countdowns
/// don't go stale while the window sits open. Each pass reads one small file
/// per session.
const RELOAD_INTERVAL: Duration = Duration::from_secs(30);

/// How often to check on a running `aws sso login`.
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct LungeApp {
    inventory: Result<Inventory, String>,
    loaded_at: Instant,
    cli: AwsCli,
    /// At most one login runs at a time: each opens a browser tab, and
    /// concurrent CLI logins would be confusing to approve.
    login: Option<LoginProcess>,
    /// The latest login outcome per session name.
    notices: HashMap<String, Notice>,
}

enum Notice {
    Success(String),
    Failure(String),
    Info(String),
}

/// What the user asked for while the window was drawn.
enum Action {
    Login(String),
    CancelLogin,
}

impl LungeApp {
    pub fn new() -> Self {
        Self {
            inventory: load(),
            loaded_at: Instant::now(),
            cli: AwsCli::default(),
            login: None,
            notices: HashMap::new(),
        }
    }

    fn reload(&mut self) {
        self.inventory = load();
        self.loaded_at = Instant::now();
    }

    fn start_login(&mut self, session: String) {
        let Ok(inventory) = &self.inventory else {
            return;
        };
        // NO CREDENTIAL CUSTODY: the CLI performs the login and writes the
        // token to its own cache; Lunge only re-reads that cache afterwards.
        match self.cli.start_login(&session, &inventory.cache_dir) {
            Ok(process) => {
                self.notices.remove(&session);
                self.login = Some(process);
            }
            Err(e) => {
                self.notices.insert(session, Notice::Failure(e.to_string()));
            }
        }
    }

    fn poll_login(&mut self) {
        let Some(process) = &mut self.login else {
            return;
        };
        let Some(result) = process.poll() else {
            return;
        };
        let session = process.session().to_string();
        let notice = match result {
            Ok(expires_at) => Notice::Success(format!(
                "Logged in. Token valid for {}.",
                style::human_duration(expires_at - OffsetDateTime::now_utc())
            )),
            Err(LoginError::Cancelled) => Notice::Info("Login cancelled.".into()),
            Err(e) => Notice::Failure(e.to_string()),
        };
        self.notices.insert(session, notice);
        self.login = None;
        self.reload();
    }
}

fn load() -> Result<Inventory, String> {
    // NO CREDENTIAL CUSTODY: read-only view of the AWS CLI's config and cache.
    Inventory::load_default(OffsetDateTime::now_utc()).map_err(|e| e.to_string())
}

impl eframe::App for LungeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_login();
        if self.loaded_at.elapsed() >= RELOAD_INTERVAL {
            self.reload();
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Lunge");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("Refresh")
                        .on_hover_text("Re-read ~/.aws/config and the SSO token cache")
                        .clicked()
                    {
                        self.reload();
                    }
                });
            });
        });

        let mut action = None;
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match &self.inventory {
                Err(message) => load_error(ui, message),
                Ok(inventory) => {
                    action = inventory_view(ui, inventory, self.login.as_ref(), &self.notices);
                }
            });
        });

        match action {
            Some(Action::Login(session)) => self.start_login(session),
            Some(Action::CancelLogin) => {
                if let Some(process) = &mut self.login {
                    process.cancel();
                }
            }
            None => {}
        }

        let ctx = ui.ctx();
        if self.login.is_some() {
            ctx.request_repaint_after(LOGIN_POLL_INTERVAL);
        } else {
            ctx.request_repaint_after(RELOAD_INTERVAL);
        }
    }
}

fn load_error(ui: &mut Ui, message: &str) {
    let dark = ui.visuals().dark_mode;
    ui.add_space(8.0);
    ui.label(RichText::new("Can't read your AWS config").strong());
    ui.label(RichText::new(message).color(style::error_color(dark)));
    ui.add_space(4.0);
    ui.label("Set up an SSO session with `aws configure sso`, then press Refresh.");
}

fn inventory_view(
    ui: &mut Ui,
    inventory: &Inventory,
    login: Option<&LoginProcess>,
    notices: &HashMap<String, Notice>,
) -> Option<Action> {
    if inventory.sessions.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("No SSO sessions configured").strong());
        ui.label(format!(
            "{} has no [sso-session] blocks. Add one with `aws configure sso`, then press Refresh.",
            inventory.config_path.display()
        ));
    }

    let now = OffsetDateTime::now_utc();
    let mut action = None;
    for entry in &inventory.sessions {
        let card = SessionCard {
            entry,
            now,
            login: login.filter(|p| p.session() == entry.session.name),
            other_login_running: login.is_some_and(|p| p.session() != entry.session.name),
            notice: notices.get(&entry.session.name),
        };
        if let Some(a) = card.show(ui) {
            action = Some(a);
        }
        ui.add_space(6.0);
    }

    if !inventory.issues.is_empty() {
        let dark = ui.visuals().dark_mode;
        let title = match inventory.issues.len() {
            1 => "1 config issue".to_string(),
            n => format!("{n} config issues"),
        };
        egui::CollapsingHeader::new(RichText::new(title).color(style::error_color(dark)))
            .id_salt("issues")
            .show(ui, |ui| {
                for issue in &inventory.issues {
                    ui.label(format!("• {issue}"));
                }
            });
    }
    action
}

struct SessionCard<'a> {
    entry: &'a SessionEntry,
    now: OffsetDateTime,
    /// This session's login, if one is running.
    login: Option<&'a LoginProcess>,
    other_login_running: bool,
    notice: Option<&'a Notice>,
}

impl SessionCard<'_> {
    fn show(&self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let session = &self.entry.session;
        egui::Frame::group(ui.style())
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    status_badge(ui, &self.entry.state);
                    ui.label(RichText::new(&session.name).strong().size(16.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        action = self.login_button(ui);
                    });
                });
                ui.label(
                    RichText::new(format!("{}  ·  {}", session.start_url, session.region)).weak(),
                );
                ui.label(state_detail(&self.entry.state, self.now));

                if let Some(process) = self.login {
                    login_progress(ui, process);
                } else if let Some(notice) = self.notice {
                    notice_label(ui, notice);
                }

                if session.profiles.is_empty() {
                    ui.label(
                        RichText::new("No profiles use this session.")
                            .weak()
                            .italics(),
                    );
                }
                for profile in &session.profiles {
                    ui.separator();
                    profile_row(ui, profile);
                }
            });
        action
    }

    fn login_button(&self, ui: &mut Ui) -> Option<Action> {
        if self.login.is_some() {
            return ui
                .button("Cancel")
                .on_hover_text("Stop waiting for this login")
                .clicked()
                .then_some(Action::CancelLogin);
        }
        let label = match self.entry.state {
            SessionState::Live { .. } => "Log in again",
            _ => "Log in",
        };
        let button = ui.add_enabled(!self.other_login_running, egui::Button::new(label));
        let button = if self.other_login_running {
            button.on_disabled_hover_text("Another login is in progress")
        } else {
            button.on_hover_text(
                "Run `aws sso login` for this session; your browser opens to approve it",
            )
        };
        button
            .clicked()
            .then(|| Action::Login(self.entry.session.name.clone()))
    }
}

/// While a login runs: a spinner, and whatever the CLI printed (the URL to
/// open if the browser didn't, or the device code), selectable for copying.
fn login_progress(ui: &mut Ui, process: &LoginProcess) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label("Waiting for you to approve the login in your browser…");
    });
    let output = process.output();
    if !output.is_empty() {
        egui::Frame::new()
            .inner_margin(6.0)
            .fill(ui.visuals().extreme_bg_color)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for line in output.iter().filter(|l| !l.trim().is_empty()) {
                    ui.add(egui::Label::new(RichText::new(line).monospace().small()).wrap());
                }
            });
    }
}

fn notice_label(ui: &mut Ui, notice: &Notice) {
    let dark = ui.visuals().dark_mode;
    match notice {
        Notice::Success(text) => ui.colored_label(style::success_color(dark), text),
        Notice::Failure(text) => ui.colored_label(style::error_color(dark), text),
        Notice::Info(text) => ui.label(RichText::new(text).weak()),
    };
}

/// A colored dot plus a text label, so state never depends on color alone.
fn status_badge(ui: &mut Ui, state: &SessionState) {
    let color = style::status_color(state, ui.visuals().dark_mode);
    let size = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size * 0.7, size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.3, color);
    ui.label(RichText::new(state.to_string()).color(color).strong());
}

fn state_detail(state: &SessionState, now: OffsetDateTime) -> String {
    match state {
        SessionState::Live { expires_at } => {
            format!(
                "Token expires in {}",
                style::human_duration(*expires_at - now)
            )
        }
        SessionState::Renewable { expired_at } => format!(
            "Token expired {} ago; the AWS CLI will likely renew it on next use",
            style::human_duration(now - *expired_at)
        ),
        SessionState::Expired { expired_at } => {
            format!("Expired {} ago", style::human_duration(now - *expired_at))
        }
        SessionState::NotLoggedIn => "No token cached for this session".into(),
        SessionState::Unreadable(reason) => format!("Token cache unreadable: {reason}"),
    }
}

fn profile_row(ui: &mut Ui, profile: &SsoProfile) {
    ui.label(RichText::new(&profile.name).strong());
    let mut details = Vec::new();
    if let Some(account) = &profile.account_id {
        details.push(format!("account {account}"));
    }
    if let Some(role) = &profile.role_name {
        details.push(role.clone());
    }
    if let Some(region) = &profile.region {
        details.push(region.clone());
    }
    if details.is_empty() {
        details.push("token-only profile (no account or role)".into());
    }
    ui.label(RichText::new(details.join("  ·  ")).weak());
}
