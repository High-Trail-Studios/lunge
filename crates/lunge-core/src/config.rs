//! Reading `~/.aws/config` (and the profile names in `~/.aws/credentials`)
//! into the sessions and profiles Lunge shows.
//!
//! Parsing mirrors the AWS CLI's own (botocore: Python's `RawConfigParser`
//! followed by `build_profile_map`) so Lunge sees the same sessions the CLI
//! does, and rejects the same files the CLI rejects. Lunge only ever reads
//! these files.
//!
//! NO CREDENTIAL CUSTODY: either file can hold static keys
//! (`aws_secret_access_key` and friends). Their values are discarded as each
//! line is parsed and only their presence is kept; parse errors never quote
//! file content; and the raw file bytes are zeroed once parsed (best effort,
//! as in [`crate::cache`]).

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Keys whose values are secrets. Never stored; see the module docs.
pub(crate) const SECRET_KEYS: [&str; 4] = [
    "aws_access_key_id",
    "aws_secret_access_key",
    "aws_session_token",
    "aws_security_token",
];

/// Stored in place of a secret value, so presence checks still work.
const REDACTED: &str = "(redacted)";

/// What Lunge shows from the AWS config and credentials files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AwsConfig {
    /// Usable `[sso-session]` blocks in file order, each with its profiles.
    pub sessions: Vec<SsoSession>,
    /// Profiles that authenticate with static access keys.
    pub access_key_profiles: Vec<AccessKeyProfile>,
    /// Problems that hide a session or profile, in file order.
    pub issues: Vec<ConfigIssue>,
}

/// A profile with static access keys (an IAM user). Lunge knows only that
/// keys exist and where; it never reads their values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessKeyProfile {
    pub name: String,
    /// The profile's default `region` from the config file, if set.
    pub region: Option<String>,
    /// Which file holds the keys. When both do, the CLI uses the
    /// credentials file.
    pub source: KeySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    CredentialsFile,
    ConfigFile,
}

/// One `[sso-session NAME]` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoSession {
    pub name: String,
    pub start_url: String,
    pub region: String,
    /// Profiles whose `sso_session` names this block, in file order.
    pub profiles: Vec<SsoProfile>,
}

/// A profile that authenticates through an `sso-session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoProfile {
    pub name: String,
    /// `sso_account_id`. Absent on token-only profiles.
    pub account_id: Option<String>,
    /// `sso_role_name`. Absent on token-only profiles.
    pub role_name: Option<String>,
    /// The profile's default `region`, if set.
    pub region: Option<String>,
}

/// A config problem that keeps Lunge from showing a session or profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigIssue {
    /// An `sso-session` block without a required key; the CLI can't log in
    /// to it either.
    SessionMissingKey { session: String, key: &'static str },
    /// A profile whose `sso_session` names a block that doesn't exist.
    UnknownSession { profile: String, session: String },
    /// A profile using the legacy `sso_start_url`-in-profile layout, which
    /// Lunge doesn't support yet.
    LegacySsoProfile { profile: String },
    /// The credentials file exists but couldn't be read or parsed, so
    /// access-key profiles may be missing.
    CredentialsFile(String),
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionMissingKey { session, key } => {
                write!(f, "sso-session \"{session}\" is missing {key}")
            }
            Self::UnknownSession { profile, session } => write!(
                f,
                "profile \"{profile}\" refers to sso-session \"{session}\", which isn't defined"
            ),
            Self::LegacySsoProfile { profile } => write!(
                f,
                "profile \"{profile}\" uses the legacy SSO layout (sso_start_url in the profile); \
                 move it to an [sso-session] block with `aws configure sso` to use it in Lunge"
            ),
            Self::CredentialsFile(message) => {
                write!(
                    f,
                    "credentials file: {message}; access-key profiles may be missing"
                )
            }
        }
    }
}

/// A syntax error the AWS CLI would also reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Why the config file couldn't be loaded.
#[derive(Debug)]
pub enum ConfigError {
    /// No AWS config could be located (no `AWS_CONFIG_FILE`, no home directory).
    NoPath,
    NotFound(PathBuf),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: ParseError,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPath => write!(
                f,
                "can't locate the AWS config file: AWS_CONFIG_FILE is unset and there is no home directory"
            ),
            Self::NotFound(path) => write!(f, "no AWS config file at {}", path.display()),
            Self::Io { path, source } => write!(f, "can't read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "{} {source}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::NoPath | Self::NotFound(_) => None,
        }
    }
}

/// The config file the AWS CLI would read: `AWS_CONFIG_FILE` if set,
/// otherwise `~/.aws/config`.
pub fn default_config_path() -> Option<PathBuf> {
    resolve_config_path(std::env::var_os("AWS_CONFIG_FILE"), std::env::home_dir())
}

fn resolve_config_path(
    env_value: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    match env_value.filter(|v| !v.is_empty()) {
        // Like the CLI, expand a leading `~`. ($VAR expansion is not supported.)
        Some(value) => {
            let path = PathBuf::from(value);
            match path.strip_prefix("~") {
                Ok(rest) => home.map(|h| h.join(rest)),
                Err(_) => Some(path),
            }
        }
        None => home.map(|h| h.join(".aws").join("config")),
    }
}

/// The credentials file the AWS CLI would read: `AWS_SHARED_CREDENTIALS_FILE`
/// if set, otherwise `~/.aws/credentials`.
pub fn default_credentials_path() -> Option<PathBuf> {
    let home = std::env::home_dir();
    match std::env::var_os("AWS_SHARED_CREDENTIALS_FILE").filter(|v| !v.is_empty()) {
        Some(value) => resolve_config_path(Some(value), home),
        None => home.map(|h| h.join(".aws").join("credentials")),
    }
}

/// Reads the config file at `config_path` and, if given, the profile names
/// in the credentials file. A missing or broken credentials file is reported
/// as an issue rather than an error, since SSO doesn't need it.
pub fn load(config_path: &Path, credentials_path: Option<&Path>) -> Result<AwsConfig, ConfigError> {
    let config_result = read_scrubbed(config_path, |text| {
        let text = std::str::from_utf8(text).map_err(|_| ParseError {
            line: 0,
            message: "file is not valid UTF-8".into(),
        })?;
        parse_ini(text)
    });
    let config_sections = match config_result {
        Ok(Ok(sections)) => sections,
        Ok(Err(source)) => {
            return Err(ConfigError::Parse {
                path: config_path.to_path_buf(),
                source,
            });
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(ConfigError::NotFound(config_path.to_path_buf()));
        }
        Err(source) => {
            return Err(ConfigError::Io {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };

    let mut issues = Vec::new();
    let mut credential_profiles = Vec::new();
    if let Some(path) = credentials_path {
        match load_credential_profile_names(path) {
            Ok(names) => credential_profiles = names,
            Err(message) => issues.push(ConfigIssue::CredentialsFile(message)),
        }
    }

    let mut config = classify(config_sections, &credential_profiles);
    issues.append(&mut config.issues);
    config.issues = issues;
    Ok(config)
}

/// Names of credentials-file profiles that hold an access key.
fn load_credential_profile_names(path: &Path) -> Result<Vec<String>, String> {
    // NO CREDENTIAL CUSTODY: this file holds secret keys. parse_ini discards
    // their values, and read_scrubbed zeroes the raw bytes afterwards.
    let result = read_scrubbed(path, |text| match std::str::from_utf8(text) {
        Ok(text) => parse_ini(text).map_err(|e| e.to_string()),
        Err(_) => Err("file is not valid UTF-8".into()),
    });
    let sections = match result {
        Ok(sections) => sections?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("can't read {}: {e}", path.display())),
    };
    Ok(sections
        .into_iter()
        .filter(|s| get(&s.entries, "aws_access_key_id").is_some())
        .map(|s| s.name)
        .collect())
}

/// Reads `path`, hands the bytes to `f`, then zeroes them before returning.
fn read_scrubbed<T>(path: &Path, f: impl FnOnce(&[u8]) -> T) -> io::Result<T> {
    let mut bytes = std::fs::read(path)?;
    let result = f(&bytes);
    bytes.fill(0);
    std::hint::black_box(&bytes);
    Ok(result)
}

/// Parses AWS config file text on its own (no credentials file).
pub fn parse(text: &str) -> Result<AwsConfig, ParseError> {
    Ok(classify(parse_ini(text)?, &[]))
}

fn classify(config_sections: Vec<IniSection>, credential_profiles: &[String]) -> AwsConfig {
    let mut sessions: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut profiles: Vec<(String, Vec<(String, String)>)> = Vec::new();

    // Classify sections the way botocore's build_profile_map does. A later
    // section with the same resolved name replaces an earlier one in place
    // (e.g. `[default]` and `[profile default]`).
    for section in config_sections {
        let target = if section.name.starts_with("profile") {
            section_name_arg(&section.name).map(|name| (&mut profiles, name))
        } else if section.name.starts_with("sso-session") {
            section_name_arg(&section.name).map(|name| (&mut sessions, name))
        } else if section.name == "default" {
            Some((&mut profiles, "default".to_string()))
        } else {
            None
        };
        if let Some((list, name)) = target {
            match list.iter_mut().find(|(n, _)| *n == name) {
                Some(existing) => existing.1 = section.entries,
                None => list.push((name, section.entries)),
            }
        }
    }

    let mut config = AwsConfig::default();
    let mut invalid_sessions = HashSet::new();
    for (name, entries) in &sessions {
        let start_url = get(entries, "sso_start_url");
        let region = get(entries, "sso_region");
        match (start_url, region) {
            (Some(start_url), Some(region)) => config.sessions.push(SsoSession {
                name: name.clone(),
                start_url: start_url.to_string(),
                region: region.to_string(),
                profiles: Vec::new(),
            }),
            (start_url, _) => {
                let key = if start_url.is_none() {
                    "sso_start_url"
                } else {
                    "sso_region"
                };
                config.issues.push(ConfigIssue::SessionMissingKey {
                    session: name.clone(),
                    key,
                });
                invalid_sessions.insert(name.as_str());
            }
        }
    }

    let region_of = |name: &str| {
        profiles
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, entries)| get(entries, "region"))
            .map(str::to_string)
    };
    let is_sso = |name: &str| {
        profiles
            .iter()
            .any(|(n, entries)| n == name && get(entries, "sso_session").is_some())
    };

    // Access-key profiles: the credentials file first (the CLI prefers it),
    // then keys placed directly in the config file. A profile that also has
    // `sso_session` is shown as SSO, which the CLI tries first.
    for name in credential_profiles {
        if !is_sso(name) && !config.access_key_profiles.iter().any(|p| p.name == *name) {
            config.access_key_profiles.push(AccessKeyProfile {
                name: name.clone(),
                region: region_of(name),
                source: KeySource::CredentialsFile,
            });
        }
    }
    for (name, entries) in &profiles {
        if get(entries, "aws_access_key_id").is_some()
            && get(entries, "sso_session").is_none()
            && !config.access_key_profiles.iter().any(|p| p.name == *name)
        {
            config.access_key_profiles.push(AccessKeyProfile {
                name: name.clone(),
                region: get(entries, "region").map(str::to_string),
                source: KeySource::ConfigFile,
            });
        }
    }

    for (name, entries) in &profiles {
        let Some(session_name) = get(entries, "sso_session") else {
            if get(entries, "sso_start_url").is_some() {
                config.issues.push(ConfigIssue::LegacySsoProfile {
                    profile: name.clone(),
                });
            }
            continue;
        };
        let profile = SsoProfile {
            name: name.clone(),
            account_id: get(entries, "sso_account_id").map(str::to_string),
            role_name: get(entries, "sso_role_name").map(str::to_string),
            region: get(entries, "region").map(str::to_string),
        };
        match config.sessions.iter_mut().find(|s| s.name == session_name) {
            Some(session) => session.profiles.push(profile),
            // Already reported against the session itself.
            None if invalid_sessions.contains(session_name) => {}
            None => config.issues.push(ConfigIssue::UnknownSession {
                profile: name.clone(),
                session: session_name.to_string(),
            }),
        }
    }

    config
}

fn get<'a>(entries: &'a [(String, String)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

/// The NAME in `[profile NAME]` / `[sso-session NAME]`. Like botocore, the
/// header is shell-split and must yield exactly two words; otherwise the
/// section is ignored.
pub(crate) fn section_name_arg(header: &str) -> Option<String> {
    let mut words = shell_split(header)?;
    if words.len() == 2 { words.pop() } else { None }
}

/// Minimal POSIX-shell word splitting (quotes and backslashes), matching
/// Python's `shlex.split` for section headers. `None` on an unclosed quote.
fn shell_split(s: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => current.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => current.push(c),
                            c => {
                                current.push('\\');
                                current.push(c);
                            }
                        },
                        c => current.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                current.push(chars.next()?);
            }
            c => {
                in_word = true;
                current.push(c);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

pub(crate) struct IniSection {
    pub(crate) name: String,
    /// 1-based line number of the `[...]` header.
    pub(crate) line: usize,
    pub(crate) entries: Vec<(String, String)>,
}

/// INI parsing with Python `RawConfigParser` defaults: `=` or `:` delimiters,
/// full-line `#`/`;` comments only (no inline comments), lowercased keys,
/// indented lines continue the previous value, and duplicate sections or keys
/// are errors.
///
/// NO CREDENTIAL CUSTODY: values of [`SECRET_KEYS`] are replaced with
/// [`REDACTED`] as they're read, and error messages never include line
/// content (a malformed line could be a key).
pub(crate) fn parse_ini(text: &str) -> Result<Vec<IniSection>, ParseError> {
    let mut sections: Vec<IniSection> = Vec::new();
    let mut seen_sections = HashSet::new();
    // Indentation of the line that opened the current value, while one is open.
    let mut value_indent: Option<usize> = None;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let error = |message: String| ParseError { line, message };

        if let Some(open) = value_indent
            && indent > open
            && let Some((key, value)) = sections.last_mut().and_then(|s| s.entries.last_mut())
        {
            if !SECRET_KEYS.contains(&key.as_str()) {
                value.push('\n');
                value.push_str(trimmed);
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix('[') {
            let name = rest
                .rfind(']')
                .map(|end| &rest[..end])
                .filter(|name| !name.is_empty())
                .ok_or_else(|| error("malformed section header (missing `]`)".into()))?;
            if !seen_sections.insert(name.to_string()) {
                return Err(error(format!("section [{name}] appears more than once")));
            }
            sections.push(IniSection {
                name: name.to_string(),
                line,
                entries: Vec::new(),
            });
            value_indent = None;
            continue;
        }

        let Some(section) = sections.last_mut() else {
            return Err(error("setting appears before any [section] header".into()));
        };
        let Some(split) = trimmed.find(['=', ':']) else {
            return Err(error("expected `key = value` or a [section] header".into()));
        };
        let key = trimmed[..split].trim().to_lowercase();
        let value = if SECRET_KEYS.contains(&key.as_str()) {
            REDACTED.to_string()
        } else {
            trimmed[split + 1..].trim().to_string()
        };
        if key.is_empty() {
            return Err(error("missing key before `=`".into()));
        }
        if section.entries.iter().any(|(k, _)| *k == key) {
            return Err(error(format!(
                "key `{key}` appears more than once in [{}]",
                section.name
            )));
        }
        section.entries.push((key, value));
        value_indent = Some(indent);
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session<'a>(config: &'a AwsConfig, name: &str) -> &'a SsoSession {
        config
            .sessions
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no session {name}"))
    }

    #[test]
    fn parses_sessions_and_their_profiles_in_file_order() {
        let config = parse(
            "\
[default]
region = us-east-1

[profile dev-admin]
sso_session = work
sso_account_id = 111111111111
sso_role_name = AdministratorAccess
region = eu-west-1

[sso-session work]
sso_start_url = https://example.awsapps.com/start
sso_region = us-east-1
sso_registration_scopes = sso:account:access

[profile prod-readonly]
sso_session = work
sso_account_id = 222222222222
sso_role_name = ReadOnly

[sso-session personal]
sso_start_url = https://personal.awsapps.com/start
sso_region = us-west-2

[profile sandbox]
sso_session = personal
sso_account_id = 333333333333
sso_role_name = Admin
",
        )
        .unwrap();

        assert!(config.issues.is_empty(), "{:?}", config.issues);
        let names: Vec<_> = config.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["work", "personal"]);

        let work = session(&config, "work");
        assert_eq!(work.start_url, "https://example.awsapps.com/start");
        assert_eq!(work.region, "us-east-1");
        assert_eq!(
            work.profiles,
            [
                SsoProfile {
                    name: "dev-admin".into(),
                    account_id: Some("111111111111".into()),
                    role_name: Some("AdministratorAccess".into()),
                    region: Some("eu-west-1".into()),
                },
                SsoProfile {
                    name: "prod-readonly".into(),
                    account_id: Some("222222222222".into()),
                    role_name: Some("ReadOnly".into()),
                    region: None,
                },
            ]
        );
        assert_eq!(session(&config, "personal").profiles[0].name, "sandbox");
    }

    #[test]
    fn empty_and_non_sso_configs_have_no_sessions() {
        assert_eq!(parse("").unwrap(), AwsConfig::default());
        let config = parse("[default]\nregion = us-east-1\noutput = json\n").unwrap();
        assert_eq!(config, AwsConfig::default());
    }

    #[test]
    fn session_without_profiles_is_still_listed() {
        let config = parse(
            "[sso-session lonely]\nsso_start_url = https://x.awsapps.com/start\nsso_region = us-east-1\n",
        )
        .unwrap();
        assert_eq!(config.sessions.len(), 1);
        assert!(config.sessions[0].profiles.is_empty());
    }

    #[test]
    fn token_only_profile_has_no_account_or_role() {
        let config = parse(
            "[sso-session s]\nsso_start_url = u\nsso_region = r\n[profile p]\nsso_session = s\n",
        )
        .unwrap();
        let profile = &config.sessions[0].profiles[0];
        assert_eq!(profile.account_id, None);
        assert_eq!(profile.role_name, None);
    }

    #[test]
    fn default_section_counts_as_a_profile() {
        let config = parse(
            "[sso-session s]\nsso_start_url = u\nsso_region = r\n[default]\nsso_session = s\n",
        )
        .unwrap();
        assert_eq!(config.sessions[0].profiles[0].name, "default");
    }

    #[test]
    fn reports_session_missing_required_keys_and_skips_its_profiles() {
        let config = parse(
            "\
[sso-session no-url]
sso_region = us-east-1
[sso-session no-region]
sso_start_url = https://x.awsapps.com/start
[profile a]
sso_session = no-url
",
        )
        .unwrap();
        assert!(config.sessions.is_empty());
        assert_eq!(
            config.issues,
            [
                ConfigIssue::SessionMissingKey {
                    session: "no-url".into(),
                    key: "sso_start_url"
                },
                ConfigIssue::SessionMissingKey {
                    session: "no-region".into(),
                    key: "sso_region"
                },
            ]
        );
    }

    #[test]
    fn reports_profile_pointing_at_undefined_session() {
        let config = parse("[profile orphan]\nsso_session = missing\n").unwrap();
        assert_eq!(
            config.issues,
            [ConfigIssue::UnknownSession {
                profile: "orphan".into(),
                session: "missing".into()
            }]
        );
    }

    #[test]
    fn reports_legacy_sso_profiles() {
        let config = parse(
            "[profile old]\nsso_start_url = https://x.awsapps.com/start\nsso_region = us-east-1\nsso_account_id = 1\nsso_role_name = r\n",
        )
        .unwrap();
        assert!(config.sessions.is_empty());
        assert_eq!(
            config.issues,
            [ConfigIssue::LegacySsoProfile {
                profile: "old".into()
            }]
        );
    }

    #[test]
    fn follows_configparser_syntax_rules() {
        let config = parse(
            "\
# full-line comment
; another
[sso-session s]
SSO_Start_URL: https://x.awsapps.com/start
sso_region   =   us-east-1
[profile p]
sso_session = s
s3 =
    max_concurrent_requests = 20
    multipart_threshold = 64MB
sso_role_name = Role # not a comment
",
        )
        .unwrap();
        let s = &config.sessions[0];
        // Keys are case-insensitive; `:` is a delimiter; values are trimmed.
        assert_eq!(s.start_url, "https://x.awsapps.com/start");
        assert_eq!(s.region, "us-east-1");
        // Indented nested blocks don't leak into the profile's own keys, and
        // `#` mid-line is part of the value, exactly as the CLI reads it.
        assert_eq!(
            s.profiles[0].role_name.as_deref(),
            Some("Role # not a comment")
        );
    }

    #[test]
    fn indented_key_after_a_key_is_a_continuation_not_a_new_key() {
        // configparser folds this `sso_region` line into sso_start_url's
        // value, so the CLI sees no sso_region. Lunge must agree.
        let config =
            parse("[sso-session s]\nsso_start_url = u\n   sso_region = us-east-1\n").unwrap();
        assert_eq!(
            config.issues,
            [ConfigIssue::SessionMissingKey {
                session: "s".into(),
                key: "sso_region"
            }]
        );
        // Indentation at the start of a section is fine.
        let config =
            parse("[sso-session s]\n   sso_region = us-east-1\nsso_start_url = u\n").unwrap();
        assert_eq!(config.sessions[0].region, "us-east-1");
    }

    #[test]
    fn quoted_and_malformed_section_names() {
        let config = parse(
            "\
[sso-session \"my session\"]
sso_start_url = u
sso_region = r
[profile 'my profile']
sso_session = my session
[profile too many words]
sso_session = my session
[profilenospace]
sso_session = my session
",
        )
        .unwrap();
        let s = &config.sessions[0];
        assert_eq!(s.name, "my session");
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["my profile"]);
    }

    #[test]
    fn later_default_definition_replaces_earlier_one() {
        let config = parse(
            "\
[sso-session a]
sso_start_url = u
sso_region = r
[sso-session b]
sso_start_url = u
sso_region = r
[default]
sso_session = a
[profile default]
sso_session = b
",
        )
        .unwrap();
        assert!(session(&config, "a").profiles.is_empty());
        assert_eq!(session(&config, "b").profiles[0].name, "default");
    }

    #[test]
    fn rejects_what_the_cli_rejects() {
        let cases = [
            ("region = x\n", 1, "before any [section]"),
            ("[default]\nnot a setting\n", 2, "expected `key = value`"),
            ("[default]\n= value\n", 2, "missing key"),
            ("[default]\n[default]\n", 2, "more than once"),
            ("[default]\nregion = a\nREGION = b\n", 3, "more than once"),
            ("[default\n", 1, "malformed section header"),
        ];
        for (text, line, fragment) in cases {
            let err = parse(text).unwrap_err();
            assert_eq!(err.line, line, "{text:?}");
            assert!(err.message.contains(fragment), "{text:?}: {}", err.message);
        }
    }

    #[test]
    fn shell_split_matches_shlex() {
        assert_eq!(shell_split("profile a").unwrap(), ["profile", "a"]);
        assert_eq!(shell_split("  profile   a  ").unwrap(), ["profile", "a"]);
        assert_eq!(shell_split("profile \"a b\"").unwrap(), ["profile", "a b"]);
        assert_eq!(shell_split("profile 'a\\b'").unwrap(), ["profile", "a\\b"]);
        assert_eq!(shell_split("profile a\\ b").unwrap(), ["profile", "a b"]);
        assert_eq!(shell_split("profile \"a"), None);
    }

    #[test]
    fn config_path_resolution() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            resolve_config_path(None, home.clone()),
            Some(PathBuf::from("/home/u/.aws/config"))
        );
        assert_eq!(
            resolve_config_path(Some("".into()), home.clone()),
            Some(PathBuf::from("/home/u/.aws/config"))
        );
        assert_eq!(
            resolve_config_path(Some("/etc/aws.conf".into()), home.clone()),
            Some(PathBuf::from("/etc/aws.conf"))
        );
        assert_eq!(
            resolve_config_path(Some("~/alt/config".into()), home),
            Some(PathBuf::from("/home/u/alt/config"))
        );
        assert_eq!(resolve_config_path(None, None), None);
    }

    #[test]
    fn load_distinguishes_missing_file() {
        let missing = std::env::temp_dir().join("lunge-test-definitely-missing-config");
        assert!(matches!(
            load(&missing, None),
            Err(ConfigError::NotFound(_))
        ));
    }

    #[test]
    fn secret_values_are_never_retained() {
        let config = parse(
            "\
[profile keys]
aws_access_key_id = AKIAEXAMPLEKEYID
aws_secret_access_key = SECRETVALUE
    SECRETCONTINUATION
aws_session_token = TOKENVALUE
region = eu-central-1
",
        )
        .unwrap();
        let debug = format!("{config:?}");
        for secret in [
            "AKIAEXAMPLEKEYID",
            "SECRETVALUE",
            "SECRETCONTINUATION",
            "TOKENVALUE",
        ] {
            assert!(!debug.contains(secret), "{secret} leaked: {debug}");
        }
        assert_eq!(
            config.access_key_profiles,
            [AccessKeyProfile {
                name: "keys".into(),
                region: Some("eu-central-1".into()),
                source: KeySource::ConfigFile,
            }]
        );
    }

    #[test]
    fn parse_errors_never_quote_file_content() {
        for text in [
            "[default]\nAKIASECRETLINE\n",
            "[default]\n= AKIASECRETLINE\n",
            "[AKIASECRETLINE\n",
            "AKIASECRETLINE = x\n",
        ] {
            let err = parse(text).unwrap_err();
            assert!(!err.to_string().contains("AKIASECRETLINE"), "{err}");
        }
    }

    #[test]
    fn credentials_file_profiles_are_merged_with_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config");
        let credentials_path = dir.path().join("credentials");
        std::fs::write(
            &config_path,
            "\
[default]
region = us-east-1
[profile work]
region = us-west-2
[profile both]
sso_session = s
[sso-session s]
sso_start_url = u
sso_region = r
",
        )
        .unwrap();
        std::fs::write(
            &credentials_path,
            "\
[default]
aws_access_key_id = AKIAEXAMPLE1
aws_secret_access_key = SECRET1
[work]
aws_access_key_id = AKIAEXAMPLE2
aws_secret_access_key = SECRET2
[both]
aws_access_key_id = AKIAEXAMPLE3
aws_secret_access_key = SECRET3
[region-only]
region = ap-south-1
",
        )
        .unwrap();

        let config = load(&config_path, Some(&credentials_path)).unwrap();

        assert!(config.issues.is_empty(), "{:?}", config.issues);
        assert_eq!(
            config.access_key_profiles,
            [
                AccessKeyProfile {
                    name: "default".into(),
                    region: Some("us-east-1".into()),
                    source: KeySource::CredentialsFile,
                },
                AccessKeyProfile {
                    name: "work".into(),
                    region: Some("us-west-2".into()),
                    source: KeySource::CredentialsFile,
                },
            ]
        );
        assert_eq!(config.sessions[0].profiles[0].name, "both");
        assert!(!format!("{config:?}").contains("SECRET"));
    }

    #[test]
    fn credentials_file_problems_are_issues_not_errors() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config");
        std::fs::write(&config_path, "[default]\n").unwrap();

        let missing = dir.path().join("no-credentials");
        let config = load(&config_path, Some(&missing)).unwrap();
        assert!(config.issues.is_empty());

        let broken = dir.path().join("credentials");
        std::fs::write(&broken, "aws_secret_access_key = SECRETBEFORESECTION\n").unwrap();
        let config = load(&config_path, Some(&broken)).unwrap();
        let [ConfigIssue::CredentialsFile(message)] = config.issues.as_slice() else {
            panic!("{:?}", config.issues)
        };
        assert!(!message.contains("SECRET"), "{message}");
    }
}
