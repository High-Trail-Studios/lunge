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

Two in-memory exceptions, both owner-approved:

- **Console federation:** short-lived role credentials are held in memory only
  for the federation HTTP call and are never written to disk, logged, or
  cached.
- **Listing accounts and roles** (`lunge_core::accounts`): the AWS CLI's
  `aws sso list-accounts`/`list-account-roles` accept the access token only on
  the command line (visible via `ps`) or from a file on disk, not stdin, so
  Lunge calls the same SSO portal endpoints itself. The token is read only
  from a Live session (`cache::read_access_token`, crate-private), kept in a
  zeroing, redacted `Secret`, and sent only in the request header to
  `portal.sso.<region>.amazonaws.com`. Nothing else may call
  `read_access_token`. The fetched account list stays in memory too.
- Creating a profile for a listed account goes through the CLI
  (`aws configure set`, `lunge_core::profiles`), not a direct file write.
  So does changing a profile's default region from the account table
  (`AwsCli::set_profile_region`, owner decision 2026-11-21). It checks the
  profile exists first, because `aws configure set --profile X` silently
  creates X if it doesn't.
- Adding an SSO session goes through the CLI too (`lunge_core::sessions`,
  owner decision): `aws configure set KEY VALUE --sso-session NAME` (AWS
  CLI 2.34.22+) for `sso_start_url`, `sso_region`,
  `sso_registration_scopes`, in that order. If the CLI rejects
  `--sso-session` as unknown, macOS/Linux fall back to piping the four
  answers into `aws configure sso-session`'s prompts; Windows can't (the
  CLI's prompt_toolkit reads the console, not stdin, and fails with "No
  Windows console found"), so it asks the user to update the CLI. Taken
  names and answers containing line breaks are refused first, a failure
  after the first key reports which keys landed, and the block is re-read
  afterwards to check the answers landed in the right keys. Don't replace
  this with a direct write without the owner's say-so.

Comment this invariant at every point the code touches credential state. Any
change that would weaken it needs an explicit decision from the owner.

### Decided details

- **Reading the SSO cache:** cache files contain `accessToken`,
  `refreshToken`, and `clientSecret`. For session state, deserialize only the
  non-secret fields (e.g. `startUrl`, `region`, `expiresAt`); the secret
  fields are `IgnoredAny`. The only exception is `read_access_token` for
  account listing (above); `refreshToken` and `clientSecret` are never
  loaded.
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
- App icon: `assets/brand/make_icon.py` (Pillow) builds it from
  `assets/brand/logo_transparent_notext.png`: the logo on a brand-navy
  rounded square on Apple's icon grid. It writes `assets/brand/icon-1024.png`
  (master, for a future `.app`) and `crates/lunge-gui/assets/icon.png` (512px,
  built into the binary as the Dock/window icon) and `icon-256.png` (Linux
  hicolor). Rerun the script rather than editing the outputs.
- Transparent logos: `assets/brand/logo_transparent_black.png` and
  `_white.png` ("Lun" in black or white) are kept as supplied. They carry a
  baked-in ~1px dark outline that shows on light backgrounds, so the README
  uses edge-cleaned, 1200px-wide copies in `assets/brand/readme/`
  (`logo-on-light.png`, `logo-on-dark.png`), switched by a `<picture>`
  element on the reader's light/dark preference.

## Scope (v1)

AWS SSO sessions, plus a read-only list of access-key profiles (sign-in for
those is deferred). No SAML or role-chaining session types. No config
editing beyond the profile rename, CLI-driven creation of sessions and
profiles, and setting a profile's region through the CLI (no editing or
deleting sessions), credential routing, or general-purpose engine.

## Not a fork

Original code, name, and visual identity. Mimic Leapp's workflow, never its
pixels, names, logos, icons, strings, or layout.

## Conventions

- Targets: macOS, Linux, and Windows x86_64 (owner decision 2026-11-21).
  Spawn the CLI only through `AwsCli::command()`, which hides the console
  window on Windows. Windows ships as a portable, unsigned zip for now
  (owner decision); code signing and Windows ARM64 are deferred.
- Preferences: eframe `persistence` saves only the `Prefs` struct (zoom,
  theme, CLI drawer open) and window geometry; `persist_egui_memory` is off so no UI state
  derived from AWS files reaches disk. Never put AWS data in `Prefs`.
- Renderer: eframe with `glow`, not the default `wgpu` (smaller binary and
  build), except on Windows, which uses `wgpu` (owner decision 2026-11-21):
  glow needs a real OpenGL driver, which Windows VMs, RDP/VDI sessions and
  CI runners often lack, while wgpu's DX12 backend falls back to WARP.
  Revisit only with a concrete reason.
- Toolchain pinned in `rust-toolchain.toml`; dependencies locked in
  `Cargo.lock`; CI actions pinned by commit SHA.
- Linux glibc floor is 2.35 (owner decision 2026-11-22). CI's
  `package-linux` job builds the `.deb`/`.rpm` inside a digest-pinned
  `ubuntu:22.04` container (not the `ubuntu-22.04` runner label, which
  GitHub will retire) and fails if the binary needs a newer glibc. That
  covers Ubuntu 22.04+ and derivatives (Mint 21+, Pop!_OS 22.04+, Zorin
  17+, elementary 7+), Debian 12+/LMDE 6+, Fedora, openSUSE Leap 16 and
  Tumbleweed, and RHEL 10; RHEL/Rocky 9 (2.34) and SLE 15 are out.
  `smoke-test-linux` installs and launches each build on Ubuntu 22.04 and
  24.04, Debian 13, Fedora, and openSUSE Leap 16 via
  `packaging/linux/smoke-test.sh` (runnable locally with Docker). The
  README's distro list must match what that job tests; derivatives and
  RHEL are described as expected to work, not tested. Raising the floor
  drops distros, so it needs the owner's say-so.
- Before committing: `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
- Releasing: bump `version` under `[workspace.package]` in `Cargo.toml`,
  run `cargo check` so `Cargo.lock` follows (CI uses `--locked`), commit,
  and push to `main`. When CI passes for that commit, the Release workflow
  builds the macOS binary and the Windows zip, takes CI's tested
  `.deb`/`.rpm`, and publishes tag `vX.Y.Z` with all four plus
  `SHA256SUMS`. Pushes without a version bump don't release (macOS minutes
  are expensive on a private repo).
- Work is tracked in Linear (project P-BRI-10); M0–M4 milestones, tickets
  from BRI-160 on.

## Linux package repositories (BRI-191): planned, not built yet

Goal: `apt install lunge` / `dnf install lunge` from our own signed repos,
so Linux users get updates with the rest of their system. Nothing below
exists in code yet; keep this section matching what's actually built.

Settled direction:

- **Hosting (owner decision 2026-11-22):** `packages.hightrailstudios.com`,
  an org-wide package host with its own S3 bucket and its own CloudFront
  distribution, separate from the `hightrailstudios.com` site (whose
  `/lunge` page is Lunge's marketing/docs and links here). Private bucket,
  origin access control, no S3 website endpoint, no CloudFront function.
  Nothing bills by the hour. Keeping it apart means a site deploy
  (`s3 sync --delete`) can never touch the repos, and the publish role can
  be scoped to just this bucket and distribution.
- **Layout:** each tool owns a prefix: `/lunge/deb/` (apt, suite
  `stable`), `/lunge/rpm/` (rpm-md), and the public key under `/keys/`.
  Lunge's publish job only ever writes under `/lunge/`, so a future tool's
  repo can't be clobbered by Lunge's stateless rebuild.
- **The repo is a build artifact.** A stateless script regenerates the
  whole apt and rpm trees from the `.deb`/`.rpm` assets on GitHub Releases
  (`apt-ftparchive` + `createrepo_c`, no aptly/reprepro database). If the
  bucket is lost, rerun the workflow. No bucket versioning needed for the
  same reason.
- **Infra lives outside this repo:** the bucket, distribution, certificate
  and DNS go in `terraform-environments` (the `hightrail-prod` account;
  the Route 53 zone is in `common`). Chronkeeper's
  `infrastructure/modules/static_site` already builds exactly this shape
  (private bucket + OAC + ACM + CloudFront + alias records); the package
  host would be its second user, so promote it to a shared module rather
  than copy it.
- **Publishing from CI:** GitHub OIDC (provider in
  `terraform-environments/modules/core/global`) to an IAM role that trusts
  only this repo's `main` branch / release environment, allowed to list
  the bucket, put/get/delete under `lunge/*`, and
  `cloudfront:CreateInvalidation` on that one distribution. No long-lived
  AWS keys in GitHub.
- **Upload order:** packages (`pool/`, `Packages/`) first, signed metadata
  (`InRelease`, `Release.gpg`, `repomd.xml(.asc)`) last, then invalidate
  only the metadata paths. Clients must never see metadata that points at a
  file not uploaded yet. Package files are immutable (long cache); metadata
  gets a short TTL. Don't delete old package versions in the same publish.
- **Signing:** one OpenPGP key for both repos. Primary key stays offline
  with the owner; CI holds only a signing subkey with an expiry, in a
  GitHub environment secret. apt verifies signed `InRelease`; for RPM, both
  the packages (`rpmsign`, so `gpgcheck=1`) and `repomd.xml`
  (`repo_gpgcheck=1`). Sign the `.rpm` before the GitHub Release is
  created so the release asset, `SHA256SUMS`, and the repo all carry the
  same file. Use SHA-256 digests (Fedora's rpm rejects SHA-1). The key's
  fingerprint is published on the website and in the README, not only next
  to the key.
- **Key expiry is an outage.** When the subkey expires, every user's
  `apt update`/`dnf` fails until they fetch the new public key. Track the
  expiry date and rotate well ahead of it.
- **apt client config:** a deb822 `.sources` file with `Signed-By:` pointing
  at a keyring in `/usr/share/keyrings/`. Never `apt-key`, never a key in
  `trusted.gpg.d` (that trusts our key for every repo on the machine).
- **Test before publishing:** CI builds the repo into a temp dir, serves it
  locally, and installs from it with signature checks on clean Ubuntu and
  Fedora containers, like the existing package smoke tests.

Open decisions (owner): key algorithm (RSA 4096 for widest rpm
compatibility vs. Ed25519); whether the key is a High Trail org key or
Lunge-only; whether the `.deb`/`.rpm` themselves install the repo config
and keyring (the "download once, then updates arrive" pattern, which also
rotates the key for users automatically).

Flatpak (BRI-189) is not part of this. Self-hosting an OSTree repo on S3
is possible but fiddly; Flathub is the usual route and gives the real
one-click install in GNOME Software/KDE Discover. Either way the sandbox
conflicts with how Lunge works (it runs the host's `aws` CLI and reads
`~/.aws`), which needs host-command and filesystem permissions that
Flathub reviewers question. Work that out before promising Flatpak.
