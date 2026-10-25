# 0089. HEIC through the operating system's decoder; no bundled HEVC decoder

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Supersedes: [0059](0059-release-packages-build-heic-only-with-a-shippable-libheif.md)
- Amends: [0017](0017-format-support.md) (HEIC via libheif behind `heic`), [0030](0030-release-packaging.md)
  (libheif installed per platform)

## Context
HEIC stills are HEVC-coded. ADR-0017 decoded them with libheif behind the `heic` feature; ADR-0059 found that no
release package could include it (Ubuntu 22.04's libheif 1.12 is older than libheif-rs `v1_17`, the universal macOS
build cannot link a single-architecture libheif, a static Windows libheif means a long codec build and static LGPL
code), so no package decodes HEIC and iPhone photos are never matched by look. Bundling any HEVC decoder in a
distributed binary carries patent exposure, also for free software. Every supported OS has, or can get with one
install, an HEVC-capable HEIF decoder: ImageIO on macOS (always, since 10.13), WIC with the HEIF Image Extensions and
an HEVC codec on Windows 10 1809+, and the distribution's libheif with an HEVC plugin on Linux.

## Decision
- PhotoSquirrel ships **no HEVC decoder** in any binary, package or build option. Packaging fails if a shipped
  binary's dependency graph contains an HEVC decoder crate or a Linux program links libheif or libde265.
- HEIC/HEIF stills are decoded by the **operating system's decoder**, through a decoder registry that `ps-media`'s
  decode functions take as a parameter (`Decoders`, trait `StillDecoder`):
  - macOS: ImageIO (`CGImageSourceCreateThumbnailAtIndex`, transform applied), through hand-written declarations
    of the few CoreFoundation, CoreGraphics and ImageIO C functions it uses (no binding crate).
  - Windows: WIC (`windows` crate), per-thread MTA COM, scaled and converted to 24-bit RGB; Media Foundation's
    `MFTEnumEx` (the HEVC check) is loaded at run time, so a Windows N edition without it still starts.
  - Linux and other non-Apple Unix: the system libheif loaded at run time with `libloading` (`libheif.so.1`), a
    hand-written table of C functions, **minimum version 1.12** (Ubuntu 22.04, the packages' glibc floor), an HEVC
    decoder required (`heif_have_decoder_for_format`), `heif_init` called when present (1.13+, loads plugins).
- The platform code lives in a new core crate, **`ps-sysdecode`** (no tokio, no gpui-kit), which implements the
  trait, detects availability by decoding an embedded sample (self-test) and returns, when unavailable, the reason and
  the fix for the detected OS or distribution. `ps-service` builds the registry at startup off the UI thread; the CLI
  per process.
- The `heic` cargo feature and `libheif-rs` are removed; source builds and packages use the same run-time loader.
- `PHOTOSQUIRREL_HEIF_DECODER=none` forces "no decoder" (any build); `=fake` (debug builds only) installs a test
  decoder; `PHOTOSQUIRREL_LIBHEIF=<path>` overrides the library path. `doctor` reports active overrides.
- The system libheif is the user's, not redistributed, so it adds nothing to `THIRD-PARTY-LICENSES` (ADR-0060).

## Alternatives considered
- **Bundle libheif + libde265** - patent exposure for a distributed HEVC decoder; LGPL bundling per platform.
- **Keep libheif-rs for source builds next to the loader** - two code paths and test matrices, and a binary that
  fails to start without libheif, for no user gain.
- **Require libheif 1.17 via dlopen** - leaves Ubuntu 22.04, the packages' floor, without HEIC; the API subset
  needed exists in 1.12.
- **libheif on every OS** - not present on macOS or Windows systems; the OS decoders are.
- **A Rust HEVC decoder** - none viable, and it would still be a bundled HEVC decoder.

## Consequences
- HEIC previews depend on the machine: always on macOS, after one or two Store installs on Windows, after a
  package install on Linux (Fedora and openSUSE need a third-party repository). The app and doctor say exactly what to
  install.
- Decoding differs slightly between backends; perceptual hashes tolerate it, and a test bounds the difference.
- A crash inside a system decoder ends the process, as with any in-process decoder.
- `cargo build` needs no system library for HEIC.

## Revisit when
A royalty-free path for HEVC appears, an OS drops its HEIF decoder, or crashes in system decoders justify an
out-of-process decoder.
