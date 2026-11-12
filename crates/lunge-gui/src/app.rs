//! The main window: a sidebar of providers (AWS for now) listing SSO
//! sessions and access-key profiles, and a detail pane for the selection.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use lunge_core::cache::SessionState;
use lunge_core::config::{AccessKeyProfile, KeySource, SsoProfile};
use lunge_core::login::{AwsCli, LoginError, LoginProcess};
use lunge_core::time::OffsetDateTime;
use lunge_core::{Inventory, SessionEntry};

use crate::style;
use crate::theme::Theme;

/// Re-read the config and token cache this often, so states and countdowns
/// don't go stale while the window sits open. Each pass reads one small file
/// per session.
const RELOAD_INTERVAL: Duration = Duration::from_secs(30);

/// How often to check on a running `aws sso login`.
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(250);

const SETUP_COMMAND: &str = "aws configure sso";

pub struct LungeApp {
    inventory: Result<Inventory, String>,
    loaded_at: Instant,
    cli: AwsCli,
    /// At most one login runs at a time: each opens a browser tab, and
    /// concurrent CLI logins would be confusing to approve.
    login: Option<LoginProcess>,
    /// The latest login outcome per session name.
    notices: HashMap<String, Notice>,
    theme: Theme,
    selection: Selection,
    aws_expanded: bool,
}

/// What the detail pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    AddSession,
    Session(String),
    AccessKeyProfile(String),
    Issues,
}

enum Notice {
    Success(String),
    Failure(String),
    Info(String),
}

/// What the user asked for while the window was drawn.
enum Action {
    Select(Selection),
    Login(String),
    CancelLogin,
}

impl LungeApp {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut app = Self {
            inventory: load(),
            loaded_at: Instant::now(),
            cli: AwsCli::default(),
            login: None,
            notices: HashMap::new(),
            theme: Theme::new(ctx),
            selection: Selection::AddSession,
            aws_expanded: true,
        };
        app.selection = app.fallback_selection();
        app
    }

    fn reload(&mut self) {
        self.inventory = load();
        self.loaded_at = Instant::now();
        if !self.selection_exists() {
            self.selection = self.fallback_selection();
        }
    }

    /// The first SSO session, or the setup panel when there are none.
    fn fallback_selection(&self) -> Selection {
        match &self.inventory {
            Ok(inventory) => inventory
                .sessions
                .first()
                .map(|e| Selection::Session(e.session.name.clone()))
                .unwrap_or(Selection::AddSession),
            Err(_) => Selection::AddSession,
        }
    }

    fn selection_exists(&self) -> bool {
        let Ok(inventory) = &self.inventory else {
            return self.selection == Selection::AddSession;
        };
        match &self.selection {
            Selection::AddSession => true,
            Selection::Session(name) => inventory.sessions.iter().any(|e| e.session.name == *name),
            Selection::AccessKeyProfile(name) => inventory
                .access_key_profiles
                .iter()
                .any(|p| p.name == *name),
            Selection::Issues => !inventory.issues.is_empty(),
        }
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

    fn apply(&mut self, action: Action) {
        match action {
            Action::Select(selection) => self.selection = selection,
            Action::Login(session) => self.start_login(session),
            Action::CancelLogin => {
                if let Some(process) = &mut self.login {
                    process.cancel();
                }
            }
        }
    }
}

fn load() -> Result<Inventory, String> {
    // NO CREDENTIAL CUSTODY: read-only view of the AWS CLI's files.
    Inventory::load_default(OffsetDateTime::now_utc()).map_err(|e| e.to_string())
}

impl eframe::App for LungeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_login();
        self.theme.update(ui.ctx());
        if self.loaded_at.elapsed() >= RELOAD_INTERVAL {
            self.reload();
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Lunge");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.theme.picker(ui);
                    if ui
                        .button("Refresh")
                        .on_hover_text(
                            "Re-read your AWS config, credentials profile names, and SSO token cache",
                        )
                        .clicked()
                    {
                        self.reload();
                    }
                });
            });
        });

        let mut actions = Vec::new();
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(240.0)
            .size_range(190.0..=420.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(a) = self.sidebar(ui) {
                        actions.push(a);
                    }
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(a) = self.detail(ui) {
                    actions.push(a);
                }
            });
        });

        for action in actions {
            self.apply(action);
        }

        let ctx = ui.ctx();
        if self.login.is_some() {
            ctx.request_repaint_after(LOGIN_POLL_INTERVAL);
        } else {
            ctx.request_repaint_after(RELOAD_INTERVAL);
        }
    }
}

// ---------------------------------------------------------------- sidebar

impl LungeApp {
    fn sidebar(&mut self, ui: &mut Ui) -> Option<Action> {
        if provider_header(ui, "AWS", self.aws_expanded).clicked() {
            self.aws_expanded = !self.aws_expanded;
        }
        if !self.aws_expanded {
            return None;
        }
        ui.add_space(4.0);

        let mut action = None;
        let mut select = |ui: &mut Ui, selection: Selection, status: Option<Status>, text: &str| {
            let selected = self.selection == selection;
            if sidebar_row(ui, selected, status, text).clicked() {
                action = Some(Action::Select(selection));
            }
        };

        select(ui, Selection::AddSession, None, "+  Add SSO session");

        let Ok(inventory) = &self.inventory else {
            return action;
        };
        let dark = ui.visuals().dark_mode;

        section_label(ui, "SSO SESSIONS");
        if inventory.sessions.is_empty() {
            ui.label(RichText::new("None yet").weak().italics());
        }
        for entry in &inventory.sessions {
            let status = Status {
                color: style::status_color(&entry.state, dark),
                label: Some(entry.state.to_string()),
            };
            select(
                ui,
                Selection::Session(entry.session.name.clone()),
                Some(status),
                &entry.session.name,
            );
        }

        section_label(ui, "SINGLE ACCOUNTS");
        if inventory.access_key_profiles.is_empty() {
            ui.label(RichText::new("None found").weak().italics());
        }
        for profile in &inventory.access_key_profiles {
            select(
                ui,
                Selection::AccessKeyProfile(profile.name.clone()),
                None,
                &profile.name,
            );
        }

        if !inventory.issues.is_empty() {
            ui.add_space(10.0);
            let text = match inventory.issues.len() {
                1 => "1 config issue".to_string(),
                n => format!("{n} config issues"),
            };
            let status = Status {
                color: style::error_color(dark),
                label: None,
            };
            select(ui, Selection::Issues, Some(status), &text);
        }
        action
    }
}

/// A provider's accordion header: a full-width orange bar with a
/// disclosure triangle. Dark text on orange keeps contrast in both themes.
fn provider_header(ui: &mut Ui, name: &str, expanded: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::click());
    let fill = if response.hovered() {
        style::PROVIDER_ACCENT_HOVER
    } else {
        style::PROVIDER_ACCENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, fill);

    let ink = Color32::from_rgb(28, 25, 23);
    let center = egui::pos2(rect.left() + 14.0, rect.center().y);
    let s = 4.5;
    let triangle = if expanded {
        vec![
            center + egui::vec2(-s, -s * 0.6),
            center + egui::vec2(s, -s * 0.6),
            center + egui::vec2(0.0, s * 0.8),
        ]
    } else {
        vec![
            center + egui::vec2(-s * 0.6, -s),
            center + egui::vec2(s * 0.8, 0.0),
            center + egui::vec2(-s * 0.6, s),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        triangle,
        ink,
        egui::Stroke::NONE,
    ));
    painter.text(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(15.0),
        ink,
    );
    response.on_hover_text(if expanded { "Collapse" } else { "Expand" })
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).small().weak().strong());
}

/// A status dot, plus the state's name so color is never the only signal.
struct Status {
    color: Color32,
    label: Option<String>,
}

/// A clickable full-width sidebar entry, optionally led by a status dot and
/// ending with the status label.
fn sidebar_row(ui: &mut Ui, selected: bool, status: Option<Status>, text: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
    let visuals = ui.visuals();
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 4.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let text_color = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    let mut x = rect.left() + 8.0;
    if let Some(status) = &status {
        painter.circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.0, status.color);
        x += 14.0;
        if let Some(label) = &status.label {
            painter.text(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                label,
                egui::FontId::proportional(11.5),
                if selected { text_color } else { status.color },
            );
        }
    }
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(14.0),
        text_color,
    );
    response
}

// ---------------------------------------------------------------- detail

impl LungeApp {
    fn detail(&self, ui: &mut Ui) -> Option<Action> {
        ui.add_space(4.0);
        let inventory = match &self.inventory {
            Ok(inventory) => inventory,
            Err(message) => {
                load_error(ui, message);
                if self.selection == Selection::AddSession {
                    ui.add_space(16.0);
                    add_session_panel(ui);
                }
                return None;
            }
        };
        match &self.selection {
            Selection::AddSession => {
                add_session_panel(ui);
                None
            }
            Selection::Session(name) => {
                let entry = inventory
                    .sessions
                    .iter()
                    .find(|e| e.session.name == *name)?;
                let login = self.login.as_ref();
                SessionDetail {
                    entry,
                    now: OffsetDateTime::now_utc(),
                    login: login.filter(|p| p.session() == entry.session.name),
                    other_login_running: login.is_some_and(|p| p.session() != entry.session.name),
                    notice: self.notices.get(&entry.session.name),
                }
                .show(ui)
            }
            Selection::AccessKeyProfile(name) => {
                let profile = inventory
                    .access_key_profiles
                    .iter()
                    .find(|p| p.name == *name)?;
                access_key_detail(ui, profile);
                None
            }
            Selection::Issues => {
                ui.heading("Config issues");
                ui.label(
                    RichText::new("These entries are hidden until fixed in your AWS config.")
                        .weak(),
                );
                ui.add_space(8.0);
                for issue in &inventory.issues {
                    ui.label(format!("• {issue}"));
                }
                None
            }
        }
    }
}

fn load_error(ui: &mut Ui, message: &str) {
    let dark = ui.visuals().dark_mode;
    ui.heading("Can't read your AWS config");
    ui.label(RichText::new(message).color(style::error_color(dark)));
}

fn add_session_panel(ui: &mut Ui) {
    ui.heading("Add an SSO session");
    ui.add_space(4.0);
    ui.label(
        "Lunge shows the SSO sessions in your AWS config; the AWS CLI creates them. \
         Run this in a terminal and answer its prompts (a session name, your start URL, \
         and its region). It signs you in and adds a profile for the account and role you pick.",
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 6))
            .corner_radius(4.0)
            .fill(ui.visuals().extreme_bg_color)
            .show(ui, |ui| {
                ui.label(RichText::new(SETUP_COMMAND).monospace().size(15.0));
            });
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(SETUP_COMMAND.to_string());
        }
    });
    ui.add_space(8.0);
    ui.label("Then press Refresh. Run it again to add more accounts or roles to a session.");
}

fn access_key_detail(ui: &mut Ui, profile: &AccessKeyProfile) {
    ui.heading(&profile.name);
    let mut details = vec!["Access-key profile".to_string()];
    if let Some(region) = &profile.region {
        details.push(region.clone());
    }
    ui.label(RichText::new(details.join("  ·  ")).weak());
    ui.add_space(10.0);
    let file = match profile.source {
        KeySource::CredentialsFile => "~/.aws/credentials",
        KeySource::ConfigFile => "~/.aws/config",
    };
    ui.label(format!(
        "The AWS CLI keeps this profile's keys in {file}. Lunge reads only the profile name, never the keys."
    ));
    ui.add_space(6.0);
    ui.label(
        RichText::new("Sign-in and console access for access-key profiles aren't supported yet.")
            .italics(),
    );
}

struct SessionDetail<'a> {
    entry: &'a SessionEntry,
    now: OffsetDateTime,
    /// This session's login, if one is running.
    login: Option<&'a LoginProcess>,
    other_login_running: bool,
    notice: Option<&'a Notice>,
}

impl SessionDetail<'_> {
    fn show(&self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let session = &self.entry.session;
        ui.horizontal(|ui| {
            ui.heading(&session.name);
            ui.add_space(8.0);
            status_badge(ui, &self.entry.state);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                action = self.login_button(ui);
            });
        });
        ui.label(RichText::new(format!("{}  ·  {}", session.start_url, session.region)).weak());
        ui.label(state_detail(&self.entry.state, self.now));

        if let Some(process) = self.login {
            login_progress(ui, process);
        } else if let Some(notice) = self.notice {
            notice_label(ui, notice);
        }

        ui.add_space(14.0);
        ui.label(RichText::new("ACCOUNTS").small().weak().strong());
        ui.add_space(2.0);
        if session.profiles.is_empty() {
            ui.label(
                RichText::new(
                    "No profiles use this session yet. Run `aws configure sso` to add one.",
                )
                .weak()
                .italics(),
            );
        }
        for profile in &session.profiles {
            profile_row(ui, profile);
            ui.add_space(4.0);
        }
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
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size * 0.7, size), Sense::hover());
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
    egui::Frame::group(ui.style())
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
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
        });
}
