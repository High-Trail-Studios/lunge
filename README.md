# Lunge

A small desktop app for AWS SSO sessions. See every SSO profile, know at a
glance which sessions are live or expired, re-login in one click, and open the
AWS console for a role in one click.

> **Status: pre-alpha.** Listing sessions with their state, one-click login,
> and light/dark theming work. Opening the AWS console (M2) is not built
> yet. Expect rough edges; see [Limitations](#limitations).

## What you see

The sidebar lists your AWS **SSO sessions** (each `[sso-session]` in your AWS
config, with its state) and **single accounts** (profiles with access keys).
Select an SSO session to see its state, how long until it expires, and the
accounts and roles that use it. The window re-reads everything every 30
seconds, and **Refresh** does it on demand.

**Add SSO session** shows the `aws configure sso` command to run: the AWS CLI
creates sessions, and Lunge picks them up.

The status bar shows when Lunge last re-read your files and has zoom presets
(100% to 200%; Ctrl + / Ctrl - / Ctrl 0 work too), which help on high-DPI
screens running at 1x scaling.

| State | Meaning |
|---|---|
| **Live** | The AWS CLI has a valid token for this session. |
| **Renewable** | The token expired, but the CLI holds a refresh token, so its next use of the session will probably renew it without a browser. Lunge can't confirm this ahead of time: how long the refresh token lasts is set by your identity provider and isn't recorded locally. |
| **Expired** | The token expired and can't be renewed; log in again. |
| **Not logged in** | No token cached for this session. |
| **Unreadable** | A cache file exists but couldn't be parsed. |

**Log in** runs `aws sso login --sso-session <name>`. The CLI opens your
browser; if it doesn't, Lunge shows the URL to open. Lunge confirms
success by re-reading the CLI's token cache, not just by the exit code.

## No credential custody

Lunge is a visual front end over the AWS CLI. It is designed so that it never
holds your credentials:

- **The AWS CLI owns all token persistence.** Lunge only *reads* the CLI's own
  state (`~/.aws/config`, `~/.aws/sso/cache/`, and profile names from
  `~/.aws/credentials`) and shells out to the CLI (`aws sso login`) to act. It
  never writes to any of them.
- **Lunge never reads secret values.** Access keys, session tokens, and SSO
  tokens are skipped as files are parsed; only their presence is noted (that
  a profile has keys, that a session has a token and when it expires).
- **Lunge never stores, encrypts, or persists credentials or tokens.** There
  is no Lunge vault, keychain entry, or config file holding secrets.
- **One narrow exception: opening the console.** To build a console sign-in
  URL, Lunge obtains short-lived role credentials and passes them to the AWS
  federation endpoint. Those credentials live in memory only for that one HTTP
  call. They are never written to disk, logged, or cached.

If you uninstall Lunge, your AWS CLI setup is exactly as it was.

**The one file Lunge writes** is its display preferences (zoom, theme, and
window size; nothing from your AWS files): `~/.local/share/lunge/app.ron` on
Linux, `~/Library/Application Support/lunge/app.ron` on macOS. Uninstalling
the package doesn't remove per-user files, so delete that folder to remove
every trace of Lunge.

## What Lunge is (and isn't)

Lunge is an original, from-scratch tool. It is a spiritual successor to the
discontinued Leapp in *workflow* (the familiar session-list interaction) but
shares no code, name, or visual identity with it, and is not affiliated with
it.

**v1 is built around AWS SSO (IAM Identity Center).** Access-key profiles are
listed, but Lunge can't sign in or open the console with them yet. SAML and
role chaining aren't supported. Lunge does not edit your AWS config or
credentials, route credentials to other tools, or replace the AWS CLI; it
requires it.

## Limitations

- **Legacy SSO profiles aren't shown.** Profiles with `sso_start_url`
  directly in the profile (no `sso_session`) appear under "config issues".
  Run `aws configure sso` to move them to an `[sso-session]` block.
- **Single accounts are listed only.** Access-key profiles (from
  `~/.aws/credentials`, or keys placed in `~/.aws/config`) appear under Single
  accounts; there's no sign-in or console action for them yet.
- **One login at a time.** Each login opens a browser tab to approve, so
  other Log in buttons are disabled while one runs.
- **Config parsing follows the AWS CLI exactly**, including its quirks: an
  indented line continues the previous value, `#` mid-line is part of the
  value, and duplicate sections make the whole file invalid (as they do for
  the CLI).
- **`AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` are honored, with `~`
  expanded**, but `$VAR`s inside them are not.

## Platforms

macOS and Linux. Windows is not supported.

## Requirements

- [AWS CLI v2](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)
  on your `PATH`, with at least one `sso-session` configured
  (`aws configure sso`).

Lunge creates no AWS resources and makes no billable AWS API calls.

## Install

Packages are not published yet. `.deb` and `.rpm` packages are built by CI on
every push to `main` (see the workflow run's artifacts). A signed macOS `.dmg`
will follow.

## Build from source

You need a C toolchain (Linux: `build-essential` or equivalent; macOS: Xcode
Command Line Tools) and [rustup](https://rustup.rs). The Rust version is
pinned in `rust-toolchain.toml` and installed automatically.

```sh
cargo run -p lunge-gui
```

Linux packages (requires `cargo install cargo-deb cargo-generate-rpm`):

```sh
cargo deb -p lunge-gui                                       # -> target/debian/*.deb
cargo build --release -p lunge-gui && cargo generate-rpm -p crates/lunge-gui   # -> target/generate-rpm/*.rpm
```

## Project layout

| Crate        | Role                                                                 |
|--------------|----------------------------------------------------------------------|
| `lunge-core` | Library: config parsing, session state, SSO login, console federation. No GUI dependencies. |
| `lunge-gui`  | The `lunge` binary: a thin [egui](https://github.com/emilk/egui) front end over `lunge-core`. |

## License

[Apache-2.0](LICENSE). Copyright High Trail Studios.
