# Design

## Context

ADR-0030 decides cargo-packager on native GitHub runners, a Ubuntu 22.04 container for Linux, a universal macOS
build, unsigned packages and the libheif fallback (ADR-0017 `heic` feature). x86-64 builds use the v2 baseline
(ADR-0032). Much of `xtask/src/package.rs`, `.github/workflows/release.yml` and `packaging/` was scaffolded by
`bootstrap-workspace` from a template; this change completes them for PhotoSquirrel and adds the README.

## Goals / Non-Goals

**Goals:**
- One tag -> one release with seven installers/archives (AppImage, .deb, Linux .tar.gz, DMG, macOS .tar.gz,
  NSIS setup, .zip) and their checksums, reproducibly.

**Non-Goals:**
- Signing/notarization, auto-update, store submissions.

## Decisions

- **Packaging (`xtask/src/package.rs`).** As scaffolded: `Kind` per OS, `artifact_name`, `checksum_line`,
  `check_tag`, `check_smoke`, `--docker`, `--universal`, `--tag`. Packager metadata lives in
  `crates/photosquirrel/Cargo.toml` (`binaries` with `photosquirrel` main and `photosquirrel-cli`, icons from
  `assets/`, `deb.depends`, desktop file `packaging/linux/photosquirrel.desktop`).
- **libheif per platform (ADR-0059).** `package.rs` probes for libheif the way libheif-sys does (`pkg-config
  libheif` on Linux and macOS, a static vcpkg library under `VCPKG_ROOT` on Windows) and builds with
  `--features photosquirrel/heic,photosquirrel-cli/heic` only for libheif 1.17+ outside the universal macOS build;
  otherwise it builds without HEIC and writes `target/packages/heic-<os>.txt`, which the release job folds into
  the notes. The release images install no libheif, so v0.1 packages are HEIC-off on every platform.
- **What the packages redistribute (ADR-0058, ADR-0060).** `THIRD-PARTY-LICENSES` from cargo-about, opening with
  the LGPL source offer for rawler (the release attaches `rawler-<version>.crate`, checked against `Cargo.lock`);
  the AppImage excludes the system libraries linuxdeploy would bundle; Windows links the CRT statically and
  packaging rejects an `.exe` importing `VCRUNTIME*`.
- **Workflow hardening.** Read-only `GITHUB_TOKEN` except in the release job, `persist-credentials: false` on
  every checkout, actions pinned to commit SHAs, and publishing only for a pushed `v` tag (a manual run is a dry
  run from any ref). The packaging image is pinned to an `ubuntu:22.04` digest.
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
