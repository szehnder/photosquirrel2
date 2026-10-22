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
