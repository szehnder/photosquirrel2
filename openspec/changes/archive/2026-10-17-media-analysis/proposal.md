# Proposal

## Why

Everything PhotoSquirrel decides (what is a photo, which copy is best, what to show in the gallery) starts from
what it can learn about one file. The engine needs fast, read-only, crash-proof media understanding before any
scanning, hashing or grouping can be built: a cheap probe that lets junk be skipped without a full read, full
metadata for the inspector and the best-copy ranking, a rule-based photo classifier, and thumbnails that make a
million-item gallery possible even with the source drive unplugged.

## What Changes

- **Probe** (at most the first 256 KB): magic-number format sniffing, header dimensions, orientation, EXIF block
  and embedded preview location (ADR-0010).
- **Metadata extraction** for images and QuickTime/MP4 videos: camera, lens, exposure, capture date with its
  provenance, GPS, orientation, maker-note presence, JPEG quality estimate and a full raw tag list (ADR-0017).
- **Photo classifier** on probe data: `photo`, `likely_photo`, `graphic`, `thumbnail_cache`, `junk`, each with
  human-readable reasons and settings-driven thresholds (ADR-0015).
- **Decode and thumbnails**: DCT-scaled downscaled decode (>= 512 px long edge) for analysis, 384 px grid
  thumbnails with dominant colour, embedded previews preferred when >= 320 px, 2048 px on-demand previews in a
  4 GB LRU disk cache, a content-addressed `thumbs.db` store, and placeholders when HEIC or ffmpeg support is
  unavailable (ADR-0016, ADR-0017).
- Decoder availability report (HEIC, ffmpeg) for `doctor`.
- Benchmarks for probe, downscaled decode and thumbnail generation on 12 MP JPEGs (feeds the analysis budget of
  ADR-0026).

## Capabilities

### New Capabilities
- `media-probe`: cheap, bounded, read-only inspection of a file's format, dimensions, orientation and embedded preview.
- `media-metadata`: extraction of EXIF and video metadata, capture-date resolution and JPEG quality estimation.
- `photo-classification`: deciding whether a file is a photo (or phone/camera video) worth keeping, with reasons.
- `thumbnails`: downscaled decodes, grid thumbnails, large previews and the thumbnail store.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0010, 0015, 0016, 0017, 0025, 0026, 0032.
- Crates: `ps-model` (media types, classification enums), `ps-media` (probe, metadata, classifier, decode,
  thumbnail encode), `ps-catalog` (`thumbs` module: the separate `thumbs.db` store and preview LRU), `ps-fixtures`
  (small hand-made media fixtures used by unit tests).

## Out of scope

- Content hashes and perceptual hashes (`content-hashing`).
- The main catalog schema, the scan pipeline and I/O lanes (`catalog`, `scanning`).
- Demosaicing RAW sensor data (never done, ADR-0017); video perceptual dedup; HEIC encode.
- LLM confirmation of `likely_photo` (`ai-insights`).
- Any UI.
