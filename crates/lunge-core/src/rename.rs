//! Renaming an SSO profile in `~/.aws/config`: the one change Lunge makes to
//! AWS files (an owner decision; everything else is read-only).
//!
//! The write is deliberately narrow and two-step. [`plan_profile_rename`]
//! reads the file, checks the rename is safe, and builds the new text,
//! changing only the profile's `[profile NAME]` header line; nothing is
//! written. [`RenamePlan::apply`] then refuses if the file changed since the
//! plan, saves a timestamped backup, writes the new text to a temporary file,
//! and swaps it in.
//!
//! NO CREDENTIAL CUSTODY: a config file that contains access keys is refused
//! outright, so Lunge never copies secret values into a new file or backup.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use time::OffsetDateTime;

use crate::config::{self, ParseError, SECRET_KEYS};

/// A checked, ready-to-write rename. Holds the config text, which contains
/// no secrets (see the module docs).
#[derive(Debug, Clone)]
pub struct RenamePlan {
    /// The file actually written (symlinks resolved).
    target: PathBuf,
    original: Vec<u8>,
    updated: String,
    pub old_name: String,
    pub new_name: String,
    /// The header line as it is now, and as it will be.
    pub before_line: String,
    pub after_line: String,
}

#[derive(Debug)]
pub enum RenameError {
    InvalidName(String),
    NameTaken(String),
    NotFound(String),
    /// `[default]` can't be renamed: plain `aws` commands rely on it.
    IsDefault,
    /// Not an `sso_session` profile; Lunge only renames SSO profiles.
    NotSso(String),
    /// Other profiles name this one in `source_profile`.
    ReferencedBy(Vec<String>),
    /// The config file holds access keys; Lunge won't rewrite it.
    ContainsSecrets,
    /// The file changed between the preview and the write.
    ChangedSincePreview,
    Parse(ParseError),
    Io(io::Error),
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(reason) => write!(f, "{reason}"),
            Self::NameTaken(name) => write!(f, "a profile named \"{name}\" already exists"),
            Self::NotFound(name) => write!(f, "profile \"{name}\" isn't in the config file"),
            Self::IsDefault => write!(f, "the default profile can't be renamed"),
            Self::NotSso(name) => write!(
                f,
                "\"{name}\" isn't an SSO profile; Lunge only renames SSO profiles"
            ),
            Self::ReferencedBy(names) => write!(
                f,
                "other profiles use this one as source_profile ({}); rename it by hand to update them too",
                names.join(", ")
            ),
            Self::ContainsSecrets => write!(
                f,
                "your config file contains access keys, so Lunge won't rewrite it; \
                 move keys to ~/.aws/credentials or rename the profile by hand"
            ),
            Self::ChangedSincePreview => write!(
                f,
                "the config file changed since the preview; nothing was written, try again"
            ),
            Self::Parse(e) => write!(f, "can't parse the config file: {e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RenameError {}

impl From<io::Error> for RenameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Checks whether `name` is acceptable as a new profile name. Stricter than
/// the CLI on purpose: names must be easy to type in `--profile` and
/// `AWS_PROFILE` without quoting.
pub fn validate_name(name: &str) -> Result<(), RenameError> {
    if name.is_empty() {
        return Err(RenameError::InvalidName("enter a name".into()));
    }
    if name == "default" {
        return Err(RenameError::InvalidName("\"default\" is reserved".into()));
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        let found = if c.is_whitespace() {
            "spaces aren't allowed".to_string()
        } else {
            format!("'{c}' isn't allowed")
        };
        return Err(RenameError::InvalidName(format!(
            "{found}; use letters, digits, '-', '_' or '.'"
        )));
    }
    Ok(())
}

/// Works out the rename without writing anything.
pub fn plan_profile_rename(
    config_path: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<RenamePlan, RenameError> {
    validate_name(new_name)?;
    if old_name == "default" {
        return Err(RenameError::IsDefault);
    }
    let target = std::fs::canonicalize(config_path)?;
    let original = std::fs::read(&target)?;
    let text = std::str::from_utf8(&original).map_err(|_| {
        RenameError::Parse(ParseError {
            line: 0,
            message: "file is not valid UTF-8".into(),
        })
    })?;
    let sections = config::parse_ini(text).map_err(RenameError::Parse)?;

    // NO CREDENTIAL CUSTODY: never rewrite (or back up) a file with keys.
    // parse_ini has already masked their values; this checks presence.
    if sections.iter().any(|s| {
        s.entries
            .iter()
            .any(|(k, _)| SECRET_KEYS.contains(&k.as_str()))
    }) {
        return Err(RenameError::ContainsSecrets);
    }

    let profile_name = |header: &str| {
        if header == "default" {
            Some("default".to_string())
        } else if header.starts_with("profile") {
            config::section_name_arg(header)
        } else {
            None
        }
    };
    if sections
        .iter()
        .any(|s| profile_name(&s.name).as_deref() == Some(new_name))
    {
        return Err(RenameError::NameTaken(new_name.to_string()));
    }
    let section = sections
        .iter()
        .find(|s| profile_name(&s.name).as_deref() == Some(old_name))
        .ok_or_else(|| RenameError::NotFound(old_name.to_string()))?;
    if !section.entries.iter().any(|(k, _)| k == "sso_session") {
        return Err(RenameError::NotSso(old_name.to_string()));
    }
    let referenced_by: Vec<String> = sections
        .iter()
        .filter(|s| {
            s.entries
                .iter()
                .any(|(k, v)| k == "source_profile" && v == old_name)
        })
        .filter_map(|s| profile_name(&s.name))
        .collect();
    if !referenced_by.is_empty() {
        return Err(RenameError::ReferencedBy(referenced_by));
    }

    // Replace just the header line, keeping its indentation, anything after
    // the closing bracket, and the file's line endings.
    let (start, end) = line_span(text, section.line);
    let before_line = text[start..end].to_string();
    let indent = &before_line[..before_line.len() - before_line.trim_start().len()];
    let trailing = before_line
        .rfind(']')
        .map(|i| &before_line[i + 1..])
        .unwrap_or("");
    let after_line = format!("{indent}[profile {new_name}]{trailing}");
    let updated = format!("{}{}{}", &text[..start], after_line, &text[end..]);

    // Belt and braces: the result must parse to the same sections, with only
    // this one renamed.
    let reparsed = config::parse_ini(&updated).map_err(RenameError::Parse)?;
    let same_shape = reparsed.len() == sections.len()
        && reparsed.iter().zip(&sections).all(|(new, old)| {
            new.entries == old.entries
                && if old.line == section.line {
                    profile_name(&new.name).as_deref() == Some(new_name)
                } else {
                    new.name == old.name
                }
        });
    if !same_shape {
        return Err(RenameError::Parse(ParseError {
            line: section.line,
            message: "the rename didn't produce the expected file; nothing was written".into(),
        }));
    }

    Ok(RenamePlan {
        target,
        original,
        updated,
        old_name: old_name.to_string(),
        new_name: new_name.to_string(),
        before_line: before_line.trim().to_string(),
        after_line: after_line.trim().to_string(),
    })
}

/// Byte range of 1-based `line` in `text`, excluding its line terminator.
fn line_span(text: &str, line: usize) -> (usize, usize) {
    let mut start = 0;
    for (index, chunk) in text.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            let content = chunk.trim_end_matches('\n').trim_end_matches('\r');
            return (start, start + content.len());
        }
        start += chunk.len();
    }
    (text.len(), text.len())
}

impl RenamePlan {
    /// The file that will be written (after resolving symlinks).
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Writes the rename. Returns the path of the backup of the original.
    pub fn apply(&self) -> Result<PathBuf, RenameError> {
        if std::fs::read(&self.target)? != self.original {
            return Err(RenameError::ChangedSincePreview);
        }
        let dir = self
            .target
            .parent()
            .ok_or_else(|| io::Error::other("config file has no parent directory"))?;
        let file_name = self
            .target
            .file_name()
            .ok_or_else(|| io::Error::other("config path has no file name"))?
            .to_string_lossy()
            .into_owned();
        let permissions = std::fs::metadata(&self.target)?.permissions();

        let now = OffsetDateTime::now_utc();
        let stamp = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        let backup = dir.join(format!("{file_name}.lunge-backup-{stamp}"));
        write_new_file(&backup, &self.original, &permissions)?;

        let temp = dir.join(format!(".{file_name}.lunge-tmp-{}", std::process::id()));
        if let Err(e) = write_new_file(&temp, self.updated.as_bytes(), &permissions)
            .and_then(|()| std::fs::rename(&temp, &self.target))
        {
            let _ = std::fs::remove_file(&temp);
            return Err(e.into());
        }
        Ok(backup)
    }
}

/// Creates `path` (failing if it exists), writes `bytes`, matches
/// `permissions`, and syncs to disk.
fn write_new_file(path: &Path, bytes: &[u8], permissions: &std::fs::Permissions) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.set_permissions(permissions.clone())?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
# my aws config
[default]
region = us-east-1

[profile AdministratorAccess-111122223333]
sso_session = work
sso_account_id = 111122223333
sso_role_name = AdministratorAccess

[sso-session work]
sso_start_url = https://example.awsapps.com/start
sso_region = us-east-1
";

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("config");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn rename_changes_only_the_header_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), CONFIG);

        let plan =
            plan_profile_rename(&path, "AdministratorAccess-111122223333", "work-admin").unwrap();
        assert_eq!(
            plan.before_line,
            "[profile AdministratorAccess-111122223333]"
        );
        assert_eq!(plan.after_line, "[profile work-admin]");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            CONFIG,
            "plan wrote"
        );

        let backup = plan.apply().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            CONFIG.replace(
                "[profile AdministratorAccess-111122223333]",
                "[profile work-admin]"
            )
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), CONFIG);
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config.lunge-backup-")
        );

        let parsed = config::load(&path, None).unwrap();
        assert_eq!(parsed.sessions[0].profiles[0].name, "work-admin");
    }

    #[test]
    fn preserves_crlf_and_trailing_text() {
        let dir = tempfile::tempdir().unwrap();
        let text = "[sso-session s]\r\nsso_start_url = u\r\nsso_region = r\r\n[profile old] \r\nsso_session = s\r\n";
        let path = write_config(dir.path(), text);
        plan_profile_rename(&path, "old", "new")
            .unwrap()
            .apply()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text.replace("[profile old]", "[profile new]")
        );
    }

    #[test]
    fn handles_quoted_and_spaced_headers() {
        let dir = tempfile::tempdir().unwrap();
        let text = "[sso-session s]\nsso_start_url = u\nsso_region = r\n[profile   \"old\"]\nsso_session = s\n";
        let path = write_config(dir.path(), text);
        plan_profile_rename(&path, "old", "new")
            .unwrap()
            .apply()
            .unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\n[profile new]\n")
        );
    }

    #[test]
    fn refuses_unsafe_renames() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "\
[default]
sso_session = s
[profile a]
sso_session = s
[profile b]
sso_session = s
[profile keys-in-credentials]
region = us-east-1
[profile chained]
source_profile = a
role_arn = arn:aws:iam::111122223333:role/x
[sso-session s]
sso_start_url = u
sso_region = r
",
        );
        type Check = fn(&RenameError) -> bool;
        let cases: [(&str, &str, Check); 7] = [
            ("b", "a", |e| matches!(e, RenameError::NameTaken(_))),
            ("b", "default", |e| matches!(e, RenameError::InvalidName(_))),
            ("b", "has space", |e| {
                matches!(e, RenameError::InvalidName(_))
            }),
            ("default", "x", |e| matches!(e, RenameError::IsDefault)),
            ("missing", "x", |e| matches!(e, RenameError::NotFound(_))),
            ("keys-in-credentials", "x", |e| {
                matches!(e, RenameError::NotSso(_))
            }),
            (
                "a",
                "x",
                |e| matches!(e, RenameError::ReferencedBy(names) if names == &["chained"]),
            ),
        ];
        for (old, new, expected) in cases {
            let err = plan_profile_rename(&path, old, new).unwrap_err();
            assert!(expected(&err), "{old} -> {new}: {err:?}");
        }
    }

    #[test]
    fn refuses_config_files_holding_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "[profile old]\nsso_session = s\n[profile k]\naws_access_key_id = AKIAEXAMPLE\naws_secret_access_key = SECRET\n",
        );
        let err = plan_profile_rename(&path, "old", "new").unwrap_err();
        assert!(matches!(err, RenameError::ContainsSecrets), "{err:?}");
        assert!(!err.to_string().contains("SECRET"));
    }

    #[test]
    fn refuses_if_file_changed_after_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), CONFIG);
        let plan = plan_profile_rename(&path, "AdministratorAccess-111122223333", "x").unwrap();
        std::fs::write(&path, format!("{CONFIG}\n# edited elsewhere\n")).unwrap();
        assert!(matches!(
            plan.apply(),
            Err(RenameError::ChangedSincePreview)
        ));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("# edited elsewhere\n")
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no backup or temp left"
        );
    }

    #[cfg(unix)]
    #[test]
    fn keeps_permissions_and_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-config");
        std::fs::write(&real, CONFIG).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("config");
        symlink(&real, &link).unwrap();

        let backup = plan_profile_rename(&link, "AdministratorAccess-111122223333", "x")
            .unwrap()
            .apply()
            .unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("[profile x]")
        );
        for path in [&real, &backup] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{}", path.display());
        }
    }
}
