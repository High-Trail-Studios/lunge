# Lunge — project notes for AI assistants

Lunge is a desktop GUI over the AWS CLI for AWS SSO sessions: list sessions,
show live/expired state, re-login in one click, open the console for a role.
(Briefly called "Luge" during planning; the name is Lunge everywhere now.)

## Invariant: no credential custody

Lunge never stores, encrypts, or persists credentials or tokens. The AWS CLI
owns all token persistence. Lunge only reads `~/.aws/config` and
`~/.aws/sso/cache/` and shells out to the `aws` CLI to act. It never writes to
either.

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
- Keep the provider boundary clean (AWS today), but build only AWS SSO. Don't
  add a provider abstraction until a second provider is actually being built.

## Scope (v1)

AWS SSO only. No IAM-user, SAML, or role-chaining session types. No config
editing, credential routing, or general-purpose engine.

## Not a fork

Original code, name, and visual identity. Mimic Leapp's workflow, never its
pixels, names, logos, icons, strings, or layout.

## Conventions

- Targets: macOS and Linux. Windows is out of scope.
- Renderer: eframe with `glow`, not the default `wgpu` (smaller binary and
  build). Revisit only with a concrete reason.
- Toolchain pinned in `rust-toolchain.toml`; dependencies locked in
  `Cargo.lock`; CI actions pinned by commit SHA.
- Before committing: `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
- Work is tracked in Linear (project P-BRI-10); M0–M4 milestones, tickets
  BRI-160 to BRI-181.
