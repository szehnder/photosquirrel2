# 0014. Explainable best-copy ranking

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
For each group we must keep the single best copy: the original or the copy closest to it.

## Decision
Members are ranked lexicographically by an explainable score:
1. Format tier: camera RAW > original camera JPEG/HEIC (has maker notes / camera EXIF) > lossless (PNG/TIFF) > other lossy.
2. Pixel count (larger wins), but a larger file that is an upscale (smaller source pixels with the same pHash and lower JPEG quality estimate) does not win.
3. Metadata completeness (DateTimeOriginal, GPS, camera make/model).
4. Estimated JPEG quality (from quantization tables).
5. File size.
6. Oldest file modification time; then shortest path depth (prefer album folders over nested backup-of-backup paths).
RAW + JPEG pairs from the same capture are kept together as one *keeper set*. The ranking returns a list of human-readable reasons; the user can override the keeper of any group and the override is persisted.

## Alternatives considered
- **Learned quality model** - opaque; may come later as a tie-breaker.

## Consequences
- The UI's group view always explains "Why this copy".

## Revisit when
Users override the automatic choice in more than ~5% of reviewed groups.
