<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/readme/logo-on-dark.png">
    <img alt="Lunge by High Trail Studios" src="assets/brand/readme/logo-on-light.png" width="560">
  </picture>
</h1>

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

**Add SSO session** asks what `aws configure sso` asks before you sign in:
a session name, your start URL, the SSO region, and the registration scopes
(pre-filled with `sso:account:access`). **Save and sign in** runs
`aws configure set KEY VALUE --sso-session NAME` for each setting, so the
AWS CLI writes the `[sso-session]` block, not Lunge. Lunge then reads your config back to check
the block holds exactly what you entered and runs `aws sso login`. Once
you're signed in, your accounts appear and **Name** asks the rest of the
CLI's questions (profile name, default region, and output format).
Prefer the terminal? **Show CLI** opens a drawer with the `aws configure sso`
command to copy, and shows exactly what Save will type into the CLI.

**Every account you can use.** While a session is Live, Lunge asks AWS which
accounts and roles it gives you and lists them all in a table under
**Accounts**: account ID, account name, role, profile, region, and the shell
text to use the profile. Click a column header to sort by it (again to
reverse). It starts sorted by profile, which puts accounts without one at
the bottom. The account name is blank until the session is logged in and
listed, or if AWS has none for it.

- Account roles without a profile show "none yet" and a **Name** button,
  which suggests a name (`<session>-<account>-<role>`, e.g.
  `acme-dev-admin`) and creates the profile with `aws configure set` once
  you confirm.
- **Region** sets the profile's default region straight away with
  `aws configure set region <region> --profile <name>`.
- **Copy** puts `AWS_PROFILE=<name> ` on the clipboard, to paste before an
  `aws` command. On Windows it copies `$env:AWS_PROFILE='<name>'; ` for
  PowerShell instead, which applies to the rest of that PowerShell window
  rather than one command.

A filter appears for long lists. Profiles whose account role AWS no longer
lists are marked "not listed".

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
  never writes the token cache or the credentials file, and writes the config
  file only when you rename a profile (see below).
- **Lunge never reads secret values**, with one exception below. Access keys,
  session tokens, refresh tokens, and client secrets are skipped as files are
  parsed; only their presence is noted.
- **Exception: listing your accounts.** To ask AWS which accounts and roles a
  Live session can use, Lunge reads that session's SSO access token from the
  CLI cache and sends it only to the AWS SSO portal
  (`portal.sso.<region>.amazonaws.com`), in memory, for those requests. It is
  never logged, saved, or put on a command line. (The AWS CLI's own
  `aws sso list-accounts` takes the token only on its command line, where
  other users can see it, or from a file on disk.)
- **Lunge never stores, encrypts, or persists credentials or tokens.** There
  is no Lunge vault, keychain entry, or config file holding secrets.
- **One narrow exception: opening the console.** To build a console sign-in
  URL, Lunge obtains short-lived role credentials and passes them to the AWS
  federation endpoint. Those credentials live in memory only for that one HTTP
  call. They are never written to disk, logged, or cached.

If you uninstall Lunge, your AWS CLI setup is exactly as it was, apart from
any profiles you chose to rename.

**Renaming a profile** (the **Rename** button on an account) is the one
change Lunge makes to AWS files, because the AWS CLI has no rename command.
It changes only that profile's `[profile NAME]` line in `~/.aws/config`:
you see the exact before and after first, a timestamped backup
(`config.lunge-backup-<time>`) is saved next to the file, and the new file is
swapped in only once fully written. Lunge refuses the rename if the name is
taken, if another profile refers to this one by `source_profile`, if the
file changed since the preview, or if your config file contains access keys
(so it never copies secrets). Scripts or `AWS_PROFILE` settings using the old
name need updating by hand.

**Lunge's own file** is its display preferences (zoom, theme, whether
the CLI drawer is open, and window size; nothing from your AWS files): `~/.local/share/lunge/app.ron` on
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
role chaining aren't supported. Apart from renaming a profile, Lunge does not
edit your AWS config or credentials itself: new sessions and profiles are
written by the AWS CLI, which Lunge runs for you. It doesn't route
credentials to other tools or replace the AWS CLI; it requires it.

## Limitations

- **Legacy SSO profiles aren't shown.** Profiles with `sso_start_url`
  directly in the profile (no `sso_session`) appear under "config issues".
  Run `aws configure sso` to move them to an `[sso-session]` block.
- **Single accounts are listed only.** Access-key profiles (from
  `~/.aws/credentials`, or keys placed in `~/.aws/config`) appear under Single
  accounts; there's no sign-in or console action for them yet.
- **Adding a session works best with AWS CLI 2.34.22 or newer.** That's
  the first release where `aws configure set` can write `[sso-session]`
  blocks. With an older CLI on macOS or Linux, Lunge instead types your
  answers into `aws configure sso-session`'s prompts; if that CLI asks
  different questions, it either stops without writing anything or Lunge's
  check after saving catches the mismatch. On Windows the older CLI's
  prompts only read from a console window, so Lunge asks you to update the
  CLI instead. The settings are written one at a time; if the CLI fails
  partway, Lunge tells you which ones landed rather than carrying on as if
  it had worked. The terminal route (**Show CLI**) always works.
- **Adding a session doesn't edit or replace one.** Names already in your
  config are refused; change an existing session with `aws configure
  sso-session` in a terminal.
- **The region list is built in.** Asking AWS for its regions would need
  role credentials just to fill a dropdown, so Lunge ships a fixed list and
  needs an update to offer regions AWS adds later. A region already set on a
  profile still shows, listed or not. Regions can't be cleared from Lunge
  (the CLI can't unset a setting); remove the `region` line by hand.
- **One login at a time.** Each login opens a browser tab to approve, so
  other Log in buttons are disabled while one runs.
- **Config parsing follows the AWS CLI exactly**, including its quirks: an
  indented line continues the previous value, `#` mid-line is part of the
  value, and duplicate sections make the whole file invalid (as they do for
  the CLI).
- **`AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` are honored, with `~`
  expanded**, but `$VAR`s inside them are not.

## Platforms

macOS, Linux, and Windows 10/11 (x86_64). Windows on ARM isn't built yet.

The Windows build draws with Direct3D 12 (wgpu) rather than OpenGL so that,
where there's no GPU driver (VMs, remote desktop, Amazon WorkSpaces), it can
fall back to Windows' built-in software renderer. macOS and Linux use OpenGL.

## Requirements

- [AWS CLI v2](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)
  on your `PATH`. Lunge can add your first `sso-session`, or use
  `aws configure sso`.

Lunge creates no AWS resources and makes no billable AWS API calls.

## Install

Download the latest build from the repository's **Releases** page:

| Platform | File | Install |
|---|---|---|
| Ubuntu 22.04+, Debian 12+, and derivatives (Mint, Pop!_OS, Zorin, elementary) | `lunge_<version>-1_amd64.deb` | `sudo apt install ./lunge_<version>-1_amd64.deb` |
| Fedora, RHEL 10 | `lunge-<version>-1.x86_64.rpm` | `sudo dnf install ./lunge-<version>-1.x86_64.rpm` |
| openSUSE Leap 16, Tumbleweed | `lunge-<version>-1.x86_64.rpm` | `sudo zypper install --allow-unsigned-rpm ./lunge-<version>-1.x86_64.rpm` |
| macOS (Apple silicon and Intel) | `lunge-<version>-macos-universal.tar.gz` | Unpack and follow `README-macos.txt` inside |
| Windows 10/11 (x86_64) | `lunge-<version>-windows-x86_64.zip` | Unzip and follow `README-windows.txt` inside (portable, nothing installed) |

Each release includes `SHA256SUMS` for checking downloads
(`sha256sum -c SHA256SUMS`, or `shasum -a 256 -c SHA256SUMS` on macOS). The
macOS build is not yet signed or notarized, so Gatekeeper blocks it until you
clear its quarantine flag (see `README-macos.txt`); a signed `.dmg` will
replace it. The Windows build is not code-signed either: SmartScreen warns
before the first run, and Windows 11 with Smart App Control turned on won't
run it at all. Releases are x86_64 only on Linux and Windows for now, and
0.x versions are marked as pre-releases.

The Linux packages need glibc 2.35 or newer. CI installs and launches every
build on Ubuntu 22.04 and 24.04, Debian 13, Fedora, and openSUSE Leap 16.
Mint, Pop!_OS, Zorin, and elementary aren't tested directly; they're expected
to work through their Ubuntu base. RHEL 10 is expected to work but isn't
tested. RHEL/Rocky 9 and older (glibc 2.34) aren't supported. The packages
aren't GPG-signed yet; signed apt and dnf repositories are planned.

Between releases, CI attaches `.deb`/`.rpm` packages to every push to `main`
(workflow run artifacts), and the **Build macOS** and **Build Windows**
workflows can be run on demand.

## Build from source

You need a C toolchain (Linux: `build-essential` or equivalent; macOS: Xcode
Command Line Tools; Windows: Visual Studio Build Tools with "Desktop
development with C++") and [rustup](https://rustup.rs). The Rust version is
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
