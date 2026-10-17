# 0022. Copy-only export: layout, verification, sidecars, manifest

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The end goal is importing the deduplicated best copies into Apple Photos, Google Photos or Lightroom. All three import folders of files; Lightroom and Apple Photos read XMP sidecars; Google Photos reads embedded EXIF only.

## Decision
Export builds a **plan** (dry run shown first) of keepers not yet exported to the chosen library, then copies:
- Layout `<library>/<YYYY>/<YYYY-MM-DD>/<original filename>` (date from DateTimeOriginal, else QuickTime creation date, else file mtime marked as "undated" in `<library>/Undated/`); name collisions get ` (2)` suffixes; RAW+JPEG pairs keep matching stems.
- Each copy streams to `<name>.ps-partial`, computing BLAKE3, fsyncs, compares with the catalog hash, preserves mtime, and renames atomically. Mismatches abort that file and are reported.
- Optional **XMP sidecar** (`<name>.xmp`) with AI keywords (`dc:subject`), description, people/pet counts as keywords, and PhotoSquirrel provenance. Source files are never modified (EXIF is not rewritten).
- `<library>/.photosquirrel/manifest.jsonl` appends one record per exported file (hash, source paths of all duplicates, keeper reasons).
Exports are idempotent and resumable: the ledger (ADR-0023) records each completed copy; re-running exports only what is new. Profiles tune the defaults: *Apple Photos* (sidecars on, HEIC + MOV live-photo pairs kept together), *Google Photos* (sidecars off), *Lightroom* (sidecars on, RAW+JPEG kept).

## Alternatives considered
- **Move instead of copy** - violates ADR-0004.
- **Write keywords into the copied files' EXIF** - risks corrupting the copy; sidecars are safer and standard.

## Consequences
- The library folder is self-describing and portable.

## Revisit when
A target library offers a reliable import API worth integrating directly.
