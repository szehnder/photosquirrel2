# Proposal

## Why

Duplicate grouping and best-copy choice are safety-critical: a false merge can hide a unique family photo. Real
photos cannot be committed, and eyeballing results does not scale. A deterministic synthetic corpus with known
truth lets every later change measure precision, recall and keeper accuracy against the ADR-0025 gate, and gives
scanning, export and cleanup realistic backup-of-backup trees to run on.

## What Changes

- Seeded procedural "photos" with camera EXIF (several makes/models/serials, DateTimeOriginal, GPS) at
  realistic texture, plus RAW+JPEG pairs and phone videos with QuickTime metadata.
- Derivatives with known truth: exact copies, renames, resized thumbnails, recompressions, EXIF-stripped copies.
- Must-not-merge sets: crops, burst-like near shots, same-scene shots minutes apart, different cameras.
- Junk: icons, UI graphics, palette PNGs, cache thumbnails in cache folders, tiny web images.
- Nested backup-of-backup folder trees spread over several simulated drives (source roots).
- `truth.json` listing every file's category, expected class, true group and expected keeper.
- An evaluator that scores a grouping result against truth (pairwise precision/recall, keeper accuracy,
  must-not-merge violations).
- `photosquirrel-cli fixtures generate` and size presets (`small`, `medium`, `large`).

## Capabilities

### New Capabilities
- `test-corpus`: generation of the deterministic ground-truth corpus and scoring of results against it.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0013, 0014, 0015, 0025, 0027.
- Crates: `ps-fixtures` (generator, truth model, evaluator), `ps-media` (encoders reused for writing JPEG/PNG and
  EXIF), `photosquirrel-cli` (`fixtures generate` subcommand, minimal argument parsing; the full CLI comes in
  `headless-cli`).

## Out of scope

- HEIC fixtures beyond the single checked-in sample (no HEIC encoder in CI); RAW formats other than DNG.
- Real videos with decodable frames (minimal containers only; poster frames are tested with a fake tool).
- Running scans or grouping against the corpus (later changes consume it).
