//! Presentation helpers: status colors and human-readable durations.

use eframe::egui::Color32;
use lunge_core::cache::SessionState;
use lunge_core::time::Duration;

/// Status color, tuned per theme for contrast. Always shown next to a text
/// label: color is never the only signal.
pub fn status_color(state: &SessionState, dark: bool) -> Color32 {
    let (on_dark, on_light) = match state {
        SessionState::Live { .. } => ((74, 222, 128), (21, 128, 61)),
        SessionState::Renewable { .. } => ((96, 165, 250), (29, 78, 216)),
        SessionState::Expired { .. } => ((248, 113, 113), (185, 28, 28)),
        SessionState::NotLoggedIn => ((156, 163, 175), (75, 85, 99)),
        SessionState::Unreadable(_) => ((251, 191, 36), (180, 83, 9)),
    };
    let (r, g, b) = if dark { on_dark } else { on_light };
    Color32::from_rgb(r, g, b)
}

pub fn success_color(dark: bool) -> Color32 {
    status_color(
        &SessionState::Live {
            expires_at: lunge_core::time::OffsetDateTime::UNIX_EPOCH,
        },
        dark,
    )
}

pub fn error_color(dark: bool) -> Color32 {
    status_color(
        &SessionState::Expired {
            expired_at: lunge_core::time::OffsetDateTime::UNIX_EPOCH,
        },
        dark,
    )
}

/// Compact duration: "2d 4h", "7h 12m", "12m", "under a minute".
pub fn human_duration(d: Duration) -> String {
    let minutes = d.whole_minutes().max(0);
    let (days, hours, mins) = (minutes / 1440, (minutes / 60) % 24, minutes % 60);
    match (days, hours, mins) {
        (0, 0, 0) => "under a minute".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_duration_formats() {
        assert_eq!(human_duration(Duration::seconds(30)), "under a minute");
        assert_eq!(human_duration(Duration::minutes(12)), "12m");
        assert_eq!(human_duration(Duration::minutes(7 * 60 + 12)), "7h 12m");
        assert_eq!(human_duration(Duration::hours(52)), "2d 4h");
        assert_eq!(human_duration(Duration::minutes(-5)), "under a minute");
    }
}
