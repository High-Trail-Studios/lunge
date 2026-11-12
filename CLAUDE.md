# Lunge — project notes for AI assistants

Lunge is a desktop GUI over the AWS CLI for AWS SSO sessions: list sessions,
show live/expired state, re-login in one click, open the console for a role.
It also lists access-key ("single account") profiles, read-only for now.
(Briefly called "Luge" during planning; the name is Lunge everywhere now.)

## Invariant: no credential custody

Lunge never stores, encrypts, or persists credentials or tokens. The AWS CLI
owns all token persistence. Lunge only reads `~/.aws/config`,
`~/.aws/sso/cache/`, and profile names in `~/.aws/credentials`, and shells
out to the `aws` CLI to act. It never writes the cache or credentials file.

**Prefer the CLI (owner rule).** Lunge should use the AWS CLI for anything
the CLI can do, and touch AWS files directly only when the CLI has no way.
The one direct write today is renaming an SSO profile (`lunge_core::rename`;
the CLI has no rename): it changes only the `[profile NAME]` header line,
previews first, backs up, writes atomically, re-parses to verify, and
refuses files containing access keys. Don't add other direct writes without
the owner's say-so.

The one exception is console federation: short-lived role credentials are held
in memory only for the federation HTTP call and are never written to disk,
logged, or cached.

Comment this invariant at every point the code touches credential state. Any
change that would weaken it needs an explicit decision from the owner.

### Decided details

- **Reading the SSO cache:** cache files contain `accessToken`,
  `refreshToken`, and `clientSecret`. Deserialize only the non-secret fields
  needed for state (e.g. `startUrl`, `region`, `expiresAt`); never declare the
  secret fields in Lunge's types, so they are never loaded into Lunge memory.
- **Session state is not just `expiresAt`:** with `sso-session` configs the
  access token lasts about an hour and the CLI silently refreshes it using the
  refresh token. An expired access token with a refresh token present is not
  the same as a dead session; model that distinction rather than reporting it
  as "expired".
- **Access-key profiles:** credentials stay in the CLI's own files
  (`~/.aws/credentials`, or `~/.aws/config`); Lunge never keeps its own copy
  (owner decision). The INI parser replaces values of `aws_access_key_id`,
  `aws_secret_access_key`, `aws_session_token`, and `aws_security_token` with
  a marker as it reads them, and parse errors never quote file content.
  Future sign-in for these profiles must also go through the CLI.
- **Role credentials for federation:** use
  `aws configure export-credentials --profile <name>` (output read from
  stdout, held in memory). Never use `aws sso get-role-credentials
  --access-token ...`: it puts the token on the command line, visible to other
  users via `ps`.

## Structure

- `crates/lunge-core`: library. Config parsing, session state, SSO login,
  console federation. Must never depend on egui or any GUI crate. Unit-test
  logic here.
- `crates/lunge-gui`: the `lunge` binary. A thin egui front end; no business
  logic.
- Keep the provider boundary clean (AWS today). The sidebar is a provider
  accordion with only AWS in it; don't add Azure/GCP placeholders or a
  provider abstraction until a second provider is actually being built.
- No AWS logo (trademark); the AWS header uses text plus a generic orange.
- Brand: the master logo is `assets/brand/logo.png` (drawn on navy
  `#141B35`). The toolbar uses a derived copy,
  `crates/lunge-gui/assets/logo-toolbar.png` (cropped, tagline removed, 128px
  tall); regenerate it from the master rather than editing it. The header bar
  is brand navy in both themes so the logo sits on its own background.
- Transparent logos: `assets/brand/logo_transparent_black.png` and
  `_white.png` ("Lun" in black or white) are kept as supplied. They carry a
  baked-in ~1px dark outline that shows on light backgrounds, so the README
  uses edge-cleaned, 1200px-wide copies in `assets/brand/readme/`
  (`logo-on-light.png`, `logo-on-dark.png`), switched by a `<picture>`
  element on the reader's light/dark preference.

## Scope (v1)

AWS SSO sessions, plus a read-only list of access-key profiles (sign-in for
those is deferred). No SAML or role-chaining session types. No config
editing beyond the profile rename (users add sessions with
`aws configure sso`), credential routing, or general-purpose engine.

## Not a fork

Original code, name, and visual identity. Mimic Leapp's workflow, never its
pixels, names, logos, icons, strings, or layout.

## Conventions

- Targets: macOS and Linux. Windows is out of scope.
- Preferences: eframe `persistence` saves only the `Prefs` struct (zoom,
  theme) and window geometry; `persist_egui_memory` is off so no UI state
  derived from AWS files reaches disk. Never put AWS data in `Prefs`.
- Renderer: eframe with `glow`, not the default `wgpu` (smaller binary and
  build). Revisit only with a concrete reason.
- Toolchain pinned in `rust-toolchain.toml`; dependencies locked in
  `Cargo.lock`; CI actions pinned by commit SHA.
- Before committing: `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
- Work is tracked in Linear (project P-BRI-10); M0–M4 milestones, tickets
  BRI-160 to BRI-181.
