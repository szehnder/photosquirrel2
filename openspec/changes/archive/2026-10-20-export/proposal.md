# Proposal

## Why

Priority one: get the single best copy of every photo out of the pile of backups, into one clean, dated library
ready for Apple Photos, Google Photos or Lightroom, without any risk to the sources. Afterwards the user must be
able to ask of any drive: "Is everything worth keeping on it already safe in my library?"

## What Changes

- Export plan (dry run) of keepers not yet exported: target paths, counts, bytes, collisions, free-space check;
  nothing is written before confirmation (ADR-0004, ADR-0022).
- Layout `<library>/<YYYY>/<YYYY-MM-DD>/<original name>`, undated files under `<library>/Undated/`, ` (2)`
  collision suffixes, never overwriting; RAW+JPEG and Live Photo pairs keep matching stems.
- Copy protocol: stream to `.ps-partial` while hashing, fsync, compare with the catalog hash, preserve mtime,
  atomic rename; mismatch aborts that file (ADR-0011, ADR-0022).
- Optional XMP sidecars (AI keywords, description, people/pet counts, provenance) and `manifest.jsonl`.
- Profiles: Apple Photos, Google Photos, Lightroom.
- Export ledger, idempotent and resumable exports (ADR-0023).
- Dispositions (exported, covered, excluded, pending, unreadable, missing) and completion reports per source and
  per folder with a "100% accounted for" badge, in the app and the CLI (ADR-0023).
- Export view and Reports view; `photosquirrel-cli export` and `report`.

## Capabilities

### New Capabilities
- `export`: copying the best copies into a library folder safely, verifiably and resumably.
- `completion-report`: deriving every file's disposition and reporting whether a source is fully accounted for.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0011, 0014, 0021, 0022, 0023, 0025, 0027, 0028.
- Crates: `ps-model` (plan, ledger, disposition types), `ps-export` (planner, copier, XMP writer, manifest,
  profiles), `ps-catalog` (migration: export_library, export_ledger), `ps-service` (exports, reports),
  `ui-workspace` (Export and Reports views, disposition in the inspector), `photosquirrel-cli` (`export`, `report`).

## Out of scope

- Moving or deleting anything (`quarantine-cleanup` later moves, never deletes).
- Writing metadata into copied files' EXIF (ADR-0022); direct import APIs of Apple/Google/Adobe.
- Exporting non-keepers on purpose (all copies) and custom folder templates beyond the ADR layout.
