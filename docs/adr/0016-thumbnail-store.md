# 0016. Thumbnails in a content-addressed SQLite store; embedded previews first

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The gallery must show hundreds of thousands of thumbnails instantly, repeatedly, from drives that may be unplugged.

## Decision
Thumbnails are stored in a separate SQLite database `thumbs.db` keyed by content identity (one thumbnail per unique image, shared by duplicates). Each row stores a 384 px (long edge) JPEG (quality 82, ~20-35 KB) and dominant colour. Large previews (2048 px) are generated on demand into an LRU disk cache capped at 4 GB. When a file has an embedded EXIF/RAW/HEIC preview >= 320 px, it is used instead of decoding the full image (orders of magnitude faster for RAW and HEIC). The UI decodes thumbnails with zune-jpeg on background threads and uploads them as GPU textures (ADR-0018).

## Alternatives considered
- **One file per thumbnail** - 1M small files are slow to create, copy and delete; SQLite blobs < 100 KB read faster than the filesystem.
- **WebP/AVIF thumbnails** - smaller, but decoding is 2-5x slower than JPEG, and decode speed bounds scrolling.

## Consequences
- The library stays browsable with sources offline.

## Revisit when
Thumbnail DB exceeds ~30 GB for typical users.
