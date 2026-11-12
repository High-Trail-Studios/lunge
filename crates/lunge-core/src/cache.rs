//! Working out each SSO session's state from the AWS CLI's token cache.
//!
//! NO CREDENTIAL CUSTODY: this module only *reads* `~/.aws/sso/cache/`. It
//! never writes, copies, or caches anything. Cache entries hold secrets
//! (`accessToken`, `refreshToken`, `clientSecret`). [`CacheEntry`] has no
//! field that stores them: serde skips over those values while parsing, and
//! only whether they are present is kept. The raw file bytes are zeroed as
//! soon as parsing finishes (best effort; the allocator and OS may still hold
//! copies, so this narrows exposure rather than guaranteeing erasure).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Where the AWS CLI keeps SSO tokens. The CLI has no setting to move it.
pub fn default_cache_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".aws").join("sso").join("cache"))
}

/// The cache file the CLI uses for an `sso-session`: the SHA-1 of the
/// session name (botocore `SSOTokenLoader._generate_cache_key`).
pub fn cache_file_for_session(cache_dir: &Path, session_name: &str) -> PathBuf {
    // Digest's Display is lowercase hex, matching Python's hexdigest().
    let key = sha1_smol::Sha1::from(session_name).digest();
    cache_dir.join(format!("{key}.json"))
}

/// What Lunge can tell about a session from the CLI's cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The cached access token is valid until `expires_at`.
    Live { expires_at: OffsetDateTime },
    /// The access token expired, but the CLI holds a refresh token and a
    /// current client registration, so its next use of this session will
    /// probably renew it without a browser. Not guaranteed: the refresh
    /// token's own lifetime (the identity provider's session length) isn't
    /// recorded in the cache.
    Renewable { expired_at: OffsetDateTime },
    /// The token expired and the CLI can't renew it; log in again.
    Expired { expired_at: OffsetDateTime },
    /// No cached token for this session.
    NotLoggedIn,
    /// A cache file exists but couldn't be read or understood.
    Unreadable(String),
}

impl SessionState {
    /// Whether the CLI can use this session right now without a browser
    /// login (possibly after a silent refresh).
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Live { .. } | Self::Renewable { .. })
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Live { .. } => "Live",
            Self::Renewable { .. } => "Renewable",
            Self::Expired { .. } => "Expired",
            Self::NotLoggedIn => "Not logged in",
            Self::Unreadable(_) => "Unreadable",
        })
    }
}

/// Reads the state of `session_name` from the CLI cache in `cache_dir`.
pub fn session_state(cache_dir: &Path, session_name: &str, now: OffsetDateTime) -> SessionState {
    let path = cache_file_for_session(cache_dir, session_name);
    // NO CREDENTIAL CUSTODY: read-only access to a CLI-owned file that
    // contains tokens. The bytes are parsed into CacheEntry (no secret
    // fields), then zeroed and dropped.
    let mut bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SessionState::NotLoggedIn,
        Err(e) => return SessionState::Unreadable(format!("can't read {}: {e}", path.display())),
    };
    let parsed = serde_json::from_slice::<CacheEntry>(&bytes);
    bytes.fill(0);
    std::hint::black_box(&bytes);
    drop(bytes);

    match parsed {
        Ok(entry) => entry.state(now),
        Err(e) => SessionState::Unreadable(format!("{} isn't valid JSON: {e}", path.display())),
    }
}

/// The non-secret parts of a CLI token cache entry.
///
/// NO CREDENTIAL CUSTODY: secret values deserialize into `IgnoredAny`, which
/// consumes them without storing anything. Never change these to `String`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheEntry {
    expires_at: Option<String>,
    registration_expires_at: Option<String>,
    access_token: Option<IgnoredAny>,
    refresh_token: Option<IgnoredAny>,
    client_id: Option<IgnoredAny>,
    client_secret: Option<IgnoredAny>,
}

impl CacheEntry {
    fn state(&self, now: OffsetDateTime) -> SessionState {
        // botocore treats an entry without both of these as invalid.
        let (Some(_), Some(expires_at)) = (&self.access_token, &self.expires_at) else {
            return SessionState::Unreadable("cache entry has no access token or expiry".into());
        };
        let Some(expires_at) = parse_timestamp(expires_at) else {
            return SessionState::Unreadable(format!("unrecognized expiresAt: {expires_at}"));
        };
        if expires_at > now {
            return SessionState::Live { expires_at };
        }
        // The same preconditions botocore checks before attempting a refresh.
        let registration_current = self
            .registration_expires_at
            .as_deref()
            .and_then(parse_timestamp)
            .is_some_and(|t| t > now);
        let can_refresh = self.refresh_token.is_some()
            && self.client_id.is_some()
            && self.client_secret.is_some()
            && registration_current;
        if can_refresh {
            SessionState::Renewable {
                expired_at: expires_at,
            }
        } else {
            SessionState::Expired {
                expired_at: expires_at,
            }
        }
    }
}

/// RFC 3339 (what current CLIs and SDKs write), plus the `...UTC` suffix
/// older AWS CLI v2 releases wrote.
fn parse_timestamp(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok().or_else(|| {
        let base = value.strip_suffix("UTC")?;
        OffsetDateTime::parse(&format!("{base}Z"), &Rfc3339).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use time::Duration;

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-11-12T12:00:00Z", &Rfc3339).unwrap()
    }

    fn state_for(json: &str) -> SessionState {
        let dir = tempfile::tempdir().unwrap();
        fs::write(cache_file_for_session(dir.path(), "work"), json).unwrap();
        session_state(dir.path(), "work", now())
    }

    /// A cache entry shaped like the AWS CLI's, with placeholder secrets.
    fn entry(expires_at: &str, registration_expires_at: &str) -> String {
        format!(
            r#"{{
  "startUrl": "https://example.awsapps.com/start",
  "region": "us-east-1",
  "accessToken": "PLACEHOLDER-ACCESS",
  "expiresAt": "{expires_at}",
  "clientId": "PLACEHOLDER-CLIENT",
  "clientSecret": "PLACEHOLDER-SECRET",
  "registrationExpiresAt": "{registration_expires_at}",
  "refreshToken": "PLACEHOLDER-REFRESH"
}}"#
        )
    }

    #[test]
    fn cache_filename_is_sha1_of_session_name() {
        // Cross-checked with: printf %s my-sso | sha1sum
        assert_eq!(
            cache_file_for_session(Path::new("/c"), "my-sso"),
            Path::new("/c/0ad374308c5a4e22f723adf10145eafad7c4031c.json")
        );
    }

    #[test]
    fn unexpired_token_is_live() {
        assert_eq!(
            state_for(&entry("2026-11-12T13:00:00Z", "2027-01-01T00:00:00Z")),
            SessionState::Live {
                expires_at: now() + Duration::hours(1)
            }
        );
    }

    #[test]
    fn expired_token_with_refresh_material_is_renewable() {
        assert_eq!(
            state_for(&entry("2026-11-12T11:00:00Z", "2027-01-01T00:00:00Z")),
            SessionState::Renewable {
                expired_at: now() - Duration::hours(1)
            }
        );
    }

    #[test]
    fn expired_registration_means_expired() {
        assert!(matches!(
            state_for(&entry("2026-11-12T11:00:00Z", "2026-11-01T00:00:00Z")),
            SessionState::Expired { .. }
        ));
    }

    #[test]
    fn expired_token_without_refresh_token_is_expired() {
        let state = state_for(
            r#"{"accessToken": "x", "expiresAt": "2026-11-12T11:00:00Z", "startUrl": "u"}"#,
        );
        assert!(matches!(state, SessionState::Expired { .. }), "{state:?}");
    }

    #[test]
    fn missing_file_is_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            session_state(dir.path(), "work", now()),
            SessionState::NotLoggedIn
        );
        assert_eq!(
            session_state(&dir.path().join("no-such-dir"), "work", now()),
            SessionState::NotLoggedIn
        );
    }

    #[test]
    fn exact_expiry_instant_is_not_live() {
        assert!(matches!(
            state_for(&entry("2026-11-12T12:00:00Z", "2026-01-01T00:00:00Z")),
            SessionState::Expired { .. }
        ));
    }

    #[test]
    fn accepts_timestamp_variants() {
        for ts in [
            "2026-11-12T13:00:00Z",
            "2026-11-12T13:00:00.000Z",
            "2026-11-12T14:00:00+01:00",
            "2026-11-12T13:00:00UTC",
        ] {
            assert_eq!(
                parse_timestamp(ts),
                Some(now() + Duration::hours(1)),
                "{ts}"
            );
        }
        assert_eq!(parse_timestamp("tomorrow"), None);
    }

    #[test]
    fn malformed_entries_are_unreadable_not_panics() {
        for json in [
            "",
            "not json",
            "[]",
            r#"{"expiresAt": "2026-11-12T13:00:00Z"}"#,
            r#"{"accessToken": "x"}"#,
            r#"{"accessToken": "x", "expiresAt": "soon"}"#,
        ] {
            assert!(
                matches!(state_for(json), SessionState::Unreadable(_)),
                "{json:?}"
            );
        }
    }

    #[test]
    fn reading_state_never_modifies_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_file_for_session(dir.path(), "work");
        let original = entry("2026-11-12T11:00:00Z", "2027-01-01T00:00:00Z");
        fs::write(&path, &original).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();

        session_state(dir.path(), "work", now());

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
