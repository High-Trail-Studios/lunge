//! Adding an `[sso-session]` block, through the AWS CLI.
//!
//! PREFER THE CLI (owner decision): `aws configure set` can't write
//! `[sso-session]` blocks (asked to, it writes a bogus `sso-session = …` line
//! under `[default]` and reports success), and `aws configure sso-session`
//! has no flags, only prompts. So Lunge runs `aws configure sso-session` and
//! pipes in the four answers, in the order the CLI asks:
//!
//! 1. SSO session name
//! 2. SSO start URL
//! 3. SSO region
//! 4. SSO registration scopes
//!
//! The CLI writes the file only after the last answer, and if it asks
//! anything more (a newer CLI with an extra prompt, or a re-prompt after it
//! rejects an answer) it hits end of input and exits without writing. To
//! keep the prompt sequence predictable, Lunge refuses names that already
//! exist (the CLI then shows current values) and answers containing line
//! breaks. Afterwards it re-reads the config and checks the block holds
//! exactly what was entered, so a misplaced answer is reported, not hidden.
//!
//! No credentials are involved: a session block only records where and how
//! to sign in. Signing in is a separate `aws sso login` (see
//! [`crate::login`]).

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::{self, ConfigError};
use crate::login::AwsCli;

/// The scope `aws configure sso` offers by default; what nearly everyone
/// needs to list accounts and get role credentials.
pub const DEFAULT_SCOPES: &str = "sso:account:access";

/// The CLI's prompts, in the order it asks them (for showing the user what
/// will be typed).
pub const PROMPTS: [&str; 4] = [
    "SSO session name",
    "SSO start URL",
    "SSO region",
    "SSO registration scopes",
];

/// The answers to `aws configure sso-session`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewSession {
    pub name: String,
    pub start_url: String,
    pub region: String,
    /// Comma-separated. Spaces around commas are dropped.
    pub scopes: String,
}

impl NewSession {
    /// The answers as they'll be typed, in prompt order: trimmed, scopes
    /// joined with bare commas.
    pub fn answers(&self) -> [String; 4] {
        [
            self.name.trim().to_string(),
            self.start_url.trim().to_string(),
            self.region.trim().to_string(),
            normalize_scopes(&self.scopes),
        ]
    }

    /// Why these answers can't be used, if they can't. Doesn't check whether
    /// the name is taken.
    pub fn validate(&self) -> Result<(), String> {
        let [name, start_url, region, scopes] = self.answers();
        if name.is_empty() {
            return Err("enter a session name".into());
        }
        if let Some(c) = name
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        {
            let found = if c.is_whitespace() {
                "spaces aren't allowed in the session name".to_string()
            } else {
                format!("'{c}' isn't allowed in the session name")
            };
            return Err(format!("{found}; use letters, digits, '-', '_' or '.'"));
        }
        if start_url.is_empty() {
            return Err("enter your start URL (from your AWS access portal)".into());
        }
        if !start_url.starts_with("https://") || start_url.len() <= "https://".len() {
            return Err("the start URL should begin with https://".into());
        }
        if start_url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("the start URL can't contain spaces".into());
        }
        if region.is_empty() {
            return Err("enter the SSO region (where IAM Identity Center runs)".into());
        }
        let region_ok = region.starts_with(|c: char| c.is_ascii_lowercase())
            && region.contains('-')
            && region.chars().any(|c| c.is_ascii_digit())
            && region
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !region_ok {
            return Err("the SSO region should look like us-east-1".into());
        }
        if scopes.is_empty() {
            return Err(format!(
                "enter at least one scope (usually {DEFAULT_SCOPES})"
            ));
        }
        if scopes
            .split(',')
            .any(|s| s.is_empty() || s.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err("separate scopes with commas; a scope can't contain spaces".into());
        }
        Ok(())
    }
}

fn normalize_scopes(scopes: &str) -> String {
    let parts: Vec<&str> = scopes.split(',').map(str::trim).collect();
    if parts.iter().all(|p| p.is_empty()) {
        return String::new();
    }
    parts.join(",")
}

#[derive(Debug)]
pub enum SessionError {
    Invalid(String),
    NameTaken(String),
    Config(ConfigError),
    CliNotFound(PathBuf),
    Spawn(io::Error),
    /// The CLI exited with an error; it writes nothing in that case.
    Failed(String),
    /// The CLI reported success but the block isn't what was entered.
    Mismatch(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "{message}"),
            Self::NameTaken(name) => write!(f, "an SSO session named \"{name}\" already exists"),
            Self::Config(e) => write!(f, "{e}"),
            Self::CliNotFound(program) => write!(
                f,
                "the AWS CLI ({}) wasn't found; install AWS CLI v2 and make sure it's on PATH",
                program.display()
            ),
            Self::Spawn(e) => write!(f, "couldn't run the AWS CLI: {e}"),
            Self::Failed(message) => write!(f, "aws configure sso-session failed: {message}"),
            Self::Mismatch(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl AwsCli {
    /// Adds `[sso-session NAME]` to the config file at `config_path` by
    /// answering `aws configure sso-session`'s prompts, then checks what the
    /// CLI wrote. Blocking: call it off the UI thread.
    pub fn create_sso_session(
        &self,
        config_path: &Path,
        new: &NewSession,
    ) -> Result<(), SessionError> {
        new.validate().map_err(SessionError::Invalid)?;
        let answers = new.answers();
        let name = &answers[0];
        let taken = config::sso_session_names(config_path).map_err(SessionError::Config)?;
        if taken.contains(name) {
            return Err(SessionError::NameTaken(name.clone()));
        }

        let mut child = self
            .command()
            .args(["configure", "sso-session"])
            .arg("--no-cli-auto-prompt")
            // Write exactly the file Lunge read.
            .env("AWS_CONFIG_FILE", config_path)
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_DEFAULT_PROFILE")
            .stdin(Stdio::piped())
            // The echoed prompts and answers; nothing to show.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => SessionError::CliNotFound(self.program.clone()),
                _ => SessionError::Spawn(e),
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            let input: String = answers.iter().map(|a| format!("{a}\n")).collect();
            // A write error means the CLI exited early; its status says why.
            let _ = stdin.write_all(input.as_bytes());
            // Dropping stdin closes it, so any further prompt sees end of
            // input instead of waiting forever.
        }
        let output = child.wait_with_output().map_err(SessionError::Spawn)?;
        if !output.status.success() {
            return Err(SessionError::Failed(cli_error(&output.stderr)));
        }
        verify(config_path, &answers)
    }
}

/// The CLI's last meaningful stderr line. When a prompt runs out of input it
/// prints only "aws: [ERROR]:", so say what that means.
fn cli_error(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("Warning: Input is not a terminal"))
        .unwrap_or("");
    let detail = line.trim_start_matches("aws: [ERROR]:").trim();
    if detail.is_empty() {
        "the CLI didn't accept the answers and stopped without writing anything; \
         check the start URL and region, or use the command in a terminal"
            .into()
    } else {
        detail.to_string()
    }
}

/// Checks the CLI wrote the block with exactly the answers given.
fn verify(config_path: &Path, answers: &[String; 4]) -> Result<(), SessionError> {
    let [name, start_url, region, scopes] = answers;
    let settings = config::sso_session_settings(config_path, name)
        .map_err(SessionError::Config)?
        .ok_or_else(|| {
            SessionError::Mismatch(format!(
                "the AWS CLI reported success but [sso-session {name}] isn't in {}; \
                 run `aws configure sso-session` in a terminal instead",
                config_path.display()
            ))
        })?;
    for (key, expected) in [
        ("sso_start_url", start_url),
        ("sso_region", region),
        ("sso_registration_scopes", scopes),
    ] {
        let found = settings
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str());
        if found != Some(expected.as_str()) {
            return Err(SessionError::Mismatch(format!(
                "the AWS CLI wrote [sso-session {name}], but its {key} is {} instead of \
                 \"{expected}\" (this AWS CLI version may ask different questions); \
                 check that block in {}",
                found.map_or("missing".to_string(), |v| format!("\"{v}\"")),
                config_path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn new_session(name: &str) -> NewSession {
        NewSession {
            name: name.into(),
            start_url: " https://example.awsapps.com/start ".into(),
            region: "us-east-1".into(),
            scopes: "sso:account:access , sso:other".into(),
        }
    }

    /// A fake `aws configure sso-session`: logs its args and config path,
    /// reads four answers, and appends a block built from them in
    /// `write_order` (or exits with an error when `write_order` is empty).
    fn fake_cli(dir: &Path, write_order: &str) -> (AwsCli, PathBuf) {
        let log = dir.join("calls.log");
        let script = format!(
            r#"echo "$* config=$AWS_CONFIG_FILE profile=$AWS_PROFILE" >> "{log}"
read name; read url; read region; read scopes
[ -z "{write_order}" ] && {{ echo "Warning: Input is not a terminal (fd=0)." >&2; echo "aws: [ERROR]:" >&2; exit 255; }}
set -- $(echo "{write_order}")
v() {{ case $1 in url) echo "$url";; region) echo "$region";; scopes) echo "$scopes";; esac; }}
printf '[sso-session %s]\nsso_start_url = %s\nsso_region = %s\nsso_registration_scopes = %s\n' "$name" "$(v $1)" "$(v $2)" "$(v $3)" >> "$AWS_CONFIG_FILE"
"#,
            log = log.display()
        );
        (AwsCli::test_script(dir, &script), log)
    }

    #[test]
    fn answers_prompts_in_order_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "[profile a]\nregion = us-east-1\n").unwrap();
        let (cli, log) = fake_cli(dir.path(), "url region scopes");
        cli.create_sso_session(&config, &new_session("work"))
            .unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains(
            "[sso-session work]\nsso_start_url = https://example.awsapps.com/start\n\
             sso_region = us-east-1\nsso_registration_scopes = sso:account:access,sso:other\n"
        ));
        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(
            calls,
            format!(
                "configure sso-session --no-cli-auto-prompt config={} profile=\n",
                config.display()
            )
        );
    }

    #[test]
    fn works_without_a_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let (cli, _) = fake_cli(dir.path(), "url region scopes");
        cli.create_sso_session(&config, &new_session("work"))
            .unwrap();
        assert!(config.exists());
    }

    #[test]
    fn reports_answers_landing_in_the_wrong_place() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let (cli, _) = fake_cli(dir.path(), "url scopes region");
        let err = cli
            .create_sso_session(&config, &new_session("work"))
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("sso_region is \"sso:account:access,sso:other\""),
            "{text}"
        );
    }

    #[test]
    fn explains_a_bare_cli_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let (cli, _) = fake_cli(dir.path(), "");
        let err = cli
            .create_sso_session(&config, &new_session("work"))
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("stopped without writing anything"), "{text}");
        assert!(!config.exists());
    }

    #[test]
    fn refuses_taken_or_invalid_input_without_running_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            "[sso-session existing]\nsso_start_url = https://x.awsapps.com/start\n\
             [sso-session 'half']\n",
        )
        .unwrap();
        let (cli, log) = fake_cli(dir.path(), "url region scopes");
        let cases = [
            (new_session("existing"), "already exists"),
            (new_session("half"), "already exists"),
            (new_session("has space"), "spaces"),
            (new_session("a]b"), "']'"),
            (
                NewSession {
                    start_url: "http://x.awsapps.com/start".into(),
                    ..new_session("ok")
                },
                "https://",
            ),
            (
                NewSession {
                    start_url: "https://x.awsapps.com/start\nus-east-1".into(),
                    ..new_session("ok")
                },
                "spaces",
            ),
            (
                NewSession {
                    region: "US East".into(),
                    ..new_session("ok")
                },
                "us-east-1",
            ),
            (
                NewSession {
                    scopes: " , ".into(),
                    ..new_session("ok")
                },
                "at least one scope",
            ),
            (
                NewSession {
                    scopes: "a,,b".into(),
                    ..new_session("ok")
                },
                "commas",
            ),
        ];
        for (new, expected) in cases {
            let err = cli.create_sso_session(&config, &new).unwrap_err();
            assert!(err.to_string().contains(expected), "{new:?}: {err}");
        }
        assert!(!log.exists(), "CLI was run");
    }

    /// Checks the prompt order against the real AWS CLI on `PATH`, writing
    /// only to a temporary config. Run after upgrading the CLI:
    /// `cargo test -p lunge-core -- --ignored real_cli`
    #[test]
    #[ignore = "needs the AWS CLI on PATH"]
    fn real_cli_writes_the_block() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "[profile keep]\nregion = us-east-1\n").unwrap();
        // create_sso_session re-reads the block and fails on any mismatch.
        AwsCli::default()
            .create_sso_session(&config, &new_session("work"))
            .unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("[profile keep]"), "{text}");
    }
}
