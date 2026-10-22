# Tasks

## 1. Decisions and schema

- [x] 1.1 ADR-0034 "Quarantined disposition" accepted (verify: listed in `docs/adr/README.md`)
- [x] 1.2 Migration 7 (cleanup_run, cleanup_move) and the quarantined flag in `file_disposition`; verify migration upgrade test in `ps-catalog`
  (Shipped as catalog migration 14, the next free number, per ADR-0061; dispositions are derived in `ps-export`, so the flag is `FileFacts::quarantined` from `cleanup_move` states 2/4. Verified by `ps-catalog/tests/cleanup_migration.rs`.)
- [x] 1.3 Extend the forbidden-call check allow-list to `ps-export::quarantine`; verify `cargo xtask layering` passes and its unit test rejects `fs::rename` elsewhere

## 2. Plan

- [x] 2.1 Eligibility query and plan rows; verify `scenario_cleanup_pending_files_never_eligible` and `scenario_cleanup_excluded_files_not_eligible`
- [x] 2.2 Plan presentation without side effects; verify `scenario_cleanup_plan_before_move`

## 3. Run

- [x] 3.1 Re-verification of source and library copy; verify `scenario_cleanup_changed_source_skipped` and `scenario_cleanup_missing_library_copy_blocks_move`
- [x] 3.2 Same-volume no-replace rename; verify `scenario_cleanup_move_is_a_same_volume_rename` and `scenario_cleanup_cross_volume_move_refused`
- [x] 3.3 Write-ahead journals; verify `scenario_cleanup_journal_records_every_move`
- [x] 3.4 Crash reconciliation (child-process kill test); verify `scenario_cleanup_crash_mid_run_is_recoverable`

## 4. Undo

- [x] 4.1 Full and partial undo with conflict handling; verify `scenario_cleanup_undo_restores_run`, `scenario_cleanup_partial_undo_by_folder`, `scenario_cleanup_occupied_path_not_overwritten`
- [x] 4.2 Never-delete invariant over cleanup + undo; verify `scenario_cleanup_cleanup_never_deletes`

## 5. Integration

- [x] 5.1 Scanner/report handling of quarantined files; verify `scenario_cleanup_quarantine_ignored_by_scanner`
- [x] 5.2 CLI `cleanup plan|run|list|undo`; verify `scenario_cleanup_no_move_without_confirmation` and `scenario_cleanup_cli_undo_round_trip`
- [x] 5.3 Cleanup view (plan review, confirm, progress, history with Undo); verify a headless UI test that Undo restores a run and screenshot review

## 6. Verification

- [x] 6.1 `cargo xtask ci` is green (clean worktree at the UI commit)
- [x] 6.2 Gate: move + undo round trip proven on the small and medium corpora (tree snapshots equal)
  (Release CLI, seed 42, all three drives scanned and exported (lightroom), then per drive: `cleanup plan`, `cleanup run --yes`,
  `report`, `cleanup undo --yes`, snapshot of every folder and file hash outside the quarantine folder. Small: 217/193/162 files
  moved, 0 skipped, 0 conflicts, snapshots equal (405/511/366 entries). Medium: 3,041/2,635/2,965 files (2.0/1.8/1.8 GB) moved
  in 161/99/94 s, 0 skipped, 0 conflicts, snapshots equal (4,805/4,599/4,302 entries); the report showed them quarantined with
  0 pending, and a rescan after the undos found 0 missing, 0 quarantined, fully accounted for.)
- [x] 6.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 6.4 `openspec validate quarantine-cleanup --strict` passes

## 7. Gate-review fixes (reviewer verdict PASS-WITH-FIXES; ADR-0062 to ADR-0066)

- [x] 7.1 (HIGH) A quarantined file whose library copy is gone or differs is pending, not accounted for, with its
  quarantine path and the reason (badge, text report, drill-down); export copies such a keeper from its quarantine
  path, read-only (ADR-0066); README: verify before deleting a quarantine folder, which the app never deletes;
  verify `scenario_cleanup_quarantined_file_without_library_copy_not_accounted`, `scenario_cleanup_export_reads_quarantined_keeper`
- [x] 7.2 (MEDIUM-HIGH) Runs bound to their source: removing or folding a source with quarantined files is refused,
  source ids a run names are never reused, migration 15 stores the source identity, undo/list use the stored root
  when the source is gone, quarantined files match by source + path + fingerprint (ADR-0063); verify
  `scenario_cleanup_source_with_quarantined_files_not_removed`, `scenario_cleanup_run_of_removed_source_refused`,
  `ps-catalog/tests/cleanup_source_identity.rs`
- [x] 7.3 (MEDIUM) No check-then-rename: `MoveFileExW` without replace on Windows, link + verified-inode unlink on
  Unix without RENAME_NOREPLACE, otherwise refused (ADR-0062); verify `scenario_cleanup_target_appearing_mid_move_not_replaced`,
  `fallback_moves_the_same_inode`, Windows/macOS CI
- [x] 7.4 (MEDIUM) Cleanup catalog writes committed with `synchronous=FULL`; reconcile resolves planned/skipped and
  quarantined moves against `journal.jsonl` and the file system (ADR-0064); verify
  `scenario_cleanup_lost_catalog_commit_recovered`, `cleanup_writes_are_committed_durably`, `durable_writes_use_synchronous_full`
- [x] 7.5 (MEDIUM) One operation slot: scan, export, cleanup plan/run/undo and reconcile exclude each other;
  reconcile skips while a run is live (ADR-0065); verify `scenario_cleanup_operations_exclude_each_other`
- [x] 7.6 (LOW) Reconcile re-hashes (content identity) before deciding where a file is; verify
  `reconcile_compares_content_not_only_size`
- [x] 7.7 (LOW) Layering move check parses with `syn` and scans tokens: imports and globs, unlink via
  libc/rustix/nix, Win32 moves, `std::fs::copy`, `//` in strings, `#[cfg(test)]` items, build.rs, benches,
  examples and xtask; verify the `layering::tests` unit tests and `cargo xtask layering`
- [x] 7.8 (LOW) A torn journal record never swallows the next one; verify `a_torn_record_never_swallows_the_next_one`
- [x] 7.9 (LOW) Wording: rescans count quarantined files (not missing), the text report lists quarantined counts,
  README matches when "Free up space" appears and how moves work, the plan's quarantine column keeps the run folder
  (`quarantine_target_keeps_the_run_folder`, `docs/screenshots/cleanup-dark.png` recaptured)
- [x] 7.10 Gate re-run (release CLI, fresh medium corpus seed 42, 9,933 files; all three drives scanned and exported,
  lightroom): drive-b `cleanup run` moved 2,635 files (1.8 GB) in 53 s, 0 skipped; report showed 2,635 quarantined,
  100% accounted for; a rescan counted 2,635 quarantined, 0 missing; `cleanup undo` restored 2,635 in 51 s;
  path/size/SHA-256 snapshots before and after equal (4,599 entries). Defect 1 repro (drive-a, 3,041 moved; one
  exported keeper's library copy deleted): `report --verify` said "3 pending (3 only in the quarantine folder,
  library copy gone)"; `export run` copied it again from the quarantine path (0 failed, quarantined file unchanged)
  and the drive was 100% accounted for again. Defect 2 repro (drive-c, 2,965 moved): `sources remove 3` and adding
  the folder that contains the drives were refused; after undo, removing source 3 and adding another folder gave
  it id 4, and run 3 still lists drive-c. All undos complete: 0 files left in any quarantine folder.
- [x] 7.11 (LOW-MEDIUM, re-review) A failed `COMMIT` (or a batch SQLite rolled back by itself) fails the
  writer's `exec` / `exec_durable` / `add_source` / `start_run` instead of returning `Ok`; the cleanup run stops
  before the rename when its `moving` intent cannot be committed; verify `a_failed_durable_commit_is_an_error`,
  `run_stops_before_the_rename_when_the_intent_commit_fails` (a trigger whose deferred foreign key fails at commit)
- [x] 7.12 (LOW-MEDIUM, re-review) A quarantined file counts as accounted for only while a valid library copy
  covers it, also when it became excluded after the move; otherwise pending in quarantine (ADR-0067); verify
  `scenario_cleanup_excluded_quarantined_file_without_library_copy_not_accounted`,
  `excluded_quarantined_files_need_a_valid_library_copy`
- [x] 7.13 (LOW, re-review) Layering move check: `fs_err` (and `fs_err::tokio`), the `remove_dir_all` crate,
  `Command::new("rm" | "rmdir" | "del" | "mv" | ...)`, `tempfile` `persist` (not `persist_noclobber`), and code
  under `cfg(any(test, feature = ..))` (only exactly `#[cfg(test)]` is skipped); verify `fs_err_calls_are_caught`,
  `remove_dir_all_crate_is_caught`, `shell_file_commands_are_caught`,
  `tempfile_persist_is_caught_but_not_persist_noclobber`, `only_pure_cfg_test_is_skipped`
- [x] 7.14 (LOW, re-review) Wording: the plan shows "Space you can free" (a move stays on the drive; the space is
  freed once the user deletes the quarantine folder), likewise the CLI help and README;
  `docs/screenshots/cleanup-dark.png` recaptured
- [x] 7.15 (TRIVIAL, re-review) A failed link or rename says why: source gone (`ENOENT`), permission (`EACCES`,
  `EPERM` except `EPERM` from `link` on the user's own file, which means no hard links), unsupported (`ENOTSUP`,
  `EOPNOTSUPP`, `ENOSYS`); never a plain rename; verify `move_failures_are_classified`,
  `source_gone_before_the_link_is_not_unsupported`, `read_only_target_folder_is_a_permission_error`
- [x] 7.16 (TRIVIAL, re-review) Doc link to `file_is_quarantined_sql!` in `ps-catalog/src/groups.rs`
