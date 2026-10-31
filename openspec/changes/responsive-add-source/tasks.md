# Tasks

Ships in 0.3.x (ROADMAP row 35), after `damaged-raw-preview`'s slot, in the release after 0.3.1. Takes **ADR-0121**.
**No migration** (`source.total_bytes` and `source.fingerprint` are already nullable; the next free migration number
stays 27).

## 1. Decision

- [x] 1.1 Write ADR-0121 "Sources are registered at constant cost, and the first scan records their fingerprint" (D1-D3; amends 0006), accepted, with an index row. Verify: the index lists it.
  Done: ADR-0121 accepted (`docs/adr/0121-sources-are-registered-at-constant-cost-and-the-first-scan-records-their-fingerprint.md`, amends 0006), with its index row in `docs/adr/README.md`; no migration (`ls crates/ps-catalog/migrations` still ends at 0026_colour.sql).

## 2. Engine

- [ ] 2.1 Constant-cost registration: `add_source` without `measure`; `SourceFs::subfolders` for the export-library search; instrumentation `stats` and `entry_stats`. Verify: `scenario_sources_adding_reads_no_file_whatever_the_tree_size`, `scenario_sources_missing_folder_rejected`, `scenario_sources_adding_writes_nothing_to_source`, `scenario_sources_inner_of_existing_reported`, the export-library refusal tests (`ps-scan` remount, ps-service export).
- [ ] 2.2 The first completed scan records the measure (`Readers::source_measure_rows`, `Command::SetSourceMeasure`, `pipeline::finish`). Verify: `scenario_sources_add_a_source` (rewritten), `scenario_sources_interrupted_first_scan_records_the_fingerprint_later`, a fill job and a rescan record nothing.
- [ ] 2.3 Re-link without a fingerprint (`Scanner::fingerprint_of`). Verify: `scenario_sources_never_scanned_source_relinked`, `scenario_sources_relink_after_an_interrupted_first_scan`, and unchanged `scenario_sources_remounted_drive_relinked`, `scenario_sources_wrong_drive_rejected`, `scenario_sources_inner_source_folded_into_outer`, every ps-scan test binary.

## 3. Setting

- [ ] 3.1 `scanning.scan_new_sources` (default on), the settings window toggle, `doctor`'s report, the `sources add` text. Verify: `scenario_settings_scan_new_sources_setting_persists` (ps-service or CLI), the settings window search test, every photosquirrel-cli test binary.

## 4. UI

- [ ] 4.1 Pending adds in `ScanMonitor` (`picking`, `adding`, `add_paths`), the pending rows with inline errors, the Add busy state, the status line ("Adding…", "Walking folders", "then N more sources"); `DemoBackend` adds rows with an add delay hook. Verify: `scenario_scan_progress_pending_row_shown_at_once`.
- [ ] 4.2 The shell add path through the monitor; the shell follows every run the monitor sees (D6c). Verify: `scenario_scan_progress_status_bar_follows_a_scan_started_by_add_source`, `scenario_scan_progress_status_bar_shows_progress`.
- [ ] 4.3 The queue (D6) and the never-scanned primary action (D6b). Verify: `scenario_scan_progress_queued_add_starts_after_the_active_run`, `scenario_scan_progress_cancelled_run_leaves_the_queue_visible`, `scenario_scan_progress_primary_action_scans_only_never_scanned_sources`, `scenario_scan_progress_start_scan_from_dashboard`.
- [ ] 4.4 The setting off and onboarding (D7, D8). Verify: `scenario_settings_new_source_not_scanned_when_the_setting_is_off`, `scenario_app_shell_onboarding_add_without_scanning_leads_to_sources`, `scenario_app_shell_first_launch_shows_onboarding`, every ui-workspace test binary.

## 5. Docs

- [ ] 5.1 README "Adding sources" (instant add, the setting, the queue, "Scan N new sources"); release notes "What's new since 0.3.1"; ROADMAP row 35. Verify: `grep -n 'Scan new sources right away' README.md packaging/release-notes.md`.

## 6. Verification

- [ ] 6.1 `cargo xtask ci` green in a clean worktree; every scenario mapped. (GitHub's Linux (full CI) job stands in for a local clean worktree, which does not fit the free disk.)
- [ ] 6.2 Corpus gate unchanged: release CLI, medium preset seed 42, drive-a/b/c: precision, recall and keeper accuracy 1.0000, 0 violations, 19,741 pairs, 1,903 groups, 8,834 grouped files, 4,166,020,278 reclaimable bytes, 6,338 suggestions.
- [ ] 6.3 Budgets: `rescan-nochange-100k` within budget.
- [ ] 6.4 Screenshots with `cargo xtask screenshot` on a fixture catalog: the pending "Adding…" row if it can be captured, the new setting, the "Not scanned yet" state and the queue; under `target/agent-review/screenshots/add-source/`, not committed.
- [ ] 6.5 Pushed; GitHub CI green on all five jobs.
- [ ] 6.6 Gate review by a fresh agent that did not write the code.
