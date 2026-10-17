# Proposal

## Why

Priority three: once a drive is proven fully accounted for, the user may want to reclaim space and make it
obvious what is left. This must never risk a photo: nothing is deleted, every move is re-verified at the moment
it happens and every move can be undone.

## What Changes

- Cleanup plan (dry run) per source or folder: files whose disposition is `exported` or `covered`, with counts and
  bytes; explicit confirmation required (ADR-0004, ADR-0023, ADR-0024).
- Immediately before each move: re-hash the source file and confirm the library copy exists and matches.
- Move by same-volume rename into `<source root>/.photosquirrel-quarantine/<run id>/<relative path>`; no data
  copy, no cross-volume moves.
- Write-ahead journal in the catalog and as JSON inside the quarantine run folder; crash reconciliation.
- Undo of a whole run or a subset; never overwriting a path that has been reused.
- Scanner ignores quarantine folders; reports show quarantined files distinctly.
- Cleanup view and `photosquirrel-cli cleanup plan|run|undo|list`.

## Capabilities

### New Capabilities
- `cleanup`: reversible quarantine of already-safe files on source drives, with plan, verification, journal and undo.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0006, 0008, 0011, 0022, 0023, 0024, 0027. New ADR needed for a `quarantined` disposition (see design).
- Crates: `ps-export` (`quarantine` module: the only source-mutating code), `ps-catalog` (migration: cleanup_run,
  cleanup_move), `ps-scan` (ignore quarantine folders), `ps-service` (cleanup API, report extension),
  `ui-workspace` (Cleanup view), `photosquirrel-cli` (`cleanup`).

## Out of scope

- Deleting anything, emptying the quarantine, moving to the OS trash (ADR-0024: the user empties it with their
  file manager).
- Cleaning files that are pending, excluded, unreadable or missing; cross-volume moves; cleaning the library.
