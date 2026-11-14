//! Listing every account and role an SSO session can use, and lining them up
//! with the profiles already in the AWS config.
//!
//! The AWS CLI's `aws sso list-accounts` / `list-account-roles` need the
//! session's access token passed in explicitly, either on the command line
//! (readable by other users via `ps`) or in a file on disk; it won't read it
//! from stdin. So, as an owner-approved exception to "prefer the CLI", Lunge
//! calls the same AWS SSO portal endpoints the CLI uses.
//!
//! NO CREDENTIAL CUSTODY (owner-approved exception): the access token is
//! read from the CLI's cache only while the session is Live, held in memory
//! as a [`Secret`] for these requests, and sent only to the AWS SSO portal in
//! the request header. It is never logged, written, cached, or put on a
//! command line, and error messages never include it.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use time::OffsetDateTime;

use crate::cache::{self, Secret, SessionState};
use crate::config::{SsoProfile, SsoSession};

/// One role in one account, as the SSO portal reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRole {
    pub account_id: String,
    pub account_name: String,
    pub role_name: String,
}

#[derive(Debug)]
pub enum ListError {
    /// The session isn't Live, so there's no usable token to ask with.
    NotLive(SessionState),
    /// AWS rejected the token (expired or signed out since the cache said
    /// Live); logging in again fixes it.
    TokenRejected,
    /// Network, TLS, or unexpected HTTP failure.
    Http(String),
    /// AWS answered with something Lunge couldn't parse.
    BadResponse(String),
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLive(state) => write!(
                f,
                "log in to list accounts (session is {})",
                state.to_string().to_lowercase()
            ),
            Self::TokenRejected => {
                write!(f, "AWS didn't accept the session's token; log in again")
            }
            Self::Http(message) => write!(f, "couldn't reach AWS SSO: {message}"),
            Self::BadResponse(message) => write!(f, "unexpected reply from AWS SSO: {message}"),
        }
    }
}

impl std::error::Error for ListError {}

/// Lists every account and role available through `session`, sorted by
/// account name then role. Blocking: call it off the UI thread.
pub fn list_account_roles(
    session: &SsoSession,
    cache_dir: &Path,
) -> Result<Vec<AccountRole>, ListError> {
    // NO CREDENTIAL CUSTODY: see the module docs. `token` is zeroed when it
    // goes out of scope at the end of this function.
    let token = cache::read_access_token(cache_dir, &session.name, OffsetDateTime::now_utc())
        .map_err(ListError::NotLive)?;
    let portal = Portal::new(portal_url(&session.region));
    portal.list_all(&token)
}

/// The SSO portal endpoint for a region (what botocore resolves for the
/// `sso` service).
fn portal_url(region: &str) -> String {
    let suffix = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    format!("https://portal.sso.{region}.{suffix}")
}

/// How many accounts' roles to fetch at once.
const PARALLEL_REQUESTS: usize = 4;
const MAX_ATTEMPTS: u32 = 4;

struct Portal {
    base_url: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountsPage {
    #[serde(default)]
    account_list: Vec<AccountInfo>,
    next_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    account_id: String,
    #[serde(default)]
    account_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolesPage {
    #[serde(default)]
    role_list: Vec<RoleInfo>,
    next_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoleInfo {
    role_name: String,
}

impl Portal {
    fn new(base_url: String) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            // Trust what the OS trusts, including CAs a company installs for
            // TLS-inspecting proxies.
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            // HTTPS_PROXY / ALL_PROXY / NO_PROXY, as the AWS CLI honours.
            .proxy(ureq::Proxy::try_from_env())
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .user_agent(concat!("lunge/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self { base_url, agent }
    }

    fn list_all(&self, token: &Secret) -> Result<Vec<AccountRole>, ListError> {
        let mut accounts = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let page: AccountsPage =
                self.get("/assignment/accounts", &[], next.as_deref(), token)?;
            accounts.extend(page.account_list);
            next = page.next_token.filter(|t| !t.is_empty());
            if next.is_none() {
                break;
            }
        }

        let mut rows = Vec::new();
        for chunk in accounts.chunks(PARALLEL_REQUESTS) {
            let results: Vec<Result<Vec<String>, ListError>> = std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|account| scope.spawn(|| self.roles(&account.account_id, token)))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .unwrap_or_else(|_| Err(ListError::Http("worker panicked".into())))
                    })
                    .collect()
            });
            for (account, roles) in chunk.iter().zip(results) {
                for role_name in roles? {
                    rows.push(AccountRole {
                        account_id: account.account_id.clone(),
                        account_name: account.account_name.clone(),
                        role_name,
                    });
                }
            }
        }
        rows.sort_by(|a, b| {
            (a.account_name.to_lowercase(), &a.account_id, &a.role_name).cmp(&(
                b.account_name.to_lowercase(),
                &b.account_id,
                &b.role_name,
            ))
        });
        Ok(rows)
    }

    fn roles(&self, account_id: &str, token: &Secret) -> Result<Vec<String>, ListError> {
        let mut roles = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let page: RolesPage = self.get(
                "/assignment/roles",
                &[("account_id", account_id)],
                next.as_deref(),
                token,
            )?;
            roles.extend(page.role_list.into_iter().map(|r| r.role_name));
            next = page.next_token.filter(|t| !t.is_empty());
            if next.is_none() {
                return Ok(roles);
            }
        }
    }

    /// GET with the bearer token, retrying throttling and server errors.
    fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        next_token: Option<&str>,
        token: &Secret,
    ) -> Result<T, ListError> {
        let url = format!("{}{path}", self.base_url);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = self
                .agent
                .get(&url)
                .query("max_result", "100")
                // NO CREDENTIAL CUSTODY: the token goes only into this header.
                .header("x-amz-sso_bearer_token", token.as_str());
            for (key, value) in query {
                request = request.query(*key, *value);
            }
            if let Some(next) = next_token {
                request = request.query("next_token", next);
            }
            // ureq errors describe the connection, never request headers.
            let mut response = request.call().map_err(|e| ListError::Http(e.to_string()))?;
            let status = response.status().as_u16();
            let retryable = status == 429 || status >= 500;
            if retryable && attempt < MAX_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(250 * 2u64.pow(attempt)));
                continue;
            }
            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|e| ListError::Http(e.to_string()))?;
            return match status {
                200 => {
                    serde_json::from_str(&body).map_err(|e| ListError::BadResponse(e.to_string()))
                }
                401 | 403 => Err(ListError::TokenRejected),
                _ => Err(ListError::Http(format!("HTTP {status} from {path}"))),
            };
        }
    }
}

/// One line in a session's account list: an account role, the profile for
/// it (if any), or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub account_id: Option<String>,
    /// From the SSO portal; `None` until the session's accounts are listed.
    pub account_name: Option<String>,
    pub role_name: Option<String>,
    /// The configured profile for this account and role, if there is one.
    pub profile: Option<SsoProfile>,
    /// Whether the SSO portal listed this account role. `false` for a
    /// profile whose access the portal no longer shows (only meaningful once
    /// a listing has been fetched).
    pub listed: bool,
}

/// Lines up a session's configured profiles with what the SSO portal lists.
/// Without a listing, it's just the profiles. With one, every listed account
/// role appears once per matching profile, or once with no profile; profiles
/// that match nothing listed (e.g. access since removed) come last.
pub fn account_rows(profiles: &[SsoProfile], listed: Option<&[AccountRole]>) -> Vec<AccountRow> {
    let from_profile =
        |profile: &SsoProfile, account_name: Option<&str>, listed: bool| AccountRow {
            account_id: profile.account_id.clone(),
            account_name: account_name.map(str::to_string),
            role_name: profile.role_name.clone(),
            profile: Some(profile.clone()),
            listed,
        };
    let Some(listed) = listed else {
        return profiles
            .iter()
            .map(|p| from_profile(p, None, false))
            .collect();
    };

    let matches = |profile: &SsoProfile, role: &AccountRole| {
        profile.account_id.as_deref() == Some(role.account_id.as_str())
            && profile.role_name.as_deref() == Some(role.role_name.as_str())
    };
    let mut rows = Vec::new();
    for role in listed {
        let mut matched = profiles.iter().filter(|p| matches(p, role)).peekable();
        if matched.peek().is_none() {
            rows.push(AccountRow {
                account_id: Some(role.account_id.clone()),
                account_name: Some(role.account_name.clone()),
                role_name: Some(role.role_name.clone()),
                profile: None,
                listed: true,
            });
        }
        for profile in matched {
            rows.push(from_profile(profile, Some(&role.account_name), true));
        }
    }
    for profile in profiles {
        if !listed.iter().any(|role| matches(profile, role)) {
            let name = listed
                .iter()
                .find(|r| profile.account_id.as_deref() == Some(r.account_id.as_str()))
                .map(|r| r.account_name.as_str());
            rows.push(from_profile(profile, name, false));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    const TOKEN: &str = "TEST-TOKEN-NOT-REAL";

    /// A tiny HTTP server answering like the SSO portal. Requests without the
    /// right bearer token get 401. `throttle_first` 429s that many requests.
    fn fake_portal(throttle_first: usize) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut throttle = throttle_first;
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut authorized = false;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if line.to_lowercase()
                        == format!("x-amz-sso_bearer_token: {TOKEN}\r\n").to_lowercase()
                    {
                        authorized = true;
                    }
                }
                let target = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                log.lock().unwrap().push(target.clone());
                let (status, body) = if throttle > 0 {
                    throttle -= 1;
                    (
                        "429 Too Many Requests",
                        r#"{"message":"slow down"}"#.to_string(),
                    )
                } else if !authorized {
                    (
                        "401 Unauthorized",
                        r#"{"message":"Session token not found or invalid"}"#.to_string(),
                    )
                } else if target.starts_with("/assignment/accounts")
                    && !target.contains("next_token")
                {
                    ("200 OK", r#"{"accountList":[{"accountId":"222222222222","accountName":"Zeta Prod"}],"nextToken":"page/2+="}"#.to_string())
                } else if target.starts_with("/assignment/accounts") {
                    assert!(target.contains("next_token=page%2F2%2B%3D"), "{target}");
                    ("200 OK", r#"{"accountList":[{"accountId":"111111111111","accountName":"alpha dev"}]}"#.to_string())
                } else if target.contains("account_id=111111111111") {
                    ("200 OK", r#"{"roleList":[{"roleName":"ReadOnlyAccess","accountId":"111111111111"},{"roleName":"AdministratorAccess","accountId":"111111111111"}]}"#.to_string())
                } else {
                    ("200 OK", r#"{"roleList":[{"roleName":"AdministratorAccess","accountId":"222222222222"}]}"#.to_string())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (url, seen)
    }

    fn secret(s: &str) -> Secret {
        Secret::new(s.to_string())
    }

    #[test]
    fn lists_all_pages_and_roles_sorted() {
        let (url, _) = fake_portal(0);
        let rows = Portal::new(url).list_all(&secret(TOKEN)).unwrap();
        let summary: Vec<_> = rows
            .iter()
            .map(|r| format!("{}/{}/{}", r.account_name, r.account_id, r.role_name))
            .collect();
        assert_eq!(
            summary,
            [
                "alpha dev/111111111111/AdministratorAccess",
                "alpha dev/111111111111/ReadOnlyAccess",
                "Zeta Prod/222222222222/AdministratorAccess",
            ]
        );
    }

    #[test]
    fn rejected_token_is_reported_without_leaking_it() {
        let (url, _) = fake_portal(0);
        let err = Portal::new(url)
            .list_all(&secret("WRONG-TOKEN-VALUE"))
            .unwrap_err();
        assert!(matches!(err, ListError::TokenRejected), "{err:?}");
        let text = format!("{err} {err:?}");
        assert!(!text.contains("WRONG-TOKEN-VALUE"), "{text}");
    }

    #[test]
    fn throttling_is_retried() {
        let (url, seen) = fake_portal(2);
        let rows = Portal::new(url).list_all(&secret(TOKEN)).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(seen.lock().unwrap().len() >= 6, "retries happened");
    }

    #[test]
    fn network_errors_are_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let err = Portal::new(url).list_all(&secret(TOKEN)).unwrap_err();
        assert!(matches!(err, ListError::Http(_)), "{err:?}");
    }

    #[test]
    fn listing_requires_a_live_session() {
        let dir = tempfile::tempdir().unwrap();
        let session = SsoSession {
            name: "work".into(),
            start_url: "u".into(),
            region: "us-east-1".into(),
            profiles: Vec::new(),
        };
        let err = list_account_roles(&session, dir.path()).unwrap_err();
        assert!(
            matches!(err, ListError::NotLive(SessionState::NotLoggedIn)),
            "{err:?}"
        );
    }

    #[test]
    fn portal_urls_by_partition() {
        assert_eq!(
            portal_url("us-east-1"),
            "https://portal.sso.us-east-1.amazonaws.com"
        );
        assert_eq!(
            portal_url("cn-north-1"),
            "https://portal.sso.cn-north-1.amazonaws.com.cn"
        );
    }

    fn profile(name: &str, account: Option<&str>, role: Option<&str>) -> SsoProfile {
        SsoProfile {
            name: name.into(),
            account_id: account.map(str::to_string),
            role_name: role.map(str::to_string),
            region: None,
        }
    }

    fn role(account: &str, name: &str, role: &str) -> AccountRole {
        AccountRole {
            account_id: account.into(),
            account_name: name.into(),
            role_name: role.into(),
        }
    }

    #[test]
    fn rows_without_a_listing_are_just_profiles() {
        let rows = account_rows(&[profile("p", Some("1"), Some("Admin"))], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].profile.as_ref().unwrap().name, "p");
        assert!(!rows[0].listed);
    }

    #[test]
    fn rows_merge_profiles_with_listing() {
        let profiles = [
            profile("prod-admin", Some("1"), Some("Admin")),
            profile("prod-admin-2", Some("1"), Some("Admin")),
            profile("gone", Some("9"), Some("Admin")),
            profile("token-only", None, None),
        ];
        let listed = [
            role("1", "Prod", "Admin"),
            role("1", "Prod", "ReadOnly"),
            role("2", "Dev", "Admin"),
        ];
        let rows = account_rows(&profiles, Some(&listed));
        let summary: Vec<_> = rows
            .iter()
            .map(|r| {
                format!(
                    "{}:{}:{}:{}",
                    r.account_id.as_deref().unwrap_or("-"),
                    r.role_name.as_deref().unwrap_or("-"),
                    r.profile
                        .as_ref()
                        .map(|p| p.name.as_str())
                        .unwrap_or("UNNAMED"),
                    r.listed
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                "1:Admin:prod-admin:true",
                "1:Admin:prod-admin-2:true",
                "1:ReadOnly:UNNAMED:true",
                "2:Admin:UNNAMED:true",
                "9:Admin:gone:false",
                "-:-:token-only:false",
            ]
        );
        assert_eq!(rows[2].account_name.as_deref(), Some("Prod"));
    }
}
