//! UI-agnostic core for Lunge.
//!
//! This crate owns AWS config parsing, SSO session state, triggering
//! `aws sso login`, and console federation. It must never depend on a GUI
//! toolkit; `lunge-gui` is a thin front end over it.
//!
//! # Invariant: no credential custody
//!
//! Lunge never stores, encrypts, or persists credentials or tokens. The AWS
//! CLI owns all token persistence. This crate only *reads* the CLI's state
//! (`~/.aws/config`, `~/.aws/sso/cache/`) and shells out to the CLI to act.
//!
//! The single exception is console federation: short-lived role credentials
//! are held in memory only for the duration of the federation HTTP call and
//! are never written to disk, logged, or cached.
//!
//! Every code path that touches credential state must restate this invariant
//! in a comment at the point of contact.

pub mod cache;
pub mod config;
pub mod login;

use std::path::{Path, PathBuf};

pub use time;
use time::OffsetDateTime;

use cache::SessionState;
use config::{AccessKeyProfile, ConfigError, ConfigIssue, SsoSession};

/// Every configured SSO session with its current state.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub config_path: PathBuf,
    pub cache_dir: PathBuf,
    pub sessions: Vec<SessionEntry>,
    pub access_key_profiles: Vec<AccessKeyProfile>,
    pub issues: Vec<ConfigIssue>,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub session: SsoSession,
    pub state: SessionState,
}

impl Inventory {
    /// Loads from the locations the AWS CLI uses (`AWS_CONFIG_FILE` or
    /// `~/.aws/config`, `AWS_SHARED_CREDENTIALS_FILE` or
    /// `~/.aws/credentials`, and `~/.aws/sso/cache`).
    pub fn load_default(now: OffsetDateTime) -> Result<Self, ConfigError> {
        let config_path = config::default_config_path().ok_or(ConfigError::NoPath)?;
        let credentials_path = config::default_credentials_path();
        let cache_dir = cache::default_cache_dir().ok_or(ConfigError::NoPath)?;
        Self::load(&config_path, credentials_path.as_deref(), &cache_dir, now)
    }

    /// NO CREDENTIAL CUSTODY: reads the config, the profile names (never key
    /// values) in the credentials file, and the CLI token cache; writes
    /// nothing.
    pub fn load(
        config_path: &Path,
        credentials_path: Option<&Path>,
        cache_dir: &Path,
        now: OffsetDateTime,
    ) -> Result<Self, ConfigError> {
        let config = config::load(config_path, credentials_path)?;
        let sessions = config
            .sessions
            .into_iter()
            .map(|session| SessionEntry {
                state: cache::session_state(cache_dir, &session.name, now),
                session,
            })
            .collect();
        Ok(Self {
            config_path: config_path.to_path_buf(),
            cache_dir: cache_dir.to_path_buf(),
            sessions,
            access_key_profiles: config.access_key_profiles,
            issues: config.issues,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_pairs_each_session_with_its_cache_state() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config");
        let cache_dir = dir.path().join("cache");
        std::fs::create_dir(&cache_dir).unwrap();
        std::fs::write(
            &config_path,
            "\
[sso-session live]
sso_start_url = https://a.awsapps.com/start
sso_region = us-east-1
[sso-session never]
sso_start_url = https://b.awsapps.com/start
sso_region = us-east-1
[profile orphan]
sso_session = gone
",
        )
        .unwrap();
        std::fs::write(
            cache::cache_file_for_session(&cache_dir, "live"),
            r#"{"accessToken": "PLACEHOLDER", "expiresAt": "2099-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let inventory =
            Inventory::load(&config_path, None, &cache_dir, OffsetDateTime::now_utc()).unwrap();

        let states: Vec<_> = inventory
            .sessions
            .iter()
            .map(|e| (e.session.name.as_str(), e.state.to_string()))
            .collect();
        assert_eq!(
            states,
            [("live", "Live".into()), ("never", "Not logged in".into())]
        );
        assert_eq!(inventory.issues.len(), 1);
    }
}
