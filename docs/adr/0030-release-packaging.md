# 0030. Installers built by GitHub Actions with cargo-packager

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The project is published on GitHub and should install like any desktop app on macOS, Windows and Linux.

## Decision
A tag `v<version>` triggers `.github/workflows/release.yml`, which builds on native runners (macOS universal arm64+x86_64 DMG, Windows NSIS installer, Linux .deb + AppImage built in an Ubuntu 22.04 container for glibc 2.35+) using `cargo-packager`, and publishes a GitHub Release. Builds use the `dist` profile (thin LTO, stripped). libheif is installed per platform (Homebrew, vcpkg, apt) for the `heic` feature; if unavailable on a platform the build falls back to the feature off and the release notes say so. Packages are unsigned initially (documented in the README).

## Alternatives considered
- **cargo-dist** - less control over desktop bundle metadata.
- **Cross-compiling from Linux** - GPUI's platform backends need native SDKs.

## Consequences
- CI also builds and tests on all three OSes for every push.

## Revisit when
Code signing / notarization certificates become available.
