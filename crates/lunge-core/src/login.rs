//! Logging in to an SSO session by running `aws sso login`.
//!
//! NO CREDENTIAL CUSTODY: Lunge never handles the token a login produces.
//! The AWS CLI runs the browser authorization flow and writes the token to
//! its own cache; Lunge then only *reads* the cache (via [`crate::cache`]) to
//! confirm the session is live. The CLI's printed output holds the
//! authorization URL and, for the device-code flow, a one-time user code,
//! never tokens.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use time::OffsetDateTime;

use crate::cache::{self, SessionState};

/// Most lines of CLI output kept per login; the flow prints only a handful.
const MAX_OUTPUT_LINES: usize = 200;

/// How to invoke the AWS CLI.
#[derive(Debug, Clone)]
pub struct AwsCli {
    program: PathBuf,
    leading_args: Vec<OsString>,
}

impl Default for AwsCli {
    /// `aws`, found on `PATH`.
    fn default() -> Self {
        Self::new("aws")
    }
}

impl AwsCli {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            leading_args: Vec::new(),
        }
    }

    /// Starts `aws sso login --sso-session <session>`. The CLI opens the
    /// browser itself. Returns immediately; poll the returned process.
    pub fn start_login(&self, session: &str, cache_dir: &Path) -> Result<LoginProcess, LoginError> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.leading_args)
            .args(["sso", "login", "--sso-session", session])
            // An AWS_CLI_AUTO_PROMPT setting would otherwise make the CLI
            // wait on stdin, which Lunge doesn't provide.
            .arg("--no-cli-auto-prompt")
            // --sso-session needs no profile; a stale AWS_PROFILE naming a
            // missing profile would make the CLI fail before logging in.
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_DEFAULT_PROFILE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => LoginError::CliNotFound(self.program.clone()),
            _ => LoginError::Spawn(e),
        })?;

        let output = Arc::new(Mutex::new(Vec::new()));
        // Readers are detached, never joined: a browser the CLI launches can
        // inherit these pipes and hold them open long after the CLI exits.
        if let Some(stdout) = child.stdout.take() {
            spawn_line_reader(stdout, Arc::clone(&output));
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_line_reader(stderr, Arc::clone(&output));
        }

        Ok(LoginProcess {
            child,
            session: session.to_string(),
            cache_dir: cache_dir.to_path_buf(),
            output,
            cancelled: false,
            finished: false,
        })
    }
}

fn spawn_line_reader(stream: impl Read + Send + 'static, output: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
            if output.len() < MAX_OUTPUT_LINES {
                output.push(line);
            }
        }
    });
}

/// A running `aws sso login`. Dropping it stops the CLI if still running.
pub struct LoginProcess {
    child: Child,
    session: String,
    cache_dir: PathBuf,
    output: Arc<Mutex<Vec<String>>>,
    cancelled: bool,
    finished: bool,
}

impl LoginProcess {
    pub fn session(&self) -> &str {
        &self.session
    }

    /// What the CLI has printed so far (stdout and stderr, interleaved):
    /// typically the authorization URL to open if the browser didn't.
    pub fn output(&self) -> Vec<String> {
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Checks without blocking. `None` while the CLI is still running.
    ///
    /// Once the CLI exits successfully, success is confirmed by re-reading
    /// the CLI's token cache rather than trusting the exit code alone.
    pub fn poll(&mut self) -> Option<Result<OffsetDateTime, LoginError>> {
        if self.finished {
            return None;
        }
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status,
            Ok(None) => return None,
            Err(e) => return Some(self.finish(Err(LoginError::Spawn(e)))),
        };
        Some(self.finish(self.outcome(status)))
    }

    /// Blocks until the CLI exits.
    pub fn wait(mut self) -> Result<OffsetDateTime, LoginError> {
        let result = match self.child.wait() {
            Ok(status) => self.outcome(status),
            Err(e) => Err(LoginError::Spawn(e)),
        };
        self.finish(result)
    }

    /// Stops the CLI. The next [`poll`](Self::poll) reports
    /// [`LoginError::Cancelled`].
    pub fn cancel(&mut self) {
        self.cancelled = true;
        let _ = self.child.kill();
    }

    fn outcome(&self, status: ExitStatus) -> Result<OffsetDateTime, LoginError> {
        if self.cancelled {
            return Err(LoginError::Cancelled);
        }
        if !status.success() {
            return Err(LoginError::Failed {
                status,
                output: self.output(),
            });
        }
        // NO CREDENTIAL CUSTODY: read-only check of the CLI's cache.
        match cache::session_state(&self.cache_dir, &self.session, OffsetDateTime::now_utc()) {
            SessionState::Live { expires_at } => Ok(expires_at),
            state => Err(LoginError::NotLive(state)),
        }
    }

    fn finish<T>(&mut self, result: T) -> T {
        self.finished = true;
        result
    }
}

impl Drop for LoginProcess {
    fn drop(&mut self) {
        if !self.finished && matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Why a login didn't produce a live session.
#[derive(Debug)]
pub enum LoginError {
    /// The AWS CLI isn't installed or isn't on `PATH`.
    CliNotFound(PathBuf),
    /// The CLI couldn't be started or waited on.
    Spawn(io::Error),
    /// The CLI exited with an error; `output` is what it printed.
    Failed {
        status: ExitStatus,
        output: Vec<String>,
    },
    /// The CLI reported success, but the cache doesn't show a live token.
    NotLive(SessionState),
    Cancelled,
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliNotFound(program) => write!(
                f,
                "the AWS CLI ({}) wasn't found; install AWS CLI v2 and make sure it's on PATH",
                program.display()
            ),
            Self::Spawn(e) => write!(f, "couldn't run the AWS CLI: {e}"),
            Self::Failed { status, output } => {
                write!(f, "aws sso login failed ({status})")?;
                match output.iter().rev().find(|l| !l.trim().is_empty()) {
                    Some(last) => write!(f, ": {}", last.trim()),
                    None => Ok(()),
                }
            }
            Self::NotLive(state) => write!(
                f,
                "aws sso login finished, but the session is still {}",
                state.to_string().to_lowercase()
            ),
            Self::Cancelled => write!(f, "login cancelled"),
        }
    }
}

impl std::error::Error for LoginError {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Runs `script` with /bin/sh in place of the real CLI. (Going through
    /// sh avoids ETXTBSY races from exec'ing a freshly written file while
    /// other test threads fork.)
    fn fake_cli(dir: &Path, script: &str) -> AwsCli {
        let path = dir.join("fake-aws.sh");
        std::fs::write(&path, script).unwrap();
        AwsCli {
            program: "/bin/sh".into(),
            leading_args: vec![path.into()],
        }
    }

    fn token_json(expires_at: OffsetDateTime) -> String {
        let ts = expires_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        format!(r#"{{"accessToken": "PLACEHOLDER", "expiresAt": "{ts}"}}"#)
    }

    #[test]
    fn successful_login_is_confirmed_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache_file = cache::cache_file_for_session(dir.path(), "work");
        let expires = (OffsetDateTime::now_utc() + time::Duration::hours(8))
            .replace_nanosecond(0)
            .unwrap();
        let json_path = dir.path().join("token.json");
        std::fs::write(&json_path, token_json(expires)).unwrap();
        let cli = fake_cli(
            dir.path(),
            &format!(
                r#"[ "$*" = "sso login --sso-session work --no-cli-auto-prompt" ] || {{ echo "bad args: $*" >&2; exit 9; }}
[ -z "$AWS_PROFILE" ] || {{ echo "AWS_PROFILE leaked" >&2; exit 8; }}
echo "Attempting to automatically open the SSO authorization page"
echo "https://oidc.us-east-1.amazonaws.com/authorize?example" >&2
cp "{}" "{}"
echo "Successfully logged into Start URL: https://example.awsapps.com/start"
"#,
                json_path.display(),
                cache_file.display()
            ),
        );

        let login = cli.start_login("work", dir.path()).unwrap();
        assert_eq!(login.session(), "work");
        let result = login.wait();
        assert_eq!(result.unwrap(), expires);
    }

    #[test]
    fn cli_failure_is_surfaced_with_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(
            dir.path(),
            "echo 'Error when retrieving token from sso: Token has expired and refresh failed' >&2\nexit 255\n",
        );
        let err = cli
            .start_login("work", dir.path())
            .unwrap()
            .wait()
            .unwrap_err();
        let LoginError::Failed { status, .. } = &err else {
            panic!("{err:?}")
        };
        assert_eq!(status.code(), Some(255));
        assert!(err.to_string().contains("refresh failed"), "{err}");
    }

    #[test]
    fn exit_zero_without_a_fresh_token_is_not_success() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), "exit 0\n");
        let err = cli
            .start_login("work", dir.path())
            .unwrap()
            .wait()
            .unwrap_err();
        assert!(
            matches!(err, LoginError::NotLive(SessionState::NotLoggedIn)),
            "{err:?}"
        );
    }

    #[test]
    fn missing_cli_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let cli = AwsCli::new(dir.path().join("no-such-aws"));
        let Err(err) = cli.start_login("work", dir.path()) else {
            panic!("spawn should fail")
        };
        assert!(matches!(err, LoginError::CliNotFound(_)), "{err:?}");
    }

    #[test]
    fn poll_reports_progress_output_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo 'open this URL'\nexec sleep 30\n");
        let mut login = cli.start_login("work", dir.path()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while login.output().is_empty() {
            assert!(Instant::now() < deadline, "no output seen");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(login.output(), ["open this URL"]);
        assert!(login.poll().is_none(), "still running");

        login.cancel();
        let result = loop {
            if let Some(result) = login.poll() {
                break result;
            }
            assert!(Instant::now() < deadline, "cancel didn't stop the CLI");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(matches!(result, Err(LoginError::Cancelled)), "{result:?}");
        assert!(login.poll().is_none(), "reports only once");
    }
}
