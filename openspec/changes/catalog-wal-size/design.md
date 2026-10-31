# Design

## Context

- `ps-catalog/src/db.rs` `open_writer` sets WAL mode, `synchronous=NORMAL` and `wal_autocheckpoint=16384` (64 MiB
  at 4 KiB pages). It sets no `journal_size_limit`, so a log that is reset keeps its largest size on disk.
- `writer.rs` runs `PRAGMA wal_checkpoint(PASSIVE)` after a terminal `SetRunStatus` and at shutdown. PASSIVE copies
  pages into the database but never truncates the log.
- `Catalog::open_file` runs the migrations on the writer connection, then opens the readers and the writer thread.
  Migration 26 updates every analysed row in one transaction, about 1 GB of log on 1M rows.
- The app ends with `cx.quit()`; the catalog is never dropped, so no connection is closed. On a clean close, the
  read-only readers close last, and a read-only connection cannot checkpoint. Either way the log outlives the
  process.
- On open, the first connection rebuilds the log index from the whole log file (WAL recovery). That cost grows with
  the committed pages left in the log.
- `ThumbStore` (`thumbs.db`) uses the same `open_writer`, and has no checkpoint of its own. The bench cache's
  `thumbs.db` is 4 KiB with a 14 MiB log.

## Goals / Non-Goals

**Goals:**
- After migrations, scan runs and close, the log is empty or at most 64 MiB.
- A large leftover log, from an older build or a killed process, is folded and truncated soon after open, off the
  UI thread, without delaying open or the first gallery query.
- A guard, so that losing any of this fails a test.

**Non-Goals:**
- Making the app close its catalog on quit (GPUI's quit path). The open-time shrink covers what quit leaves.
- Changing `wal_autocheckpoint` or `synchronous`.
- Changing the first-frame budget. Its measurement on the owner's cache heals at the next open.

## Decisions

### D1. `journal_size_limit` = 64 MiB on every writer connection (ADR-0122)

This is the size of one automatic checkpoint interval (`wal_autocheckpoint` 16,384 pages of 4 KiB). Normal scans
stay inside it, so the file does not shrink and regrow on every reset during a scan. A log that grew past it (one
big transaction, or readers holding the reset back) is cut back to 64 MiB at the next reset. It applies to the
catalog and to `thumbs.db`, because both open through `db::open_writer`.

### D2. One TRUNCATE helper with a bounded wait

`db::truncate_wal(conn, wait)` sets `busy_timeout(wait)`, runs `PRAGMA wal_checkpoint(TRUNCATE)` and restores the
connection's 5 s timeout. It returns `WalCheckpoint { busy, log_frames, checkpointed }`.

A TRUNCATE takes the write lock, waits for readers on older snapshots, copies, then waits until no reader uses the
log and truncates it. When the wait runs out, SQLite carries on as a PASSIVE checkpoint and reports `busy`. Nothing
fails; D1's limit shrinks the log at the next reset.

Waits:
- the end of a run: 250 ms, so the next queued run does not stall behind a reader;
- shutdown: 500 ms, so quitting is never held up for seconds;
- the background shrink: 1 s per try (D3).

### D3. Background shrink at open

`Catalog::open_file` and `ThumbStore::open_file` start a `catalog-wal` thread when a migration ran, or when the log
file is over 8 MiB. The thread opens its own read-write connection and:
1. runs a PASSIVE checkpoint. This is the long copy (about 1 GB for the bench cache), and it takes no write lock,
   so the writer and the readers carry on;
2. runs D2's TRUNCATE with a 1 s wait, retried up to 10 times, 250 ms apart, while it reports `busy`. Only this step
   holds the write lock, and only for the few pages written since step 1. The writer's 5 s busy timeout is longer
   than any single try, so a write that collides with it waits, never fails.

The thread logs the log size before and after, and the time it took. `Catalog` and `ThumbStore` keep its handle and
join it before the writer shuts down and before the owner lock is released. No second connection then outlives the
owner. A process that quits without dropping the catalog just ends the thread. SQLite checkpoints are crash-safe:
pages are copied before the log is reset.

`Catalog::wait_for_wal_shrink()` (and the same on `ThumbStore`) joins the thread and returns its `WalShrink`
report. Tests use it, and so can the CLI.

**Why 8 MiB.** A leftover log costs its WAL recovery at every open, about 0.66 ms per MiB of committed pages (1.09 GB
cost about 720 ms of first frame). 8 MiB costs about 5 ms, which is noise against the 600 ms budget. Under that the
fsyncs of a checkpoint at every start are not worth it, and D1 plus SQLite's automatic checkpoint keep the log
bounded anyway. The thumbnail store's 14 MiB log in the bench cache is over the threshold.

**Before or after the first gallery query.** The thread starts at once, concurrently with the first query. Readers
are never blocked by a checkpoint. The first open of a catalog with a large log still pays its WAL recovery, which
the first connection does inside `open_writer`. Only the opens after that are healed. Measured in
"Measurements" below.

### D4. Migrations

The migrations run on the writer connection inside `open_file`. Their checkpoint is D3's background thread
(triggered by "a migration ran"), not a synchronous TRUNCATE in `open_file`. Folding a 1 GB migration log costs
seconds, and the first frame of an upgrade launch should not wait for it.

### D5. Run end and shutdown

The PASSIVE checkpoint after a terminal `SetRunStatus` becomes D2's TRUNCATE (250 ms). At writer shutdown, PASSIVE
becomes TRUNCATE (500 ms), so `Catalog::close` and drop leave an empty log even though the readers close last.

### D6. Idle-memory budget 320 MB (ADR-0123)

Measured with the 1M synthetic catalog: 267-276 MB in 0.3.0 and at HEAD, and 290.6 MB once on the desktop. 320 MB is
10% above the worst observation and about 17% above the typical one. A real regression of the 0.3.1 kind (the 440 MB
of mapped reader pages in ADR-0026's reader note) still fails it clearly. Rejected:
- 300 MB: 3% of headroom over a measurement whose runs spread by 9%;
- 350 MB: it would hide a 25% growth.

`perf-baseline.json` keeps 209.6 unless a reliable measurement is taken (see "Measurements").

## Risks / Trade-offs

- [The background PASSIVE pass competes for I/O with the first gallery query] → It happens only once per large log.
  Measured below.
- [A reader that never ends keeps TRUNCATE busy] → The log is still copied by the PASSIVE pass, and D1 truncates it
  at the next reset.
- [A second read-write connection in the process] → It never writes rows, only checkpoints, so ADR-0005's single
  writer holds for data.

## Measurements

(Filled in during implementation.)
