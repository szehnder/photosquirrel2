# 0059. Release packages build HEIC decoding only where libheif is found and new enough

- Status: accepted
- Date: 2026-10-21
- Deciders: maintainers
- Amends: [0017](0017-format-support.md) ("default in release builds"), [0030](0030-release-packaging.md)

## Context
ADR-0017 puts HEIC/HEIF decoding behind the `heic` feature and libheif; ADR-0030 installs libheif per platform and
falls back to the feature off where it is unavailable. In practice, for the first release:
- the Linux packaging image is Ubuntu 22.04 (glibc floor), whose libheif is 1.12; the `heic` feature needs 1.17
  (libheif-rs `v1_17`);
- the macOS build is universal, and Homebrew's libheif is a single-architecture dylib that the app bundle would not
  carry, so the app would fail to start on machines without Homebrew's libheif;
- a static vcpkg libheif on Windows adds a long codec build to every release and links LGPL code statically.

## Decision
`cargo xtask package` probes for libheif the way libheif-sys does (pkg-config on Linux and macOS, a static vcpkg
library under `VCPKG_ROOT` on Windows) and builds both binaries with `--features photosquirrel/heic,photosquirrel-cli/heic`
only when it finds libheif 1.17 or newer and the build is not the universal macOS build. Otherwise it builds without
HEIC and writes `target/packages/heic-<os>.txt`; the release job appends these lines to the release notes. The
release workflow installs no libheif, so the first releases ship without HEIC decoding on every platform: HEIC
files are still found, catalogued, exactly de-duplicated and exported, show "preview unavailable", and are not
matched by look. Packages built on a host with a suitable libheif (local use) link it dynamically.

## Alternatives considered
- **Bundle libheif and its codecs in each package** - right long term; needs per-format bundling (AppImage,
  .deb dependency, dylib relocation in the app bundle, DLLs or static linking on Windows) and an LGPL review.
- **libheif-sys `embedded-libheif`** - builds libheif with CMake but still needs the HEVC decoder (libde265)
  available per platform and architecture.

## Consequences
- The README and release notes say HEIC previews are not in the prebuilt packages yet; building from source with
  `--features heic` enables them.

## Revisit when
Bundling libheif in the packages is implemented, or a pure-Rust HEVC decoder is viable (ADR-0017).
