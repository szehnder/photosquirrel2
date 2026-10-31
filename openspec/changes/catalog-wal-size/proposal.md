# Proposal

## Why

**A leftover write-ahead log makes the first gallery frame slow.** The 1M-file synthetic bench catalog carries a
1.09 GB `catalog.db-wal`, written on 2026-10-30, most likely by migration 26 (`file.colour`, which updates every
analysed row in one transaction). With it, the first gallery frame takes 1,193-1,251 ms instead of 473-487 ms
(budget 600 ms). A copy of the same catalog after `PRAGMA wal_checkpoint(TRUNCATE)` is back to about 480 ms.

Why the log stays:
- The catalog runs only PASSIVE checkpoints (after each scan run, at shutdown, and SQLite's automatic one every
  16,384 pages). A PASSIVE checkpoint copies pages into the database but never shrinks the log file.
- No `journal_size_limit` is set, so a log that is reset keeps its full size on disk.
- The app ends with `cx.quit()`, which does not drop the catalog: its connections are never closed, so SQLite's
  checkpoint-and-delete on the last close never runs. Even a clean close leaves the log, because the read-only
  readers close last and a read-only connection cannot checkpoint.
- Every open then has to rebuild the log's index from the whole file (SQLite's WAL recovery), and readers look up
  pages through it.

Real users who upgrade a large catalog to 0.3.1 get the same slow first frame on every launch, until something
truncates the log. Nothing does today.

**The idle-memory budget is stale.** Idle RSS with the 1M synthetic catalog measures 267-276 MB in both 0.3.0 and
the current HEAD (one desktop run: 290.6 MB), against a 250 MB budget (ADR-0026). The 209.6 MB baseline dates from
2026-10-21. The growth happened across earlier releases, not in 0.3.1. The owner chose to raise the budget.

## What Changes

- **WAL policy (new ADR-0122).**
  - `journal_size_limit` = 64 MiB on every catalog and thumbnail-store writer connection, so a reset log shrinks.
  - A TRUNCATE checkpoint after migrations, at the end of each scan run and at shutdown, waiting only briefly for
    readers.
  - When a catalog or thumbnail store is opened with a log over 8 MiB, or a migration ran, a background thread folds
    the log into the database and truncates it. It runs a PASSIVE pass first, which blocks nobody, then a short
    TRUNCATE. Opening never waits for it, and neither does the UI thread.
- **Idle-memory budget 320 MB (new ADR-0123, supersedes the idle-RSS part of ADR-0026).** This updates
  `perf-budgets.toml`, the app-shell "Fast start" requirement and its scenario (renamed "Idle memory within 320 MB",
  with its test), the performance-budgets requirement, `xtask`'s ADR table, `docs/performance.md` and the code
  comments that cite 250 MB.
- **Release notes:** "What's new since 0.3.1" is folded into "What's new in 0.3.1", since 0.3.1 is not tagged and
  everything ships together, plus one line on the WAL fix.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `catalog`: ADDED "Small write-ahead log".
- `app-shell`: "Fast start" REMOVED and re-ADDED as "Start-up time and idle memory" (320 MB; the idle scenario is renamed "Idle memory within 320 MB", which a MODIFIED block cannot do).
- `performance-budgets`: MODIFIED "Every budget defined" (idle memory 320 MB).

## Impact

- `crates/ps-catalog`: `db.rs` (pragma, checkpoint helpers, the background shrink), `writer.rs` (run end,
  shutdown), `lib.rs` (`Catalog` starts and joins the shrink), `thumbs.rs` (the same at open).
- No migration, and no change to the file format or schema. The next free migration number stays 27.
- `perf-budgets.toml`, `xtask/src/bench.rs`, `crates/photosquirrel/tests/budgets.rs`, `docs/performance.md`,
  `packaging/release-notes.md`, `openspec/ROADMAP.md` (row 36).
- The corpus gate, `rescan-nochange-100k` and `catalog-insert` are unchanged.
