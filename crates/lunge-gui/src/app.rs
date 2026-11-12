//! The main window: SSO sessions with their state, and the profiles under each.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Layout, RichText, Ui};
use lunge_core::cache::SessionState;
use lunge_core::config::SsoProfile;
use lunge_core::time::OffsetDateTime;
use lunge_core::{Inventory, SessionEntry};

use crate::style;

/// Re-read the config and token cache this often, so states and countdowns
/// don't go stale while the window sits open. Each pass reads one small file
/// per session.
const RELOAD_INTERVAL: Duration = Duration::from_secs(30);

pub struct LungeApp {
    inventory: Result<Inventory, String>,
    loaded_at: Instant,
}

impl LungeApp {
    pub fn new() -> Self {
        Self {
            inventory: load(),
            loaded_at: Instant::now(),
        }
    }

    fn reload(&mut self) {
        self.inventory = load();
        self.loaded_at = Instant::now();
    }
}

fn load() -> Result<Inventory, String> {
    // NO CREDENTIAL CUSTODY: read-only view of the AWS CLI's config and cache.
    Inventory::load_default(OffsetDateTime::now_utc()).map_err(|e| e.to_string())
}

impl eframe::App for LungeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match &self.inventory {
                Err(message) => load_error(ui, message),
                Ok(inventory) => inventory_view(ui, inventory),
            });
        });

        ui.ctx().request_repaint_after(RELOAD_INTERVAL);
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

fn inventory_view(ui: &mut Ui, inventory: &Inventory) {
    if inventory.sessions.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("No SSO sessions configured").strong());
        ui.label(format!(
            "{} has no [sso-session] blocks. Add one with `aws configure sso`, then press Refresh.",
            inventory.config_path.display()
        ));
    }

    let now = OffsetDateTime::now_utc();
    for entry in &inventory.sessions {
        session_card(ui, entry, now);
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
}

fn session_card(ui: &mut Ui, entry: &SessionEntry, now: OffsetDateTime) {
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                status_badge(ui, &entry.state);
                ui.label(RichText::new(&entry.session.name).strong().size(16.0));
            });
            ui.label(
                RichText::new(format!(
                    "{}  ·  {}",
                    entry.session.start_url, entry.session.region
                ))
                .weak(),
            );
            ui.label(state_detail(&entry.state, now));

            if entry.session.profiles.is_empty() {
                ui.label(
                    RichText::new("No profiles use this session.")
                        .weak()
                        .italics(),
                );
            }
            for profile in &entry.session.profiles {
                ui.separator();
                profile_row(ui, profile);
            }
        });
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
