# Design

## Context

ADR-0030 decides cargo-packager on native GitHub runners, a Ubuntu 22.04 container for Linux, a universal macOS
build, unsigned packages and the libheif fallback (ADR-0017 `heic` feature). x86-64 builds use the v2 baseline
(ADR-0032). Much of `xtask/src/package.rs`, `.github/workflows/release.yml` and `packaging/` was scaffolded by
`bootstrap-workspace` from a template; this change completes them for PhotoSquirrel and adds the README.

## Goals / Non-Goals

**Goals:**
- One tag -> one release with six installers/archives and checksums, reproducibly.

**Non-Goals:**
- Signing/notarization, auto-update, store submissions.

## Decisions

- **Packaging (`xtask/src/package.rs`).** As scaffolded: `Kind` per OS, `artifact_name`, `checksum_line`,
  `check_tag`, `check_smoke`, `--docker`, `--universal`, `--tag`. Packager metadata lives in
  `crates/photosquirrel/Cargo.toml` (`binaries` with `photosquirrel` main and `photosquirrel-cli`, icons from
  `assets/`, `deb.depends`, desktop file `packaging/linux/photosquirrel.desktop`).
- **libheif per platform.** Homebrew on macOS, vcpkg on Windows, apt in the Docker image; `package.rs` probes
  `pkg-config libheif` (or the vcpkg path) and passes `--no-default-features --features <rest>` when absent,
  writing `target/packages/heic-<os>.txt`; the release job folds these into the notes.
- **Template clean-up.** References to the template project in `release.yml`, `packaging/linux/Dockerfile` and
  `packaging/release-notes.md` (wrong ADR numbers, `GREMLINER_SECRET_STORE`, "A Gremlin database to connect
  to") are replaced with PhotoSquirrel content: optional LM Studio, optional ffmpeg, keyring note.
- **Workflow tests.** `xtask` unit tests parse `release.yml` (with `serde_yaml`-free line checks to avoid a new
  dependency) and the Dockerfile `FROM` line.
- **README.** Sections: what it does, safety guarantees (ADR-0004, ADR-0029), features, install (table matching
  release notes), first launch (unsigned), build from source (`cargo xtask doctor`), architecture links
  (ADRs). Screenshots copied from `cargo xtask screenshot --all-surfaces` (dark theme, synthetic corpus, never
  real photos) into `docs/screenshots/`.

## Risks / Trade-offs

- [vcpkg libheif build time on Windows runners] -> cache vcpkg; fallback to HEIC off.
- [Unsigned macOS apps blocked by Gatekeeper] -> documented first-launch steps in README and notes.

## Verification

- `cargo test -p xtask package` (all `scenario_release_packaging_*`).
- `cargo xtask package --docker` on Linux; `cargo xtask package --universal` on macOS; `cargo xtask package` on Windows.
- Push a pre-release tag `v0.1.0-rc.1` to a fork and check the GitHub Release contents.
- `cargo xtask ci`, `openspec validate release-packaging --strict`.
