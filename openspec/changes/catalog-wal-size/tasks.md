# Tasks

Ships in 0.3.x (ROADMAP row 36), with 0.3.1. Takes **ADR-0122** (WAL policy) and **ADR-0123** (idle-memory budget).
**No migration.** The next free migration number stays 27.

## 1. Decisions

- [ ] 1.1 Write ADR-0122 "The catalog keeps its write-ahead log small" (D1-D5) and ADR-0123 "The idle-memory budget is 320 MB" (D6, supersedes the idle-RSS part of 0026), both accepted, with index rows. Verify: the index lists both.

## 2. Catalog

- [ ] 2.1 `journal_size_limit`, `db::truncate_wal`, `WalCheckpoint`, run end and shutdown checkpoints (D1, D2, D5). Verify: `scenario_catalog_scan_run_end_empties_the_log`, `scenario_catalog_closed_catalog_leaves_an_empty_log`, unit test that `open_writer` sets the limit, every ps-catalog test binary.
- [ ] 2.2 The background shrink at open for the catalog and the thumbnail store (D3, D4), `wait_for_wal_shrink`. Verify: `scenario_catalog_migrated_catalog_leaves_an_empty_log`, `scenario_catalog_large_leftover_log_emptied_after_open`, `scenario_catalog_large_leftover_thumbnail_log_emptied_after_open`, `scenario_catalog_small_leftover_log_left_to_normal_checkpoints`.
- [ ] 2.3 Measure on a copy of a 1M catalog with a 1 GB log (under `target/agent-review/tmp`, never the bench cache): open time, first ID list, log size before and after, a second open; record in design.md "Measurements".

## 3. Budget

- [ ] 3.1 Idle-memory budget 320 MB: `perf-budgets.toml`, `xtask` ADR table, `scenario_app_shell_idle_memory_within_320_mb`, `docs/performance.md`, code comments. Verify: `grep -rn '250 MB' openspec/specs docs/performance.md perf-budgets.toml crates xtask` names nothing about idle memory outside the archive; `scenario_performance_budgets_all_adr_budgets_defined` passes.

## 4. Docs

- [ ] 4.1 Release notes: fold "What's new since 0.3.1" into "What's new in 0.3.1", add the WAL line, keep "Upgrading from 0.3.0" accurate; ROADMAP row 36. Verify: `grep -n 'since 0.3.1' packaging/release-notes.md` is empty.

## 5. Verification

- [ ] 5.1 Local checks: fmt, clippy -D warnings (all targets) of ps-catalog, ps-scan, photosquirrel and xtask; `cargo xtask layering`, `cargo xtask tokens`; tests of ps-catalog, ps-scan, ps-service; `openspec validate --all --strict`; `cargo xtask spec-coverage`.
- [ ] 5.2 Corpus gate (release CLI, medium preset seed 42, drive-a/b/c) exactly the baseline; `rescan-nochange-100k` and `catalog-insert` within budget.
- [ ] 5.3 Pushed; GitHub CI green on all five jobs.
- [ ] 5.4 Gate review by a fresh agent that did not write the code.
