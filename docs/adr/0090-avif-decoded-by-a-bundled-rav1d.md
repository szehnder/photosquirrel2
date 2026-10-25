# 0090. AVIF decoded by a bundled rav1d

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0017](0017-format-support.md) (AVIF via libheif)

## Context
ADR-0017 decoded AVIF with libheif alongside HEIC, so no package decodes AVIF either. AV1 is covered by the
royalty-free AOMedia Patent License 1.0, so there is no patent reason to depend on the OS. dav1d is the reference
decoder (BSD-2); its Rust binding needs a system `libdav1d`, which the packages would have to build per platform and
architecture with meson and nasm. `rav1d` 1.1 is a Rust port of dav1d (BSD-2-Clause) on crates.io, built by cargo,
with optional assembly that needs nasm on x86_64.

## Decision
- AVIF stills are decoded on every platform by **`rav1d`** (features `bitdepth_8`, `bitdepth_16`, `asm`), wrapped in a
  safe `ps-media::av1` module (one single-threaded decoder per call, frame size limit, contained panics).
- The AVIF container is parsed by `ps-media::bmff` (`av1C`, `colr`, `pixi`, `irot`, `imir`, `auxC`, `grid` via
  `dimg`); YUV is converted to RGB in `ps-media` (nclx matrix and range, bilinear chroma); 10/12-bit is reduced to 8.
- `nasm` is a build tool for x86_64 builds, required in CI, the packaging image and release runners (packaging refuses
  a build without assembly). The binaries' default-on `avif-asm` feature enables rav1d's `asm`; a contributor without
  nasm builds with `--no-default-features` at slower AVIF speed, and `doctor` reports when AVIF assembly is off.
- `THIRD-PARTY-LICENSES` carries rav1d's BSD-2 notice through the cargo-about flow (ADR-0058); packaging fails if it
  is missing.

## Alternatives considered
- **dav1d via dav1d-sys / `image`'s `avif-native`** - a C library to build, lipo and link `/MT` per package.
- **The OS decoder for AVIF too** - ties a royalty-free format to the same installs as HEIC.
- **`re_rav1d`** - an unmaintained 2024 fork.
- **No assembly** - several times slower AV1 decoding to save a build tool.
- **nasm always required, no opt-out** - every contributor on x86_64 would need nasm for a format few photos use.

## Consequences
- AVIF decodes identically on every platform; one more BSD-2 notice; nasm on x86_64 build machines.
- HDR (PQ/HLG) AVIF is not tone-mapped; good enough for thumbnails and perceptual hashing.

## Revisit when
rav1d offers a safe Rust API, a pure-Rust AV1 decoder of similar speed needs no assembler, or AVIF sequences matter.
