# 0122. The catalog keeps its write-ahead log small

- Status: accepted
- Date: 2026-10-31
- Deciders: maintainers
- Amends: [0005](0005-sqlite-catalog-single-writer.md) (the checkpoint policy of the catalog and of `thumbs.db`)

## Context
The catalog runs in WAL mode (ADR-0005). Until now it ran PASSIVE checkpoints only: SQLite's automatic one every
16,384 pages, one after each scan run and one at shutdown. It set no `journal_size_limit`. A PASSIVE checkpoint
copies pages into the database but never shrinks the log, and a log that is reset keeps its largest size on disk.

The log also outlives every process:
- The app ends with `cx.quit()`, which does not drop the catalog, so no connection is closed and SQLite's
  checkpoint-and-delete on the last close never runs.
- On a clean close, the read-only readers close after the writer, and a read-only connection cannot checkpoint.

At the next open, the first connection rebuilds the log index from every committed page in the log (WAL
recovery). Migration 26 (`file.colour`, ADR-0113) updates every analysed row in one transaction. On the 1M-file
synthetic bench catalog it left a 1.09 GB log. Measured:
- with that log, opening took 630-740 ms (WAL recovery), against 3 ms with an empty log; the 1M ID list itself took
  141-154 ms against 131-137 ms;
- the app's first gallery frame was 1,193-1,251 ms instead of 473-487 ms (budget 600 ms), on every launch, until
  something truncated the log.

Nothing did, and users who upgrade a large catalog to 0.3.1 would hit the same thing.

## Decision
**Every writer connection sets `journal_size_limit` = 64 MiB.** That is one automatic checkpoint interval (16,384
pages of 4 KiB). A log that is reset is cut back to that size, and scans that stay inside it do not shrink and
regrow the file at every reset. It applies to the catalog and to `thumbs.db`, both opened through
`db::open_writer`.

**TRUNCATE instead of PASSIVE, with a bounded wait.** `wal::truncate(conn, wait)` runs
`PRAGMA wal_checkpoint(TRUNCATE)` with the busy timeout set to `wait`, then restores the 5 s timeout. When readers
outlast the wait, SQLite finishes as a PASSIVE checkpoint and reports `busy`. Nothing fails, and the size limit
applies at the next reset. It is used:
- after a terminal run status, waiting 100 ms. The writer does it only once its queue has been quiet for 100 ms, so
  after the scanner's final status write and flush, never inside them: a reader holding the log would otherwise
  delay the final status, and with it a cancel. A queued run does not stall behind a reader for more than that wait;
- at writer shutdown, waiting 500 ms, so `Catalog::close` and drop leave an empty log although the readers close
  last.

**A background shrink at open.** When `Catalog::open_file` or `ThumbStore::open_file` migrated an existing file, or
finds more than 8 MiB of committed log, a `catalog-wal` thread folds and truncates the log. The size is counted in
committed frames as the opening connection sees them after recovery (`PRAGMA wal_checkpoint(NOOP)`), not in file
bytes: a log that was reset keeps its file size under `journal_size_limit` but costs the next open nothing, and
would otherwise start the thread at every launch. 8 MiB of committed log is about 5 ms of WAL recovery (about
0.66 ms per MiB), noise against the first-frame budget. The thread:
1. waits 5 s after opening (`SHRINK_DELAY`), so the first gallery frame and its thumbnails are drawn first;
2. opens its own read-write connection and runs a PASSIVE checkpoint. This is the long copy, 5-8 s for 1.09 GB
   including the fsyncs, and it takes no write lock;
3. runs TRUNCATE tries of at most 1 s each, up to 10, 250 ms apart (a cancel stops it between tries). Only these hold the write lock, and only for the
   pages written since step 2. Each try is shorter than the writer's own 5 s busy timeout, so a write that collides
   with one waits and never fails.

Opening never waits for the thread. `wait_for_wal_shrink()` starts it at once if it is still waiting, and returns
how it ended. Tests and the synthetic bench setup use it. Dropping the catalog or store, and `Catalog::close`, cancel a shrink that has
not started, stop one that is running between its tries (the PASSIVE copy itself cannot be interrupted), and join
it, before the writer stops and the owner lock is released; `Catalog::close_after_wal_shrink` lets it finish
first. The join can take as long as the copy of a large log, so the catalog must not be dropped on the UI thread.
The writer's shutdown TRUNCATE covers a cancelled shrink.

A migration's log is handled by the same background thread, not by a synchronous checkpoint in `open_file`.
Folding 1 GB costs seconds, and an upgrade launch should not wait for it.

**The synthetic bench catalog is settled before a measured launch.** `synthetic::ensure` opens, shrinks and closes
a reused catalog and its thumbnail store before the measured child starts, so a migration or a leftover log is
never inside the measurement.

## Alternatives considered
- **Checkpoint synchronously at open, before the readers.** This adds 5-8 s to the launch that heals a 1 GB log,
  against 0.7 s per launch for keeping it.
- **Start the background shrink at once.** In 5 of 6 measured runs the first ID list was unaffected (145-153 ms),
  but in one, right after the catalog was copied, it took 2.8 s. The 5 s delay keeps the first frame clear of the
  copy's I/O, and costs only that a session shorter than 5 s does not heal (its next close or open does).
- **TRUNCATE in the background without the PASSIVE pass.** A TRUNCATE takes the write lock before it copies, so the
  writer would wait the whole copy and fail after 5 s.
- **Close the catalog on quit.** This depends on GPUI's quit path, and a killed process leaves the log anyway. The
  open-time shrink covers both.
- **A lower `wal_autocheckpoint`.** More frequent checkpoints during scans, with no effect on the log a single large
  transaction leaves.

## Consequences
- After a migration, a scan run or a close, the log is empty, or at most 64 MiB when readers were in the way. A
  catalog opened with a large leftover log is slow to open once. It is healed about 5-15 s later, and every later
  open is fast again (measured on the bench catalog's copy: 3 ms open, 131-137 ms ID list).
- Closing a catalog with a large log (the CLI, after a migration) runs the copy at exit, once.
- A second read-write connection exists for the length of a shrink. It writes no rows, so ADR-0005's single writer
  holds for data.
- No migration and no schema change.

## Revisit when
The app closes its catalog on quit, or a migration writes more than about 1 GB of log.
