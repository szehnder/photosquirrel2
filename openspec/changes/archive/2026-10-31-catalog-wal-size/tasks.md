# Tasks

Ships in 0.3.x (ROADMAP row 36), with 0.3.1. Takes **ADR-0122** (WAL policy) and **ADR-0123** (idle-memory budget).
**No migration.** The next free migration number stays 27.

## 1. Decisions

- [x] 1.1 Write ADR-0122 "The catalog keeps its write-ahead log small" (D1-D5) and ADR-0123 "The idle-memory budget is 320 MB" (D7, supersedes the idle-RSS part of 0026), both accepted, with index rows. Verify: the index lists both.
  Done: `docs/adr/0122-the-catalog-keeps-its-write-ahead-log-small.md` (amends 0005) and
  `docs/adr/0123-the-idle-memory-budget-is-320-mb.md` (supersedes the idle-RSS bound of 0026), both accepted;
  `docs/adr/README.md` lists both after 0121. ADR-0026 is unchanged.

## 2. Catalog

- [x] 2.1 `journal_size_limit`, `wal::truncate`, `WalCheckpoint`, run end and shutdown checkpoints (D1, D2, D5). Verify: `scenario_catalog_scan_run_end_empties_the_log`, `scenario_catalog_closed_catalog_leaves_an_empty_log`, unit test that `open_writer` sets the limit, every ps-catalog test binary.
  Done: `ps-catalog/src/wal.rs` (`WAL_SIZE_LIMIT`, `truncate`, `WalCheckpoint`, `wal_len`/`wal_path`); `db::open_writer` sets `journal_size_limit`; the writer sets a flag at a terminal `SetRunStatus` and runs `wal::truncate` (100 ms wait) once its queue has been quiet for 100 ms, so after the scanner's final flush (gate review M1), and again at shutdown (500 ms wait). `scenario_catalog_scan_run_end_empties_the_log`, `the_run_end_checkpoint_does_not_delay_the_final_flush` and `scenario_catalog_closed_catalog_leaves_an_empty_log` pass (both fail with the old PASSIVE checkpoints), plus units `writer_connections_limit_the_log` and `truncate_empties_the_log_and_restores_the_busy_timeout`; every ps-catalog test binary is green.
- [x] 2.2 The background shrink at open for the catalog and the thumbnail store (D3, D4), `wait_for_wal_shrink`; the synthetic bench catalog settled before a measured launch (D6). Verify: `scenario_catalog_migrated_catalog_leaves_an_empty_log`, `scenario_catalog_large_leftover_log_emptied_after_open`, `scenario_catalog_large_leftover_thumbnail_log_emptied_after_open`, `scenario_catalog_small_leftover_log_left_to_normal_checkpoints`, `writes_during_the_background_shrink_succeed`, unit `a_shrink_waits_for_its_delay_and_drop_cancels_it`, ps-service `ensure_folds_a_leftover_log_into_a_reused_catalog`.
  Done: `wal::ShrinkTask` (5 s `SHRINK_DELAY`, PASSIVE then up to 10 TRUNCATE tries of 1 s, cancelled on drop if not started, joined if started), started by `Catalog::open_file` and `ThumbStore::open_file` after an upgrade or with a log over 8 MiB; `wait_for_wal_shrink` on both; `synthetic::settle` in `ensure_at`. The four scenarios, `writes_during_the_background_shrink_succeed`, `a_shrink_waits_for_its_delay_and_drop_cancels_it` and `ensure_folds_a_leftover_log_into_a_reused_catalog` pass. With the shrink and the TRUNCATE disabled, all six wal.rs scenarios fail. ps-catalog, ps-scan and ps-service: 70 test binaries, 0 failed.
- [x] 2.3 Measure on a copy of a 1M catalog with a 1 GB log (under `target/agent-review/tmp`, never the bench cache): open time, first ID list, log size before and after, a second open; record in design.md "Measurements".
  Done: design.md "Measurements": with the 1.09 GB log, open took 630-740 ms (WAL recovery) on every open; after the background shrink (4.8-8.3 s, log 1,088,054,952 B to 0 B), open took 3-6 ms. The 1M ID list took 141-154 ms with the log and 131-137 ms after, and 145-162 ms during a shrink. The copies were removed, and the bench cache was never opened.

## 3. Budget

- [x] 3.1 Idle-memory budget 320 MB: `perf-budgets.toml`, `xtask` ADR table, `scenario_app_shell_idle_memory_within_320_mb`, `docs/performance.md`, code comments. Verify: `grep -rn '250 MB' openspec/specs docs/performance.md perf-budgets.toml crates xtask` names nothing about idle memory outside the archive; `scenario_performance_budgets_all_adr_budgets_defined` passes.
  Done: `perf-budgets.toml` 320.0 (comment cites ADR-0123), `xtask` `ADR_0026` table and the missing-measurement test, `scenario_app_shell_idle_memory_within_320_mb` (with an ignored placeholder of the old name, see 5.4), `docs/performance.md`, the db.rs reader comment. `scenario_performance_budgets_all_adr_budgets_defined` and every xtask test pass (72). The grep finds 250 MB only in the main specs that the delta replaces at the archive, in the perf-budgets comment that records the old bound, and in the db.rs note of the old budget.
- [ ] 3.2 Owner: at the next run on the reference desktop, `cargo xtask bench --check` and then `cargo xtask bench --update-baseline` for `idle-rss` (it needs the unlocked desktop; the baseline is 209.6 MB and a local check shows a relative regression against it until it is updated; CI skips app measurements). Verify: `perf-baseline.json` `idle-rss` holds the new measurement and a following `bench --check` passes.

## 4. Docs

- [x] 4.1 Release notes: fold "What's new since 0.3.1" into "What's new in 0.3.1", add the WAL line, keep "Upgrading from 0.3.0" accurate; ROADMAP row 36. Verify: `grep -n 'since 0.3.1' packaging/release-notes.md` is empty.
  Done: the responsive-add-source bullet heads "What's new in 0.3.1", followed by "Large catalogs stay quick to open
  after the update"; "Upgrading from 0.3.0" adds that the first launch on a large catalog takes a few seconds longer
  while it is upgraded. The memory budget is not mentioned, since it changes no behaviour. ROADMAP row 36 was added in
  the spec commit. `grep -n 'since 0.3.1' packaging/release-notes.md`: no match.

## 5. Verification

- [x] 5.1 Local checks: fmt, clippy -D warnings (all targets) of ps-catalog, ps-scan, photosquirrel and xtask; `cargo xtask layering`, `cargo xtask tokens`; tests of ps-catalog, ps-scan, ps-service; `openspec validate --all --strict`; `cargo xtask spec-coverage`.
  Done: `cargo fmt --all --check`; clippy -D warnings (all targets) of ps-catalog, ps-service, xtask and photosquirrel are clean; `cargo xtask layering` OK (17 crates); `cargo xtask tokens` OK; ps-catalog, ps-scan, ps-service tests green; `openspec validate --all --strict` 32/32 and the change valid; `cargo xtask spec-coverage`: 694 scenarios, 694 mapped.
  Gate-review fixes rerun: 428 tests passed (ps-catalog, ps-scan, ps-service), xtask 72, fmt, clippy -D warnings, layering, tokens, `openspec validate --all --strict` (32/32) and spec-coverage (694/694) are clean.
- [x] 5.2 Corpus gate (release CLI, medium preset seed 42, drive-a/b/c) exactly the baseline; `rescan-nochange-100k` and `catalog-insert` within budget.
  Done: release CLI, medium preset seed 42, drive-a/b/c: 10,173 files (5.8 GB) in 23.1 s, 0 errors. `eval-corpus` precision, recall and keeper accuracy 1.0000, 0 must-not-merge violations, 19,741/19,741 pairs, 1,903 keeper groups, gate PASS. `groups list`: 8,834 grouped files, 1,903 groups, 4,166,020,278 reclaimable bytes, 6,338 suggestions, exactly the baseline. After the scan `catalog.db-wal` was 0 B, and `thumbs.db-wal` was capped at 67,108,864 B by the size limit. The unchanged rescan read 0 B. `cargo xtask bench --check`: `rescan-nochange-100k` 0.54 s (budget 4 s), `catalog-insert` 14.19 us (budget 20 us), `catalog-id-list-1m` 132.99 ms (budget 300 ms). Corpus removed.
  Gate-review fixes rerun: same corpus, 10,173 files in 22.0 s, precision, recall and keeper accuracy 1.0000, 0 violations, 19,741/19,741 pairs, 1,903 groups, 8,834 grouped files, 4,166,020,278 reclaimable bytes, 6,338 suggestions (the baseline exactly); `catalog.db-wal` 0 B, the unchanged rescan read 0 B in 0.2 s. `bench --check`: `rescan-nochange-100k` 0.52 s, `control-latency-p95` 79.77 ms (baseline 81.06), `catalog-insert` 14.07 us best of 3 (single runs 14-20 us on the busy machine). The readers variant of the control-latency test: 77.7-83.8 ms, against 101-120 ms with the checkpoint inside the status write (design.md).
- [x] 5.3 Pushed; GitHub CI green on all five jobs.
  Done: run 41258654771 on e37b9e3, all five jobs green. The run before it (41253183807, 26a2aa3) failed only its relative performance job: `control-latency-p95` 130 ms against a 81 ms baseline, with single runs from 74 to 171 ms on the shared runner. Locally the same test gives 95-98 ms with this change and 95 ms with the run-end checkpoint put back to PASSIVE, so the change does not move it; the rerun passed.
- [x] 5.4 At the archive: remove the ignored placeholder `scenario_app_shell_idle_memory_within_250_mb` in `crates/photosquirrel/tests/budgets.rs`, which keeps the main spec's old scenario name mapped until the delta replaces it. Verify: `cargo xtask spec-coverage` after the archive.
- [x] 5.5 Gate review by a fresh agent that did not write the code.
  Done: PASS-WITH-FIXES (M1 run-end checkpoint off the control path, M2 baseline task 3.2, L3-L7); fixed in 0f4fb61 and a4f703d; run 41266510192 green on all five jobs. Task 3.2 stays with the owner (needs the unlocked desktop).
