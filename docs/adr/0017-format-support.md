# 0017. Formats: JPEG/PNG/TIFF/WebP/GIF native, RAW via rawler, HEIC via libheif, video metadata native, poster frames via ffmpeg

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The collection spans two decades: JPEGs, PNGs, camera RAW, iPhone HEIC and phone videos (MOV/MP4).

## Decision
- **JPEG**: `zune-jpeg` for full decodes, `jpeg-decoder` DCT scaling (1/2, 1/4, 1/8) for fast thumbnails; quality estimated from quantization tables.
- **PNG/TIFF/WebP/GIF/BMP**: `image` crate.
- **Camera RAW** (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2...): `rawler` for metadata and the embedded full-size JPEG preview; RAW sensor data is never demosaiced.
- **HEIC/HEIF/AVIF stills**: `libheif` behind the cargo feature `heic` (default in release builds; CI installs libheif). Without it, HEIC files are still catalogued (metadata, dimensions, exact dedup) and marked "preview unavailable".
- **EXIF/metadata** for images and QuickTime/MP4 videos: `nom-exif` (pure Rust).
- **Video poster frames**: an optional `ffmpeg` executable on PATH or configured in settings; without it videos show a styled placeholder with duration and date.

## Alternatives considered
- **Shell out to exiftool** - Perl runtime dependency, slow per-file process spawn.
- **Bundled ffmpeg libraries** - large licence/size cost for poster frames only.

## Consequences
- The engine reports which optional decoders are available (`photosquirrel-cli doctor`).

## Revisit when
A pure-Rust HEVC decoder becomes viable; then drop the libheif dependency.
