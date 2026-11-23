//! Adding an `[sso-session]` block, through the AWS CLI.
//!
//! PREFER THE CLI (owner decision): Lunge never writes the block itself.
//! AWS CLI 2.34.22 and newer can write it directly with
//! `aws configure set KEY VALUE --sso-session NAME`, one key at a time, in
//! the order `aws configure sso-session` writes them:
//!
//! 1. `sso_start_url`
//! 2. `sso_region`
//! 3. `sso_registration_scopes`
//!
//! That works the same on every platform. Older CLIs reject `--sso-session`
//! as an unknown option before writing anything. On macOS and Linux Lunge
//! then falls back to running `aws configure sso-session` and piping in the
//! four answers to its prompts (name, start URL, region, scopes). That
//! fallback can't work on Windows: the CLI's prompt library reads keystrokes
//! from the console, not from stdin, and fails with "No Windows console
//! found" when there isn't one. So on Windows an old CLI gets a "please
//! update" error instead.
//!
//! Either way, Lunge refuses names that already exist and answers
//! containing line breaks first, and afterwards re-reads the config and
//! checks the block holds exactly what was entered, so a misplaced or
//! partial write is reported, not hidden.
//!
//! No credentials are involved: a session block only records where and how
//! to sign in. Signing in is a separate `aws sso login` (see
//! [`crate::login`]).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::{self, ConfigError};
use crate::login::AwsCli;

/// The scope `aws configure sso` offers by default; what nearly everyone
/// needs to list accounts and get role credentials.
pub const DEFAULT_SCOPES: &str = "sso:account:access";

/// The first AWS CLI release with `aws configure set --sso-session`.
pub const MIN_CLI_FOR_SET: &str = "2.34.22";

/// The keys a session block gets, in the order they're written; they take
/// `answers()[1..]`.
const KEYS: [&str; 3] = ["sso_start_url", "sso_region", "sso_registration_scopes"];

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

    /// The `aws configure set` commands Save runs, for display.
    pub fn commands(&self) -> Vec<String> {
        let answers = self.answers();
        let name = if answers[0].is_empty() {
            "NAME"
        } else {
            answers[0].as_str()
        };
        KEYS.iter()
            .zip(&answers[1..])
            .map(|(key, value)| format!("aws configure set {key} {value} --sso-session {name}"))
            .collect()
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
    /// The CLI exited with an error before writing anything.
    Failed(String),
    /// The CLI wrote some keys of the block, then failed.
    Partial {
        name: String,
        written: Vec<&'static str>,
        message: String,
    },
    /// The CLI has no `configure set --sso-session` and the prompt fallback
    /// can't run here (Windows).
    CliTooOld,
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
            Self::Failed(message) => write!(f, "the AWS CLI couldn't add the session: {message}"),
            Self::Partial {
                name,
                written,
                message,
            } => write!(
                f,
                "the AWS CLI added [sso-session {name}] with only {} and then failed: {message}. \
                 Finish it with `aws configure sso-session` in a terminal, or remove it from \
                 your AWS config",
                written.join(", ")
            ),
            Self::CliTooOld => write!(
                f,
                "this AWS CLI is too old to add a session from Lunge on this system; update \
                 to AWS CLI {MIN_CLI_FOR_SET} or newer, or run `aws configure sso-session` \
                 in a terminal"
            ),
            Self::Mismatch(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// How one `aws configure set` call went.
enum SetFailure {
    /// The CLI doesn't know `--sso-session` (before 2.34.22); nothing was
    /// written.
    Unsupported,
    Other(SessionError),
}

impl AwsCli {
    /// Adds `[sso-session NAME]` to the config file at `config_path` through
    /// the AWS CLI, then checks what the CLI wrote. Blocking: call it off
    /// the UI thread.
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

        for (i, (key, value)) in KEYS.iter().zip(&answers[1..]).enumerate() {
            match self.set_session_key(config_path, name, key, value) {
                Ok(()) => {}
                // Only the first call can find the option missing; nothing
                // has been written yet.
                Err(SetFailure::Unsupported) if i == 0 => {
                    return self.answer_prompts(config_path, &answers);
                }
                Err(SetFailure::Unsupported) => {
                    return Err(SessionError::Partial {
                        name: name.clone(),
                        written: KEYS[..i].to_vec(),
                        message: "the CLI stopped accepting --sso-session".into(),
                    });
                }
                Err(SetFailure::Other(SessionError::Failed(message))) if i > 0 => {
                    return Err(SessionError::Partial {
                        name: name.clone(),
                        written: KEYS[..i].to_vec(),
                        message,
                    });
                }
                Err(SetFailure::Other(e)) => return Err(e),
            }
        }
        verify(config_path, &answers)
    }

    /// Runs `aws configure set KEY VALUE --sso-session NAME`.
    fn set_session_key(
        &self,
        config_path: &Path,
        name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), SetFailure> {
        let output = self
            .command()
            // The option goes last so an old CLI reports it as unknown
            // rather than misreading the positionals.
            .args(["configure", "set", key, value, "--sso-session", name])
            .arg("--no-cli-auto-prompt")
            // Write exactly the file Lunge read.
            .env("AWS_CONFIG_FILE", config_path)
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_DEFAULT_PROFILE")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| SetFailure::Other(self.spawn_error(e)))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Unknown options") && stderr.contains("--sso-session") {
            return Err(SetFailure::Unsupported);
        }
        Err(SetFailure::Other(SessionError::Failed(cli_error(
            &output.stderr,
        ))))
    }

    fn spawn_error(&self, e: io::Error) -> SessionError {
        match e.kind() {
            io::ErrorKind::NotFound => SessionError::CliNotFound(self.program.clone()),
            _ => SessionError::Spawn(e),
        }
    }

    /// The fallback for CLIs older than [`MIN_CLI_FOR_SET`]: runs
    /// `aws configure sso-session` and answers its four prompts on stdin
    /// (name, start URL, region, scopes). The CLI writes the file only after
    /// the last answer; if it asks anything more it hits end of input and
    /// exits without writing.
    ///
    /// Not on Windows: there the CLI's prompt library reads the console,
    /// not stdin, and Lunge's CLI processes have no console.
    #[cfg(windows)]
    fn answer_prompts(
        &self,
        _config_path: &Path,
        _answers: &[String; 4],
    ) -> Result<(), SessionError> {
        Err(SessionError::CliTooOld)
    }

    #[cfg(not(windows))]
    fn answer_prompts(
        &self,
        config_path: &Path,
        answers: &[String; 4],
    ) -> Result<(), SessionError> {
        use std::io::Write;

        let mut child = self
            .command()
            .args(["configure", "sso-session"])
            .arg("--no-cli-auto-prompt")
            .env("AWS_CONFIG_FILE", config_path)
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_DEFAULT_PROFILE")
            .stdin(Stdio::piped())
            // The echoed prompts and answers; nothing to show.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| self.spawn_error(e))?;
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
        verify(config_path, answers)
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
    for (key, expected) in KEYS.iter().zip([start_url, region, scopes]) {
        let found = settings
            .iter()
            .rev()
            .find(|(k, _)| k == *key)
            .map(|(_, v)| v.as_str());
        if found != Some(expected.as_str()) {
            return Err(SessionError::Mismatch(format!(
                "the AWS CLI wrote [sso-session {name}], but its {key} is {} instead of \
                 \"{expected}\"; check that block in {}",
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

    /// How the fake CLI handles `configure set --sso-session`.
    #[derive(Clone, Copy)]
    enum Set {
        /// Writes the key, like CLI 2.34.22+.
        Works,
        /// Rejects the option, like older CLIs.
        Unknown,
        /// Writes the first key, then fails.
        FailsSecond,
    }

    /// A fake AWS CLI: logs its args and config path. `configure set KEY
    /// VALUE --sso-session NAME` behaves as `set` says. `configure
    /// sso-session` reads four answers and appends a block built from them
    /// in `write_order` (or exits with an error when `write_order` is
    /// empty).
    fn fake_cli(dir: &Path, set: Set, write_order: &str) -> (AwsCli, PathBuf) {
        let log = dir.join("calls.log");
        let set = match set {
            Set::Works => "works",
            Set::Unknown => "unknown",
            Set::FailsSecond => "fails-second",
        };
        let script = format!(
            r#"echo "$* config=$AWS_CONFIG_FILE profile=$AWS_PROFILE" >> "{log}"
if [ "$2" = set ]; then
  case {set} in
    unknown) echo "aws: [ERROR]: An error occurred (ParamValidation): Unknown options: --sso-session,$6" >&2; exit 252;;
    fails-second) [ "$3" = sso_region ] && {{ echo "aws: [ERROR]: disk full" >&2; exit 255; }};;
  esac
  grep -qxF "[sso-session $6]" "$AWS_CONFIG_FILE" 2>/dev/null || printf '[sso-session %s]\n' "$6" >> "$AWS_CONFIG_FILE"
  printf '%s = %s\n' "$3" "$4" >> "$AWS_CONFIG_FILE"
  exit 0
fi
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

    const BLOCK: &str = "[sso-session work]\nsso_start_url = https://example.awsapps.com/start\n\
                         sso_region = us-east-1\nsso_registration_scopes = sso:account:access,sso:other\n";

    fn set_call(key: &str, value: &str, config: &Path) -> String {
        format!(
            "configure set {key} {value} --sso-session work --no-cli-auto-prompt config={} profile=\n",
            config.display()
        )
    }

    #[test]
    fn sets_each_key_with_configure_set_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "[profile a]\nregion = us-east-1\n").unwrap();
        let (cli, log) = fake_cli(dir.path(), Set::Works, "");
        cli.create_sso_session(&config, &new_session("work"))
            .unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert_eq!(written, format!("[profile a]\nregion = us-east-1\n{BLOCK}"));
        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(
            calls,
            [
                set_call(
                    "sso_start_url",
                    "https://example.awsapps.com/start",
                    &config
                ),
                set_call("sso_region", "us-east-1", &config),
                set_call(
                    "sso_registration_scopes",
                    "sso:account:access,sso:other",
                    &config
                ),
            ]
            .concat()
        );
    }

    #[test]
    fn shows_the_commands_save_runs() {
        assert_eq!(
            new_session("work").commands(),
            [
                "aws configure set sso_start_url https://example.awsapps.com/start --sso-session work",
                "aws configure set sso_region us-east-1 --sso-session work",
                "aws configure set sso_registration_scopes sso:account:access,sso:other --sso-session work",
            ]
        );
        assert!(new_session(" ").commands()[0].ends_with("--sso-session NAME"));
    }

    #[test]
    fn works_without_a_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let (cli, _) = fake_cli(dir.path(), Set::Works, "");
        cli.create_sso_session(&config, &new_session("work"))
            .unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), BLOCK);
    }

    #[test]
    fn reports_a_partial_write() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let (cli, _) = fake_cli(dir.path(), Set::FailsSecond, "");
        let err = cli
            .create_sso_session(&config, &new_session("work"))
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("with only sso_start_url"), "{text}");
        assert!(text.contains("disk full"), "{text}");
    }

    #[test]
    fn falls_back_to_prompts_on_an_old_cli() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "[profile a]\nregion = us-east-1\n").unwrap();
        let (cli, log) = fake_cli(dir.path(), Set::Unknown, "url region scopes");
        cli.create_sso_session(&config, &new_session("work"))
            .unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert_eq!(written, format!("[profile a]\nregion = us-east-1\n{BLOCK}"));
        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(
            calls,
            format!(
                "{}configure sso-session --no-cli-auto-prompt config={} profile=\n",
                set_call(
                    "sso_start_url",
                    "https://example.awsapps.com/start",
                    &config
                ),
                config.display()
            )
        );
    }

    #[test]
    fn reports_answers_landing_in_the_wrong_place() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let (cli, _) = fake_cli(dir.path(), Set::Unknown, "url scopes region");
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
        let (cli, _) = fake_cli(dir.path(), Set::Unknown, "");
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
        let (cli, log) = fake_cli(dir.path(), Set::Works, "url region scopes");
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

    /// Checks the real AWS CLI on `PATH` writes the block (with `configure
    /// set`, or the prompt fallback on a CLI older than 2.34.22), writing
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
