//! Creating a profile for an account role, and changing a profile's default
//! region, through the AWS CLI.
//!
//! PREFER THE CLI: this runs `aws configure set <key> <value> --profile NAME`
//! once per setting, so the AWS CLI itself writes `~/.aws/config`; Lunge
//! never edits the file for this. No credentials are involved: the profile
//! only records which session, account, role, and region to use.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::{self, ConfigError};
use crate::login::AwsCli;
use crate::regions;
use crate::rename::{self, RenameError};

/// What to create.
#[derive(Debug, Clone)]
pub struct NewProfile {
    pub name: String,
    pub session: String,
    pub account_id: String,
    pub role_name: String,
    /// Default region for commands using this profile; `None` to leave it
    /// unset.
    pub region: Option<String>,
    /// Default output format (`json`, `yaml`, `yaml-stream`, `text`,
    /// `table`); `None` to leave it unset (the CLI then uses json).
    pub output: Option<String>,
}

/// The output formats the AWS CLI accepts for `output`.
pub const OUTPUT_FORMATS: [&str; 5] = ["json", "yaml", "yaml-stream", "text", "table"];

#[derive(Debug)]
pub enum CreateError {
    InvalidName(RenameError),
    NameTaken(String),
    Config(ConfigError),
    CliNotFound(PathBuf),
    Spawn(io::Error),
    /// `aws configure set` failed. If `partial`, earlier settings were
    /// already written, so a half-made profile exists (the CLI can't delete
    /// profiles, so it has to be removed by hand).
    Failed {
        key: &'static str,
        message: String,
        partial: bool,
        name: String,
    },
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(e) => write!(f, "{e}"),
            Self::NameTaken(name) => write!(f, "a profile named \"{name}\" already exists"),
            Self::Config(e) => write!(f, "{e}"),
            Self::CliNotFound(program) => write!(
                f,
                "the AWS CLI ({}) wasn't found; install AWS CLI v2 and make sure it's on PATH",
                program.display()
            ),
            Self::Spawn(e) => write!(f, "couldn't run the AWS CLI: {e}"),
            Self::Failed {
                key,
                message,
                partial,
                name,
            } => {
                write!(f, "aws configure set {key} failed: {message}")?;
                if *partial {
                    write!(
                        f,
                        ". Profile \"{name}\" was partly created; remove its [profile {name}] \
                         block from your AWS config by hand"
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CreateError {}

#[derive(Debug)]
pub enum UpdateError {
    InvalidRegion(String),
    /// No such profile in the config file. `aws configure set` would create
    /// one, so Lunge stops instead (e.g. it was renamed meanwhile).
    NoSuchProfile(String),
    Config(ConfigError),
    CliNotFound(PathBuf),
    Spawn(io::Error),
    Failed(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRegion(region) => write!(f, "\"{region}\" isn't a region code"),
            Self::NoSuchProfile(name) => write!(
                f,
                "profile \"{name}\" isn't in the config file anymore; refresh and try again"
            ),
            Self::Config(e) => write!(f, "{e}"),
            Self::CliNotFound(program) => write!(
                f,
                "the AWS CLI ({}) wasn't found; install AWS CLI v2 and make sure it's on PATH",
                program.display()
            ),
            Self::Spawn(e) => write!(f, "couldn't run the AWS CLI: {e}"),
            Self::Failed(message) => write!(f, "aws configure set region failed: {message}"),
        }
    }
}

impl std::error::Error for UpdateError {}

/// Why one `aws configure set` run failed.
enum SetFailure {
    CliNotFound,
    Spawn(io::Error),
    /// The CLI's last stderr line.
    Failed(String),
}

impl AwsCli {
    /// Runs `aws configure set KEY VALUE --profile PROFILE` against the
    /// config file at `config_path`.
    fn configure_set(
        &self,
        config_path: &Path,
        profile: &str,
        key: &str,
        value: &str,
    ) -> Result<(), SetFailure> {
        let output = self
            .command()
            .args(["configure", "set", key, value, "--profile", profile])
            .arg("--no-cli-auto-prompt")
            // Write exactly the file Lunge read.
            .env("AWS_CONFIG_FILE", config_path)
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_DEFAULT_PROFILE")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => SetFailure::CliNotFound,
                _ => SetFailure::Spawn(e),
            })?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no error output")
            .trim()
            .to_string();
        Err(SetFailure::Failed(message))
    }

    /// Sets an existing profile's default `region` with `aws configure set`.
    /// Blocking: call it off the UI thread.
    pub fn set_profile_region(
        &self,
        config_path: &Path,
        profile: &str,
        region: &str,
    ) -> Result<(), UpdateError> {
        if !regions::looks_like_region(region) {
            return Err(UpdateError::InvalidRegion(region.to_string()));
        }
        let names = config::all_profile_names(config_path, None).map_err(UpdateError::Config)?;
        if !names.iter().any(|n| n == profile) {
            return Err(UpdateError::NoSuchProfile(profile.to_string()));
        }
        self.configure_set(config_path, profile, "region", region)
            .map_err(|failure| match failure {
                SetFailure::CliNotFound => UpdateError::CliNotFound(self.program.clone()),
                SetFailure::Spawn(e) => UpdateError::Spawn(e),
                SetFailure::Failed(message) => UpdateError::Failed(message),
            })
    }

    /// Creates an SSO profile with `aws configure set`, writing to the
    /// config file at `config_path`. Blocking (runs the CLI up to five
    /// times): call it off the UI thread.
    pub fn create_sso_profile(
        &self,
        config_path: &Path,
        credentials_path: Option<&Path>,
        new: &NewProfile,
    ) -> Result<(), CreateError> {
        rename::validate_name(&new.name).map_err(CreateError::InvalidName)?;
        let taken = config::all_profile_names(config_path, credentials_path)
            .map_err(CreateError::Config)?;
        if taken.contains(&new.name) {
            return Err(CreateError::NameTaken(new.name.clone()));
        }

        // sso_session goes last: until it's written the profile isn't an SSO
        // profile, so an interrupted run is less likely to look usable.
        let mut settings = vec![
            ("sso_account_id", new.account_id.as_str()),
            ("sso_role_name", new.role_name.as_str()),
        ];
        if let Some(region) = new.region.as_deref().filter(|r| !r.is_empty()) {
            settings.push(("region", region));
        }
        if let Some(output) = new.output.as_deref().filter(|o| !o.is_empty()) {
            settings.push(("output", output));
        }
        settings.push(("sso_session", new.session.as_str()));

        for (index, (key, value)) in settings.into_iter().enumerate() {
            self.configure_set(config_path, &new.name, key, value)
                .map_err(|failure| match failure {
                    SetFailure::CliNotFound => CreateError::CliNotFound(self.program.clone()),
                    SetFailure::Spawn(e) => CreateError::Spawn(e),
                    SetFailure::Failed(message) => CreateError::Failed {
                        key,
                        message,
                        partial: index > 0,
                        name: new.name.clone(),
                    },
                })?;
        }
        Ok(())
    }
}

/// The shell a copied `AWS_PROFILE` line is meant for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// sh, bash, zsh, fish: `AWS_PROFILE=name aws ...` for one command.
    Posix,
    /// Windows PowerShell and PowerShell 7.
    PowerShell,
}

impl Shell {
    /// PowerShell on Windows, a POSIX shell elsewhere.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::PowerShell
        } else {
            Self::Posix
        }
    }
}

/// Text to paste before an `aws` command so it runs as `profile`, quoted
/// for `shell`. POSIX: `AWS_PROFILE=name ` applies to the command typed
/// after it. PowerShell has no per-command form, so `$env:AWS_PROFILE='name'; `
/// sets it for the rest of that PowerShell session.
pub fn profile_env_prefix(profile: &str, shell: Shell) -> String {
    match shell {
        Shell::Posix => {
            let plain = profile
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+'));
            if plain && !profile.is_empty() {
                format!("AWS_PROFILE={profile} ")
            } else {
                format!("AWS_PROFILE='{}' ", profile.replace('\'', r"'\''"))
            }
        }
        // Single quotes: PowerShell expands nothing inside them.
        Shell::PowerShell => format!("$env:AWS_PROFILE='{}'; ", profile.replace('\'', "''")),
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    #[test]
    fn env_prefix_per_shell() {
        assert_eq!(
            profile_env_prefix("work-prod.admin", Shell::Posix),
            "AWS_PROFILE=work-prod.admin "
        );
        assert_eq!(
            profile_env_prefix("my profile", Shell::Posix),
            "AWS_PROFILE='my profile' "
        );
        assert_eq!(
            profile_env_prefix("it's", Shell::Posix),
            r"AWS_PROFILE='it'\''s' "
        );
        assert_eq!(
            profile_env_prefix("work-prod", Shell::PowerShell),
            "$env:AWS_PROFILE='work-prod'; "
        );
        assert_eq!(
            profile_env_prefix("it's $x", Shell::PowerShell),
            "$env:AWS_PROFILE='it''s $x'; "
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn new_profile(name: &str) -> NewProfile {
        NewProfile {
            name: name.into(),
            session: "work".into(),
            account_id: "111122223333".into(),
            role_name: "AdministratorAccess".into(),
            region: Some("eu-west-1".into()),
            output: Some("json".into()),
        }
    }

    /// A fake `aws` that logs each call (args and AWS_CONFIG_FILE) and fails
    /// when asked to set `fail_key`.
    fn logging_cli(dir: &Path, fail_key: &str) -> (AwsCli, PathBuf) {
        let log = dir.join("calls.log");
        let script = format!(
            r#"echo "$* config=$AWS_CONFIG_FILE" >> "{log}"
[ "$3" = "{fail_key}" ] && {{ echo "aws: [ERROR]: simulated failure" >&2; exit 255; }}
exit 0
"#,
            log = log.display()
        );
        (AwsCli::test_script(dir, &script), log)
    }

    fn write_config(dir: &Path) -> PathBuf {
        let path = dir.join("config");
        std::fs::write(
            &path,
            "[default]\nregion = us-east-1\n[profile existing]\nsso_session = work\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn runs_configure_set_per_setting_with_session_last() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(dir.path());
        let (cli, log) = logging_cli(dir.path(), "none");
        cli.create_sso_profile(&config, None, &new_profile("work-prod-admin"))
            .unwrap();
        let calls = std::fs::read_to_string(log).unwrap();
        let config = config.display();
        assert_eq!(
            calls,
            format!(
                "configure set sso_account_id 111122223333 --profile work-prod-admin --no-cli-auto-prompt config={config}\n\
                 configure set sso_role_name AdministratorAccess --profile work-prod-admin --no-cli-auto-prompt config={config}\n\
                 configure set region eu-west-1 --profile work-prod-admin --no-cli-auto-prompt config={config}\n\
                 configure set output json --profile work-prod-admin --no-cli-auto-prompt config={config}\n\
                 configure set sso_session work --profile work-prod-admin --no-cli-auto-prompt config={config}\n"
            )
        );
    }

    #[test]
    fn refuses_taken_or_invalid_names_without_running_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(dir.path());
        let creds = dir.path().join("credentials");
        std::fs::write(&creds, "[keys-profile]\naws_access_key_id = AKIAEXAMPLE\n").unwrap();
        let (cli, log) = logging_cli(dir.path(), "none");
        for (name, expected) in [
            ("existing", "already exists"),
            ("default", "reserved"),
            ("keys-profile", "already exists"),
            ("has space", "spaces"),
        ] {
            let err = cli
                .create_sso_profile(&config, Some(&creds), &new_profile(name))
                .unwrap_err();
            assert!(err.to_string().contains(expected), "{name}: {err}");
        }
        assert!(!log.exists(), "CLI was run");
    }

    #[test]
    fn sets_region_only_on_an_existing_profile() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(dir.path());
        let (cli, log) = logging_cli(dir.path(), "none");
        cli.set_profile_region(&config, "existing", "ap-southeast-2")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!(
                "configure set region ap-southeast-2 --profile existing --no-cli-auto-prompt config={}\n",
                config.display()
            )
        );
        std::fs::remove_file(&log).unwrap();

        let err = cli
            .set_profile_region(&config, "missing", "us-east-1")
            .unwrap_err();
        assert!(matches!(err, UpdateError::NoSuchProfile(_)), "{err}");
        let err = cli
            .set_profile_region(&config, "existing", "us-east-1 --debug")
            .unwrap_err();
        assert!(matches!(err, UpdateError::InvalidRegion(_)), "{err}");
        assert!(!log.exists(), "CLI was run");

        let (cli, _) = logging_cli(dir.path(), "region");
        let err = cli
            .set_profile_region(&config, "existing", "us-east-1")
            .unwrap_err();
        assert!(err.to_string().contains("simulated failure"), "{err}");
    }

    #[test]
    fn reports_partial_creation_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(dir.path());
        let (cli, _) = logging_cli(dir.path(), "sso_role_name");
        let err = cli
            .create_sso_profile(&config, None, &new_profile("half"))
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("simulated failure"), "{text}");
        assert!(text.contains("partly created"), "{text}");

        let (cli, _) = logging_cli(dir.path(), "sso_account_id");
        let err = cli
            .create_sso_profile(&config, None, &new_profile("none"))
            .unwrap_err();
        assert!(!err.to_string().contains("partly created"), "{err}");
    }
}
