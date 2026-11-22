//! The main window: a sidebar of providers (AWS for now) listing SSO
//! sessions and access-key profiles, and a detail pane for the selection.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use lunge_core::accounts::{self, AccountRole, AccountRow, SortColumn};
use lunge_core::cache::SessionState;
use lunge_core::config::{self, AccessKeyProfile, ConfigIssue, KeySource};
use lunge_core::login::{AwsCli, LoginError, LoginProcess};
use lunge_core::profiles::{self, Shell};
use lunge_core::regions;
use lunge_core::time::OffsetDateTime;
use lunge_core::{Inventory, SessionEntry};

use crate::add_session::{self, AddSessionForm};
use crate::create_dialog::{self, CreateProfileDialog};
use crate::rename_dialog::{self, RenameDialog};
use crate::style;
use crate::theme::{self, Theme};

/// Re-read the config and token cache this often, so states and countdowns
/// don't go stale while the window sits open. Each pass reads one small file
/// per session.
const RELOAD_INTERVAL: Duration = Duration::from_secs(30);

/// How often to check on a running `aws sso login`.
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How often to redraw while idle, so "Updated …" stays roughly current.
const IDLE_REPAINT_INTERVAL: Duration = Duration::from_secs(10);

/// Toolbar logo: "Lunge" wordmark and mark on brand navy, rendered 4x
/// oversize so it stays sharp when zoomed. Built by
/// assets/brand/make_logos.py (the master logo, tagline removed).
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo-toolbar.png");
const LOGO_HEIGHT: f32 = 32.0;

/// Zoom levels offered in the status bar. Ctrl +/-/0 also work.
const ZOOM_PRESETS: [f32; 4] = [1.0, 1.25, 1.5, 2.0];
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.5..=3.0;

/// Display preferences remembered between launches, in eframe's app
/// storage (`~/.local/share/lunge/app.ron` on Linux). Holds no AWS data.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Prefs {
    zoom: f32,
    theme: theme::Choice,
    /// Whether the Add SSO session pane shows its CLI drawer.
    show_cli: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            theme: theme::Choice::System,
            show_cli: false,
        }
    }
}

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
    /// Mirrors the egui zoom factor (which Ctrl +/-/0 also change) for saving.
    zoom: f32,
    /// The open rename dialog, with the session whose profile it renames.
    rename: Option<(String, RenameDialog)>,
    /// The open "name this account" dialog, with its session.
    create: Option<(String, CreateProfileDialog)>,
    /// Each session's full account list from AWS, fetched while it's Live.
    /// Kept in memory only; never saved.
    accounts: HashMap<String, Accounts>,
    /// Filter text for the selected session's account list.
    account_filter: String,
    /// The account list's sort column, and whether it's descending.
    account_sort: (SortColumn, bool),
    /// A profile region change the CLI is writing; one at a time.
    region_update: Option<RegionUpdate>,
    /// The Add SSO session form; kept while you look elsewhere.
    add_session: AddSessionForm,
    show_cli: bool,
    logo: egui::TextureHandle,
}

struct RegionUpdate {
    session: String,
    profile: String,
    region: String,
    result: Receiver<Result<(), String>>,
}

enum Accounts {
    Loading(Receiver<Result<Vec<AccountRole>, String>>),
    Loaded(Vec<AccountRole>),
    Failed(String),
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
    /// Rename a profile of the given session.
    Rename {
        session: String,
        profile: String,
    },
    /// Create a profile for an account role that has none.
    NameAccount {
        session: String,
        account_id: String,
        account_name: String,
        role_name: String,
    },
    RefreshAccounts(String),
    /// Set a profile's default region (through the CLI).
    SetRegion {
        session: String,
        profile: String,
        region: String,
    },
    /// Text was copied to the clipboard; say so under the session.
    Copied {
        session: String,
        text: String,
    },
}

/// What the detail pane needs to draw the Add SSO session form.
struct AddSessionPane<'a> {
    form: &'a mut AddSessionForm,
    show_cli: &'a mut bool,
    cli: &'a AwsCli,
}

impl LungeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let prefs: Prefs = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        let zoom = prefs.zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
        let ctx = &cc.egui_ctx;
        ctx.set_zoom_factor(zoom);
        let mut app = Self {
            inventory: load(),
            loaded_at: Instant::now(),
            cli: AwsCli::default(),
            login: None,
            notices: HashMap::new(),
            theme: Theme::new(ctx, prefs.theme),
            selection: Selection::AddSession,
            aws_expanded: true,
            zoom,
            rename: None,
            create: None,
            accounts: HashMap::new(),
            account_filter: String::new(),
            // Profile first: blanks sort last, so unnamed accounts fall to the bottom.
            account_sort: (SortColumn::Profile, false),
            region_update: None,
            add_session: AddSessionForm::default(),
            show_cli: prefs.show_cli,
            logo: load_logo(ctx),
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
        // A fresh login can change what the session may access.
        self.accounts.remove(&session);
        self.notices.insert(session, notice);
        self.login = None;
        self.reload();
    }

    /// When the Add SSO session form's CLI run finishes: show the new
    /// session and sign in to it, so its accounts can be listed.
    fn poll_add_session(&mut self) {
        let Some(name) = self.add_session.poll() else {
            return;
        };
        self.reload();
        self.account_filter.clear();
        self.selection = Selection::Session(name.clone());
        if self.login.is_none() {
            self.start_login(name);
        } else {
            let notice = Notice::Info(
                "Session added. Log in once the other login finishes to list your accounts.".into(),
            );
            self.notices.insert(name, notice);
        }
    }

    /// Starts listing the selected session's accounts if it's Live and
    /// hasn't been listed yet.
    fn ensure_accounts(&mut self, ctx: &egui::Context) {
        let (Ok(inventory), Selection::Session(name)) = (&self.inventory, &self.selection) else {
            return;
        };
        if self.accounts.contains_key(name) {
            return;
        }
        let Some(entry) = inventory.sessions.iter().find(|e| e.session.name == *name) else {
            return;
        };
        if !matches!(entry.state, SessionState::Live { .. }) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let session = entry.session.clone();
        let cache_dir = inventory.cache_dir.clone();
        let ctx = ctx.clone();
        // NO CREDENTIAL CUSTODY (owner-approved exception): the token is
        // read and used inside list_account_roles only; see lunge_core::accounts.
        std::thread::spawn(move || {
            let result =
                accounts::list_account_roles(&session, &cache_dir).map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.accounts.insert(name.clone(), Accounts::Loading(rx));
    }

    fn poll_accounts(&mut self) {
        for state in self.accounts.values_mut() {
            if let Accounts::Loading(rx) = state
                && let Ok(result) = rx.try_recv()
            {
                *state = match result {
                    Ok(list) => Accounts::Loaded(list),
                    Err(e) => Accounts::Failed(e),
                };
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Select(selection) => {
                if selection != self.selection {
                    self.account_filter.clear();
                }
                self.selection = selection;
            }
            Action::Login(session) => self.start_login(session),
            Action::CancelLogin => {
                if let Some(process) = &mut self.login {
                    process.cancel();
                }
            }
            Action::Rename { session, profile } => {
                if let Ok(inventory) = &self.inventory {
                    let dialog = RenameDialog::new(inventory.config_path.clone(), profile);
                    self.rename = Some((session, dialog));
                }
            }
            Action::NameAccount {
                session,
                account_id,
                account_name,
                role_name,
            } => {
                let Ok(inventory) = &self.inventory else {
                    return;
                };
                let Some(entry) = inventory
                    .sessions
                    .iter()
                    .find(|e| e.session.name == session)
                else {
                    return;
                };
                let dialog = CreateProfileDialog::new(
                    self.cli.clone(),
                    inventory.config_path.clone(),
                    inventory.credentials_path.clone(),
                    &entry.session,
                    account_id,
                    account_name,
                    role_name,
                );
                self.create = Some((session, dialog));
            }
            Action::RefreshAccounts(session) => {
                self.accounts.remove(&session);
            }
            Action::SetRegion {
                session,
                profile,
                region,
            } => self.start_region_update(session, profile, region),
            Action::Copied { session, text } => {
                let notice = Notice::Info(format!("Copied {} to the clipboard.", text.trim_end()));
                self.notices.insert(session, notice);
            }
        }
    }

    fn start_region_update(&mut self, session: String, profile: String, region: String) {
        let Ok(inventory) = &self.inventory else {
            return;
        };
        if self.region_update.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cli = self.cli.clone();
        let config_path = inventory.config_path.clone();
        let (name, code) = (profile.clone(), region.clone());
        // PREFER THE CLI: `aws configure set region` writes the change.
        std::thread::spawn(move || {
            let result = cli
                .set_profile_region(&config_path, &name, &code)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.notices.remove(&session);
        self.region_update = Some(RegionUpdate {
            session,
            profile,
            region,
            result: rx,
        });
    }

    fn poll_region_update(&mut self) {
        let Some(update) = &self.region_update else {
            return;
        };
        let Ok(result) = update.result.try_recv() else {
            return;
        };
        let notice = match result {
            Ok(()) => Notice::Success(format!(
                "Set {}'s default region to {}.",
                update.profile, update.region
            )),
            Err(e) => Notice::Failure(e),
        };
        self.notices.insert(update.session.clone(), notice);
        self.region_update = None;
        self.reload();
    }

    fn show_create_dialog(&mut self, ctx: &egui::Context) {
        let Some((session, dialog)) = &mut self.create else {
            return;
        };
        match dialog.show(ctx) {
            create_dialog::Outcome::Open => {}
            create_dialog::Outcome::Cancelled => self.create = None,
            create_dialog::Outcome::Created { name } => {
                let notice = Notice::Success(format!(
                    "Created profile {name}. Use it with --profile {name} or AWS_PROFILE={name}."
                ));
                self.notices.insert(session.clone(), notice);
                self.create = None;
                self.reload();
            }
        }
    }

    fn show_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some((session, dialog)) = &mut self.rename else {
            return;
        };
        match dialog.show(ctx) {
            rename_dialog::Outcome::Open => {}
            rename_dialog::Outcome::Cancelled => self.rename = None,
            rename_dialog::Outcome::Renamed {
                old_name,
                new_name,
                backup,
            } => {
                let notice = Notice::Success(format!(
                    "Renamed {old_name} to {new_name}. Backup: {}",
                    backup.display()
                ));
                self.notices.insert(session.clone(), notice);
                self.rename = None;
                self.reload();
            }
        }
    }
}

fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let rgba = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png)
        .expect("bundled logo is a valid PNG")
        .to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
}

fn load() -> Result<Inventory, String> {
    // NO CREDENTIAL CUSTODY: read-only view of the AWS CLI's files.
    Inventory::load_default(OffsetDateTime::now_utc()).map_err(|e| e.to_string())
}

impl eframe::App for LungeApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let prefs = Prefs {
            zoom: self.zoom,
            theme: self.theme.choice(),
            show_cli: self.show_cli,
        };
        eframe::set_value(storage, eframe::APP_KEY, &prefs);
    }

    /// Only `Prefs` and the window geometry are saved; egui's internal UI
    /// state stays in memory, so nothing derived from AWS files can end up
    /// on disk.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_login();
        self.poll_accounts();
        self.poll_add_session();
        self.poll_region_update();
        self.theme.update(ui.ctx());
        self.zoom = ui.ctx().zoom_factor();
        if self.loaded_at.elapsed() >= RELOAD_INTERVAL {
            self.reload();
        }

        // Brand navy in both themes: the logo is drawn on navy, so the header
        // matches it, and widgets in it use dark styling for contrast.
        let header = egui::Frame::new()
            .fill(style::BRAND_NAVY)
            .inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::top("toolbar").frame(header).show(ui, |ui| {
            ui.style_mut().visuals = egui::Visuals::dark();
            ui.horizontal(|ui| {
                let size = self.logo.size_vec2();
                let logo_size = egui::vec2(size.x * LOGO_HEIGHT / size.y, LOGO_HEIGHT);
                ui.add(
                    egui::Image::new((self.logo.id(), logo_size))
                        .alt_text("Lunge by High Trail Studios"),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.theme.picker(ui);
                    if ui
                        .button("Refresh")
                        .on_hover_text(
                            "Re-read your AWS config, credentials profile names, and SSO token cache",
                        )
                        .clicked()
                    {
                        self.accounts.clear();
                        self.reload();
                    }
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

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
        self.ensure_accounts(ui.ctx());
        self.show_rename_dialog(ui.ctx());
        self.show_create_dialog(ui.ctx());

        let ctx = ui.ctx();
        if self.login.is_some() || self.region_update.is_some() {
            ctx.request_repaint_after(LOGIN_POLL_INTERVAL);
        } else {
            ctx.request_repaint_after(IDLE_REPAINT_INTERVAL);
        }
    }
}

// ---------------------------------------------------------------- status bar

impl LungeApp {
    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let seconds = self.loaded_at.elapsed().as_secs();
            let updated = if seconds < 10 {
                "Updated just now".to_string()
            } else {
                format!("Updated {}s ago", seconds / 10 * 10)
            };
            ui.label(RichText::new(updated).weak())
                .on_hover_text(format!(
                    "Lunge re-reads your AWS files every {}s",
                    RELOAD_INTERVAL.as_secs()
                ));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ctx = ui.ctx().clone();
                let current = ctx.zoom_factor();
                // Right-to-left layout: add presets largest first.
                for zoom in ZOOM_PRESETS.iter().rev() {
                    let selected = (current - zoom).abs() < 0.01;
                    let label = format!("{:.0}%", zoom * 100.0);
                    if ui
                        .selectable_label(selected, label)
                        .on_hover_text("Ctrl + and Ctrl - zoom too; Ctrl 0 resets")
                        .clicked()
                    {
                        ctx.set_zoom_factor(*zoom);
                    }
                }
                if !ZOOM_PRESETS.iter().any(|z| (current - z).abs() < 0.01) {
                    ui.label(RichText::new(format!("{:.0}%", current * 100.0)).strong());
                }
                ui.label(RichText::new("Zoom").weak());
            });
        });
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
    fn detail(&mut self, ui: &mut Ui) -> Option<Action> {
        ui.add_space(4.0);
        let Self {
            inventory,
            selection,
            login,
            notices,
            accounts,
            account_filter,
            account_sort,
            region_update,
            add_session,
            show_cli,
            cli,
            ..
        } = self;
        let pane = AddSessionPane {
            form: add_session,
            show_cli,
            cli,
        };
        let inventory = match inventory {
            Ok(inventory) => &*inventory,
            Err(message) => {
                // No config yet is the normal first run: the form creates
                // one, so don't greet a new user with an error.
                let no_config = config::default_config_path().is_some_and(|p| !p.exists());
                if *selection == Selection::AddSession {
                    if !no_config {
                        load_error(ui, message);
                        ui.add_space(16.0);
                    }
                    add_session_pane(ui, pane, None);
                } else {
                    load_error(ui, message);
                }
                return None;
            }
        };
        match &*selection {
            Selection::AddSession => {
                add_session_pane(ui, pane, Some(inventory));
                None
            }
            Selection::Session(name) => {
                let entry = inventory
                    .sessions
                    .iter()
                    .find(|e| e.session.name == *name)?;
                let login = login.as_ref();
                SessionDetail {
                    entry,
                    now: OffsetDateTime::now_utc(),
                    login: login.filter(|p| p.session() == entry.session.name),
                    other_login_running: login.is_some_and(|p| p.session() != entry.session.name),
                    notice: notices.get(&entry.session.name),
                    accounts: accounts.get(&entry.session.name),
                    filter: account_filter,
                    sort: account_sort,
                    region_update: region_update.as_ref().map(|u| u.profile.as_str()),
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

/// The Add SSO session form. `inventory` is `None` when the config couldn't
/// be read: a missing config is fine (the CLI creates it), a broken one
/// isn't.
fn add_session_pane(ui: &mut Ui, pane: AddSessionPane<'_>, inventory: Option<&Inventory>) {
    let target = match inventory {
        Some(inventory) => add_session::Target {
            config_path: Some(inventory.config_path.clone()),
            config_broken: false,
            taken: inventory
                .sessions
                .iter()
                .map(|e| e.session.name.clone())
                .chain(inventory.issues.iter().filter_map(|issue| match issue {
                    ConfigIssue::SessionMissingKey { session, .. } => Some(session.clone()),
                    _ => None,
                }))
                .collect(),
        },
        None => {
            let config_path = config::default_config_path();
            add_session::Target {
                config_broken: config_path.as_ref().is_some_and(|p| p.exists()),
                config_path,
                taken: Vec::new(),
            }
        }
    };
    pane.form.show(ui, pane.show_cli, pane.cli, &target);
}

/// A file in the AWS CLI's folder, named the way the platform's users write
/// it: `~` means little to most Windows users.
fn aws_file(name: &str) -> String {
    if cfg!(windows) {
        format!(r"%USERPROFILE%\.aws\{name}")
    } else {
        format!("~/.aws/{name}")
    }
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
        KeySource::CredentialsFile => aws_file("credentials"),
        KeySource::ConfigFile => aws_file("config"),
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
    accounts: Option<&'a Accounts>,
    filter: &'a mut String,
    sort: &'a mut (SortColumn, bool),
    /// The profile whose region is being written, if any (in any session).
    region_update: Option<&'a str>,
}

/// Show the account filter once a list is longer than this.
const FILTER_THRESHOLD: usize = 6;

impl SessionDetail<'_> {
    fn show(&mut self, ui: &mut Ui) -> Option<Action> {
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
        let live = matches!(self.entry.state, SessionState::Live { .. });
        let dark = ui.visuals().dark_mode;
        let listed = match self.accounts {
            Some(Accounts::Loaded(list)) => Some(list.as_slice()),
            _ => None,
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("ACCOUNTS").small().weak().strong());
            match self.accounts {
                Some(Accounts::Loading(_)) => {
                    ui.spinner();
                    ui.label(RichText::new("Listing your accounts…").weak().small());
                }
                Some(Accounts::Loaded(list)) => {
                    let unnamed = accounts::account_rows(&session.profiles, Some(list))
                        .iter()
                        .filter(|r| r.profile.is_none())
                        .count();
                    let text = match unnamed {
                        0 => format!("{} available, all named", list.len()),
                        n => format!("{} available, {n} without a profile", list.len()),
                    };
                    ui.label(RichText::new(text).weak().small());
                }
                Some(Accounts::Failed(message)) => {
                    ui.colored_label(style::error_color(dark), RichText::new(message).small());
                }
                None if !live => {
                    ui.label(
                        RichText::new("Log in to see every account and role you can use")
                            .weak()
                            .small(),
                    );
                }
                None => {}
            }
            let loading = matches!(self.accounts, Some(Accounts::Loading(_)));
            if live && !loading {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button("Refresh list")
                        .on_hover_text("Ask AWS again which accounts and roles you can use")
                        .clicked()
                    {
                        action = Some(Action::RefreshAccounts(session.name.clone()));
                    }
                });
            }
        });
        ui.add_space(2.0);

        let mut rows = accounts::account_rows(&session.profiles, listed);
        if rows.is_empty() {
            ui.label(
                RichText::new("No profiles use this session yet. Log in to list your accounts, or run `aws configure sso`.")
                    .weak()
                    .italics(),
            );
            return action;
        }
        let filterable = rows.len() > FILTER_THRESHOLD;
        if filterable {
            ui.add(
                egui::TextEdit::singleline(self.filter)
                    .hint_text("Filter by account, ID, role, profile, or region")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
        }
        // Only filter while the box is on screen, so hidden text can't hide rows.
        let filter = if filterable {
            self.filter.trim().to_lowercase()
        } else {
            String::new()
        };
        accounts::sort_rows(&mut rows, self.sort.0, self.sort.1);
        let visible: Vec<&AccountRow> = rows
            .iter()
            .filter(|r| filter.is_empty() || row_matches(r, &filter))
            .collect();
        if visible.is_empty() {
            ui.label(
                RichText::new("No accounts match the filter.")
                    .weak()
                    .italics(),
            );
            return action;
        }

        let table = AccountTable {
            rows: &visible,
            have_listing: listed.is_some(),
            region_update: self.region_update,
            shell: Shell::current(),
        };
        let click = table.show(ui, &session.name, self.sort);
        let session = session.name.clone();
        match click {
            Some((_, RowClick::Rename(profile))) => {
                action = Some(Action::Rename { session, profile });
            }
            Some((row, RowClick::Name)) => {
                action = Some(Action::NameAccount {
                    session,
                    account_id: row.account_id.clone().unwrap_or_default(),
                    account_name: row.account_name.clone().unwrap_or_default(),
                    role_name: row.role_name.clone().unwrap_or_default(),
                });
            }
            Some((_, RowClick::SetRegion { profile, region })) => {
                action = Some(Action::SetRegion {
                    session,
                    profile,
                    region,
                });
            }
            Some((_, RowClick::Copied(text))) => {
                action = Some(Action::Copied { session, text });
            }
            None => {}
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

/// A small button with a drawn pencil before its label (egui's built-in
/// fonts have no pencil glyph).
fn pencil_button(ui: &mut Ui, label: &str) -> egui::Response {
    let response = ui.add(egui::Button::new(format!("     {label}")).small());
    let color = ui.style().interact(&response).fg_stroke.color;
    let rect = response.rect;
    let c = egui::pos2(rect.left() + 11.0, rect.center().y);
    let s = 3.6;
    // Body: a thick diagonal stroke; tip: a small triangle at the lower left.
    let tail = c + egui::vec2(s, -s);
    let neck = c + egui::vec2(-s * 0.45, s * 0.45);
    let painter = ui.painter();
    painter.line_segment([neck, tail], egui::Stroke::new(2.4, color));
    let tip = c + egui::vec2(-s * 1.15, s * 1.15);
    let n = egui::vec2(0.9, 0.9);
    painter.add(egui::Shape::convex_polygon(
        vec![neck - n, neck + n, tip],
        color,
        egui::Stroke::NONE,
    ));
    response
}

fn row_matches(row: &AccountRow, filter: &str) -> bool {
    [
        row.profile.as_ref().map(|p| p.name.as_str()),
        row.account_name.as_deref(),
        row.account_id.as_deref(),
        row.role_name.as_deref(),
        row.profile.as_ref().and_then(|p| p.region.as_deref()),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(filter))
}

enum RowClick {
    Rename(String),
    Name,
    SetRegion { profile: String, region: String },
    Copied(String),
}

/// The account list as a sortable table: one row per account role, with
/// its profile (if any), region, and the shell text to use the profile.
struct AccountTable<'a> {
    rows: &'a [&'a AccountRow],
    have_listing: bool,
    /// The profile whose region is being written, if any.
    region_update: Option<&'a str>,
    shell: Shell,
}

impl<'a> AccountTable<'a> {
    fn show(
        &self,
        ui: &mut Ui,
        session: &str,
        sort: &mut (SortColumn, bool),
    ) -> Option<(&'a AccountRow, RowClick)> {
        let mut click = None;
        // Scrolls sideways when the window is too narrow for every column.
        egui::ScrollArea::horizontal()
            .id_salt(("account-table", session))
            .show(ui, |ui| {
                ui.visuals_mut().faint_bg_color = style::table_stripe(ui.visuals().dark_mode);
                egui::Grid::new(("accounts", session))
                    .num_columns(7)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .min_row_height(30.0)
                    .show(ui, |ui| {
                        sort_header(ui, "Account ID", SortColumn::AccountId, sort);
                        sort_header(ui, "Account name", SortColumn::AccountName, sort);
                        sort_header(ui, "Role", SortColumn::Role, sort);
                        sort_header(ui, "Profile", SortColumn::Profile, sort);
                        sort_header(ui, "Region", SortColumn::Region, sort);
                        ui.label("");
                        header_label(ui, "CLI");
                        ui.end_row();

                        for &row in self.rows {
                            if let Some(c) = self.row(ui, row) {
                                click = Some((row, c));
                            }
                            ui.end_row();
                        }
                    });
            });
        click
    }

    fn row(&self, ui: &mut Ui, row: &AccountRow) -> Option<RowClick> {
        let mut click = None;
        let token_only = row.account_id.is_none() && row.role_name.is_none();

        ui.horizontal(|ui| {
            ui.label(row.account_id.as_deref().unwrap_or(""));
            if self.have_listing && !row.listed && row.account_id.is_some() {
                ui.label(RichText::new("not listed").small().weak())
                    .on_hover_text(
                        "Not in your current AWS access list; access may have been removed.",
                    );
            }
        });
        // Blank when the portal hasn't been asked (or has no name for it).
        ui.label(row.account_name.as_deref().unwrap_or(""));
        if token_only {
            ui.label(RichText::new("token only").small().weak())
                .on_hover_text("This profile has no account or role; it only holds the SSO token.");
        } else {
            ui.label(row.role_name.as_deref().unwrap_or(""));
        }

        let Some(profile) = &row.profile else {
            ui.label(RichText::new("none yet").italics().weak());
            ui.add_enabled(false, egui::Button::new("—").small())
                .on_disabled_hover_text("Name this account first to set its region");
            if pencil_button(ui, "Name")
                .on_hover_text("Create a profile for this account and role")
                .clicked()
            {
                click = Some(RowClick::Name);
            }
            ui.label("");
            return click;
        };

        ui.label(RichText::new(&profile.name).strong());
        if let Some(region) = self.region_picker(ui, &profile.name, profile.region.as_deref()) {
            click = Some(RowClick::SetRegion {
                profile: profile.name.clone(),
                region,
            });
        }
        if ui
            .small_button("Rename")
            .on_hover_text(format!(
                "Give this profile your own name in {}",
                aws_file("config")
            ))
            .clicked()
        {
            click = Some(RowClick::Rename(profile.name.clone()));
        }
        let text = profiles::profile_env_prefix(&profile.name, self.shell);
        ui.horizontal(|ui| {
            ui.label(RichText::new(text.trim_end()).monospace().small());
            let hint = match self.shell {
                Shell::Posix => "Copy; paste it before an aws command to run that command as this profile",
                Shell::PowerShell => {
                    "Copy for PowerShell; it uses this profile for the rest of that PowerShell window"
                }
            };
            if ui.small_button("Copy").on_hover_text(hint).clicked() {
                ui.ctx().copy_text(text.clone());
                click = Some(RowClick::Copied(text.clone()));
            }
        });
        click
    }

    /// The profile's default region, as a dropdown of every known region.
    /// Returns a newly picked region.
    fn region_picker(&self, ui: &mut Ui, profile: &str, current: Option<&str>) -> Option<String> {
        if self.region_update == Some(profile) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Saving…").small().weak());
            });
            return None;
        }
        let mut picked = None;
        let busy = self.region_update.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let combo = egui::ComboBox::from_id_salt(("region", profile))
                .selected_text(current.unwrap_or("—"))
                .width(110.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    for region in regions::REGIONS {
                        let label = format!("{}   {}", region.code, region.name);
                        if ui
                            .selectable_label(current == Some(region.code), label)
                            .clicked()
                            && current != Some(region.code)
                        {
                            picked = Some(region.code.to_string());
                        }
                    }
                });
            let hover = match current {
                Some(code) => regions::name(code).unwrap_or(code).to_string(),
                None => "No default region; pick one to set it".into(),
            };
            combo
                .response
                .on_hover_text(format!("{hover}. Saved with `aws configure set region`."))
                .on_disabled_hover_text("Another region change is being saved");
        });
        picked
    }
}

fn header_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().weak().strong());
}

/// A clickable column header. Clicking the sorted column flips direction;
/// another column sorts ascending. The sorted column shows a drawn triangle
/// (egui's built-in fonts have no reliable arrow glyphs).
fn sort_header(ui: &mut Ui, text: &str, column: SortColumn, sort: &mut (SortColumn, bool)) {
    let active = sort.0 == column;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label = RichText::new(text).small().strong();
        let label = if active { label } else { label.weak() };
        let response = ui
            .add(egui::Label::new(label).sense(Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("Sort by {}", text.to_lowercase()));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
        if active {
            let color = ui.visuals().strong_text_color();
            let c = rect.center();
            let s = 3.5;
            let points = if sort.1 {
                vec![
                    c + egui::vec2(-s, -s * 0.6),
                    c + egui::vec2(s, -s * 0.6),
                    c + egui::vec2(0.0, s * 0.7),
                ]
            } else {
                vec![
                    c + egui::vec2(-s, s * 0.6),
                    c + egui::vec2(s, s * 0.6),
                    c + egui::vec2(0.0, -s * 0.7),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
        }
        if response.clicked() {
            *sort = if active {
                (column, !sort.1)
            } else {
                (column, false)
            };
        }
    });
}
