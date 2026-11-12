# Lunge

A small desktop app for AWS SSO sessions. See every SSO profile, know at a
glance which sessions are live or expired, re-login in one click, and open the
AWS console for a role in one click.

> **Status: pre-alpha.** The workspace, CI, and packaging are in place; the
> features below are being built. Nothing here is usable yet.

## No credential custody

Lunge is a visual front end over the AWS CLI. It is designed so that it never
holds your credentials:

- **The AWS CLI owns all token persistence.** Lunge only *reads* the CLI's own
  state (`~/.aws/config` and `~/.aws/sso/cache/`) and shells out to the CLI
  (`aws sso login`) to act. It never writes to either location.
- **Lunge never stores, encrypts, or persists credentials or tokens.** There
  is no Lunge vault, keychain entry, or config file holding secrets.
- **One narrow exception: opening the console.** To build a console sign-in
  URL, Lunge obtains short-lived role credentials and passes them to the AWS
  federation endpoint. Those credentials live in memory only for that one HTTP
  call. They are never written to disk, logged, or cached.

If you uninstall Lunge, your AWS CLI setup is exactly as it was.

## What Lunge is (and isn't)

Lunge is an original, from-scratch tool. It is a spiritual successor to the
discontinued Leapp in *workflow* (the familiar session-list interaction) but
shares no code, name, or visual identity with it, and is not affiliated with
it.

**v1 scope is AWS SSO (IAM Identity Center) only.** Lunge does not manage
IAM-user access keys, SAML sessions, or role chaining. It does not edit your
AWS config, route credentials to other tools, or replace the AWS CLI; it
requires it.

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
