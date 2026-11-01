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

`wal::truncate(conn, wait)` (new module `ps-catalog/src/wal.rs`) sets `busy_timeout(wait)`, runs `PRAGMA wal_checkpoint(TRUNCATE)` and restores the
connection's 5 s timeout. It returns `WalCheckpoint { busy, log_frames, checkpointed }`.

A TRUNCATE takes the write lock, waits for readers on older snapshots, copies, then waits until no reader uses the
log and truncates it. When the wait runs out, SQLite carries on as a PASSIVE checkpoint and reports `busy`. Nothing
fails; D1's limit shrinks the log at the next reset.

Waits:
- the end of a run: 100 ms, after the writer's queue has been quiet for 100 ms (D5);
- shutdown: 500 ms, so quitting is never held up for seconds;
- the background shrink: 1 s per try (D3).

### D3. Background shrink at open

`Catalog::open_file` and `ThumbStore::open_file` start a `catalog-wal` thread when they migrated an existing file,
or when more than 8 MiB of log is committed (`wal::SHRINK_AT_OPEN`, counted in frames with
`PRAGMA wal_checkpoint(NOOP)` after the opening connection's recovery, because a reset log keeps its file size). A new file and an observer start none. The thread:
1. waits `wal::SHRINK_DELAY` (5 s) after opening, so the first gallery frame and its thumbnails come first;
2. opens its own read-write connection (`journal_size_limit` set) and runs a PASSIVE checkpoint. This is the long
   copy, 5-8 s for the bench catalog's 1.09 GB including the fsyncs. It takes no write lock, so the writer and the
   readers carry on;
3. runs D2's TRUNCATE with a 1 s wait, retried up to 10 times, 250 ms apart, while it reports `busy`. Only this step
   holds the write lock, and only for the pages written since step 2. The writer's 5 s busy timeout is longer than
   any single try, so a write that collides with it waits and never fails (`writes_during_the_background_shrink_succeed`).

The thread logs the log size before and after, and the time it took.

`wait_for_wal_shrink()` (on `Catalog` and `ThumbStore`) starts a waiting shrink at once, joins it and returns its
`WalShrink` (`before`, `after`, `complete`, `elapsed`). Tests and the synthetic bench setup (D6) use it.

`Catalog` and `ThumbStore` keep the task as their first field. Dropping it, or `Catalog::close`, cancels a shrink that
has not started, stops one that runs between its TRUNCATE tries, and joins it (`close_after_wal_shrink` lets it end
first; the join can take as long as the PASSIVE copy, so neither is for the UI thread), before the writer shuts down and the owner lock is released. No second connection
outlives the owner, and a short CLI command does not wait 5 s at exit. The writer's shutdown TRUNCATE (D5) then
empties the catalog's log anyway. A process that quits without dropping the catalog just ends the thread. SQLite
checkpoints are crash-safe: pages are copied before the log is reset.

**Why 8 MiB.** A leftover log costs WAL recovery at every open, about 0.66 ms per MiB of committed pages. 8 MiB
costs about 5 ms, which is noise against the 600 ms budget. Below that, the fsyncs of a checkpoint at every start
are not worth it, and D1 plus the automatic checkpoint keep the log bounded anyway. The thumbnail store's 14 MiB
log in the bench cache is above the threshold.

**Before or after the first gallery query: after.** The first open of a catalog with a large log still pays its WAL
recovery. The first connection does that inside `open_writer`, before any query, and nothing can avoid it short of
checkpointing before the open. So that open only gets healed for the opens after it. The remaining question is when
the copy runs:
- Synchronously before the readers: 5-8 s added to that launch. Rejected.
- Concurrently from the start: in 5 of 6 runs it did not slow the first ID list, but in one run it took 2.8 s
  instead of 0.15 s (the first run after copying 2.3 GB, so likely the copy's write-back competing).
- Delayed 5 s, then concurrently: chosen, as insurance for the first frame. The only cost is that a session shorter
  than 5 s does not heal by itself. Its writer's shutdown TRUNCATE does, when it closes, and so does the next open.

### D4. Migrations

The migrations run on the writer connection inside `open_file`. Their checkpoint is D3's background thread
(triggered by "a migration ran"), not a synchronous TRUNCATE in `open_file`. Folding a 1 GB migration log costs
seconds, and the first frame of an upgrade launch should not wait for it.

### D5. Run end and shutdown

The PASSIVE checkpoint after a terminal `SetRunStatus` becomes D2's TRUNCATE (100 ms wait). It does not run inside the
status write: the scanner submits the final status, flushes, and only then reports the run done, so a wait there
(a reader holding the log) would be part of every cancel (control latency budget 250 ms; CI once measured 130 ms
against a baseline of 81 ms). The status write sets a flag, and the writer checkpoints when its queue has been quiet
for 100 ms (`wal::RUN_END_QUIET`), so after the flush. At writer shutdown, PASSIVE
becomes TRUNCATE (500 ms), so `Catalog::close` and drop leave an empty log even though the readers close last.

### D6. The synthetic bench catalog is settled before a measured launch

The measured app quits without closing its catalog, so a migration in a measured launch leaves its log for every
later measured launch. That is how the bench cache got its 1 GB log. `ps_service::library::synthetic::ensure`,
which runs in the parent process before the measured child, now opens a reused catalog, runs `wait_for_wal_shrink`
and closes it, and does the same for the thumbnail store. The measured launch then never pays a migration or a WAL
recovery. On an up-to-date catalog with an empty log this costs a few milliseconds. The owner's bench cache is
healed by the next `cargo xtask bench` run. This change never opened it.
Test: `ensure_folds_a_leftover_log_into_a_reused_catalog`.

### D7. Idle-memory budget 320 MB (ADR-0123)

Measured with the 1M synthetic catalog: 267-276 MB in 0.3.0 and at HEAD, and 290.6 MB once on the desktop. 320 MB is
10% above the worst observation and about 17% above the typical one. A real regression of the 0.3.1 kind (the 440 MB
of mapped reader pages in ADR-0026's reader note) still fails it clearly. Rejected:
- 300 MB: 3% of headroom over a measurement whose runs spread by 9%;
- 350 MB: it would hide a 25% growth.

`perf-baseline.json` keeps 209.6 unless a reliable measurement is taken (see "Measurements").

## Risks / Trade-offs

- [The background PASSIVE pass competes for I/O with what the user does at second 5] → It happens only once per
  large log. Measured below: an ID list during the copy was not slowed.
- [A reader that never ends keeps TRUNCATE busy] → The log is still copied by the PASSIVE pass, and D1 truncates it
  at the next reset.
- [A second read-write connection in the process] → It never writes rows, only checkpoints, so ADR-0005's single
  writer holds for data.

## Measurements

On a copy of the bench cache's 1M-file catalog (`catalog.db` 1.2 GB plus its 1.09 GB log, no `-shm`), under
`target/agent-review/tmp`, release build, through a temporary example that is not committed: `Catalog::open_file`,
then the 1M ID list (`Filter::default()`, `Sort::default()`, the gallery's first query), then the shrink. Each run
used a fresh copy.

| | open | first 1M ID list | open to IDs | log after |
|---|---|---|---|---|
| Old code, 3 runs (no shrink) | 680-726 ms | 141-154 ms | 834-867 ms | 1,088,054,952 B, every time |
| New code, first open, 6 runs | 629-743 ms | 146-153 ms (once 2,764 ms, shrink started at once) | 778-846 ms | 0 B after the shrink: 4.8-8.3 s |
| New code, next opens, 2 runs | 3-6 ms | 131-137 ms | 135-143 ms | 0 B |

- The ID list during a running shrink (300 ms into it) took 145-162 ms.
- The open cost of a leftover log is WAL recovery (about 0.66 ms per MiB). The ID list itself pays only about
  10-15 ms for reading through the log.
- So the first gallery frame of the launch after the healing one drops by about 700 ms, consistent with the
  coordinator's 1,193-1,251 ms with the log against 473-487 ms after a TRUNCATE.
- The app's window measurement (`--measure-first-gallery-frame`) needs the unlocked desktop, so it is not repeated
  here.

`perf-baseline.json` `idle-rss` stays 209.6. A reliable measurement needs the release app's window on the owner's
unlocked desktop (`--measure-idle-memory`). The numbers in ADR-0123 come from the coordinator's runs, which are not
a baseline run of `cargo xtask bench --update-baseline`.


### Gate-review fixes: control latency, idle-rss baseline

Control latency with the run-end checkpoint inside the status write versus after the flush (release, `ps-scan`
`control_latencies`, 20 trials of cancel and pause at random moments, two runs each; the readers variant has two
threads reading the catalog throughout, as the gallery does):

| | without readers | with readers |
|---|---|---|
| checkpoint inside the status write (wait 250 ms) | 94.8-112.8 ms | 101.1-120.4 ms |
| checkpoint after the flush (this change) | 72.9-81.1 ms | 77.7-83.8 ms |

CI's single measurement (130 ms against 81 ms) was runner noise (single runs 74-171 ms), but the table shows the
in-path wait costs 20-30 ms at p95 even locally, so the checkpoint left the control path.

`idle-rss` in `perf-baseline.json` stays 209.6 MB. ADR-0123 raises the budget to 320 MB, but a local
`cargo xtask bench --check` of the release app measures 267-290 MB, which is more than 15% and 5% of the budget above
that baseline, so it reports a relative regression until the owner runs `cargo xtask bench --update-baseline` on the
reference desktop (it needs the unlocked desktop). CI skips app measurements, so it does not show it. The baseline
file is not edited by guess.
