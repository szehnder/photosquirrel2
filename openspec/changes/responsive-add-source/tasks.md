# Tasks

Ships in 0.3.x (ROADMAP row 35), after `damaged-raw-preview`'s slot, in the release after 0.3.1. Takes **ADR-0121**.
**No migration** (`source.total_bytes` and `source.fingerprint` are already nullable; the next free migration number
stays 27).

## 1. Decision

- [x] 1.1 Write ADR-0121 "Sources are registered at constant cost, and the first scan records their fingerprint" (D1-D3; amends 0006), accepted, with an index row. Verify: the index lists it.
  Done: ADR-0121 accepted (`docs/adr/0121-sources-are-registered-at-constant-cost-and-the-first-scan-records-their-fingerprint.md`, amends 0006), with its index row in `docs/adr/README.md`; no migration (`ls crates/ps-catalog/migrations` still ends at 0026_colour.sql).

## 2. Engine

- [x] 2.1 Constant-cost registration: `add_source` without `measure`; `SourceFs::subfolders` for the export-library search; instrumentation `stats` and `entry_stats`. Verify: `scenario_sources_adding_reads_no_file_whatever_the_tree_size`, `scenario_sources_missing_folder_rejected`, `scenario_sources_adding_writes_nothing_to_source`, `scenario_sources_inner_of_existing_reported`, the export-library refusal tests (`ps-scan` remount, ps-service export).
  Done: `Scanner::add_source` keeps every check and drops `source::measure` (removed with `Measured`/`choose_samples`); `SourceFs::subfolders` (entry types only) for `find_export_library`; instrumentation `stat_count`/`entry_stat_count`. `scenario_sources_adding_reads_no_file_whatever_the_tree_size` (6 files vs 3,000 files plus folders below level 4: 0 opens, 0 bytes, 0 entry stats, 12 stats and 6 listings for both), `scenario_sources_missing_folder_rejected`, `scenario_sources_adding_writes_nothing_to_source`, `scenario_sources_inner_of_existing_reported` and the remount export-library tests pass; `library_search_probes_each_device_once` became `library_search_stats_no_file` (Deviations).
- [x] 2.2 The first completed scan records the measure (`Readers::source_measure_rows`, `Command::SetSourceMeasure`, `pipeline::finish`). Verify: `scenario_sources_add_a_source` (rewritten), `scenario_sources_interrupted_first_scan_records_the_fingerprint_later`, a fill job and a rescan record nothing.
  Done: `Readers::source_measure_rows` (one pass: present rows' summed size, identified rows of 1 B to 64 MB, whether any row is identified), `Command::SetSourceMeasure`, `source::from_rows`, `pipeline::record_measures` in `finish` for completed scans of sources with `fingerprint IS NULL` (`RunSource::measure`) that were online throughout. `scenario_sources_add_a_source` (listed at once with no size or fingerprint and 0 opens; after the scan the truth's total size, 1 to 64 samples, every file opened once; an unchanged rescan leaves the source record identical), `scenario_sources_interrupted_first_scan_records_the_fingerprint_later` (cancelled: none; resumed: size and fingerprint), unit `samples_come_from_identified_files_in_range`; a fill job never records (`ctx.fill` excluded).
- [x] 2.3 Re-link without a fingerprint (`Scanner::fingerprint_of`). Verify: `scenario_sources_never_scanned_source_relinked`, `scenario_sources_relink_after_an_interrupted_first_scan`, and unchanged `scenario_sources_remounted_drive_relinked`, `scenario_sources_wrong_drive_rejected`, `scenario_sources_inner_source_folded_into_outer`, every ps-scan test binary.
  Done: `Scanner::fingerprint_of` (the stored samples, else `from_rows` over the identified rows now) and `check_fingerprint`; no samples and no identified row: re-linked without a check. `scenario_sources_never_scanned_source_relinked` (0 opens, then the first scan records the fingerprint), `scenario_sources_relink_after_an_interrupted_first_scan` (drive-c refused with mismatches, the moved drive accepted), and unchanged `scenario_sources_remounted_drive_relinked`, `scenario_sources_wrong_drive_rejected`, `scenario_sources_inner_source_folded_into_outer`; every ps-scan and ps-catalog test binary green.

## 3. Setting

- [x] 3.1 `scanning.scan_new_sources` (default on), the settings window toggle, `doctor`'s report, the `sources add` text. Verify: `scenario_settings_scan_new_sources_setting_persists` (ps-service or CLI), the settings window search test, every photosquirrel-cli test binary.
  Done: `Scanning::scan_new_sources` (default true, serde default, no `SETTINGS_VERSION` bump); the Scanning section toggle "Scan new sources right away"; `doctor` text and `--json` `scan_new_sources`; `sources add` says "not scanned yet". `scenario_settings_scan_new_sources_setting_persists` (photosquirrel-cli: on from an empty directory, off after a save and a new store, `doctor --json` false, the add text) and the settings window tests pass; every ps-model, ps-service and photosquirrel-cli test binary green.

## 4. UI

- [x] 4.1 Pending adds in `ScanMonitor` (`picking`, `adding`, `add_paths`), the pending rows with inline errors, the Add busy state, the status line ("Adding…", "Walking folders", "then N more sources"); `DemoBackend` adds rows with an add delay hook. Verify: `scenario_scan_progress_pending_row_shown_at_once`.
  Done: `ScanMonitor::{picking, adding, add_busy, pick_and_add, add_paths, dismiss_add}`, `PendingAdd`, `AddOutcome`, `AddStart`; the Sources view's pending rows (`source-pending-<n>`, inline error with Dismiss) and the Add busy state ("Adding…", unavailable); `status_line`: "Adding <name>…", "Walking folders", " · then N more sources"; `DemoBackend` registers real rows (`never_scanned`, `add_delay`). `scenario_scan_progress_pending_row_shown_at_once` (the picker answered, the executor stepped until the pending state exists with no add made yet, the next frame shows "Holiday 2024: Adding…", "Adding Holiday 2024…" and Add unavailable; after the 300 ms add the real row and its run) passes.
- [x] 4.2 The shell add path through the monitor; the shell follows every run the monitor sees (D6c). Verify: `scenario_scan_progress_status_bar_follows_a_scan_started_by_add_source`, `scenario_scan_progress_status_bar_shows_progress`.
  Done: `Shell::add_source` goes through the monitor when a scan hub exists (`after_add`: toasts only for failed adds, queued or not-scanned notes); the shell observes the monitor and starts its activity sampling once per run (`followed_run`); `CatalogBackend::add_source` no longer fails when only the start is refused. `scenario_scan_progress_status_bar_follows_a_scan_started_by_add_source` (Library open, Add Source: the run of the new source, "Walking folders"/"Scanning" in `status-activity`, the shell's activity calls rise, the view stays the Library) and `scenario_scan_progress_status_bar_shows_progress` pass.
- [x] 4.3 The queue (D6) and the never-scanned primary action (D6b). Verify: `scenario_scan_progress_queued_add_starts_after_the_active_run`, `scenario_scan_progress_cancelled_run_leaves_the_queue_visible`, `scenario_scan_progress_primary_action_scans_only_never_scanned_sources`, `scenario_scan_progress_start_scan_from_dashboard`.
  Done: `ScanMonitor::queued` (deduplicated), `start_queued` from `run_ended` after a completed run (online ones; offline stay), kept on cancel and pause, removed when started; `SourceRow::first_scan_done`; the primary `scan-new` ("Scan “X”", "Scan N new sources") and the secondary selection label ("Scan 2 sources (1 new)"); row states "Queued, scans next", "First scan not finished", "Not scanned yet". `scenario_scan_progress_queued_add_starts_after_the_active_run` (starts `[[1], [2]]`), `scenario_scan_progress_cancelled_run_leaves_the_queue_visible`, `scenario_scan_progress_primary_action_scans_only_never_scanned_sources` and `scenario_scan_progress_start_scan_from_dashboard` pass.
- [x] 4.4 The setting off and onboarding (D7, D8). Verify: `scenario_settings_new_source_not_scanned_when_the_setting_is_off`, `scenario_app_shell_onboarding_add_without_scanning_leads_to_sources`, `scenario_app_shell_first_launch_shows_onboarding`, every ui-workspace test binary.
  Done: The setting read from `AppSettings` at each add; `selection_hint` applied once by the Sources view; the shell opens Sources after an add from onboarding that did not scan. `scenario_settings_new_source_not_scanned_when_the_setting_is_off` (no run, "Not scanned yet", "Scan “Holiday 2024”", "Scan 1 source"), `scenario_app_shell_onboarding_add_without_scanning_leads_to_sources` and `scenario_app_shell_first_launch_shows_onboarding` pass; the ui-workspace lib and all 18 test binaries green (the ignored medium-corpus test needs a corpus that is not there; `dashboard_controls_on_the_real_engine` passes with `--ignored`).

## 5. Docs

- [x] 5.1 README "Adding sources" (instant add, the setting, the queue, "Scan N new sources"); release notes "What's new since 0.3.1"; ROADMAP row 35. Verify: `grep -n 'Scan new sources right away' README.md packaging/release-notes.md`.
  Done: README "Adding sources" (under Quick start) and the `sources add` line; release notes "What's new since 0.3.1"; ROADMAP row 35; `docs/gpui-notes.md` on answering the folder picker and stepping the executor in tests. `grep -n 'Scan new sources right away' README.md packaging/release-notes.md`: README.md:162, packaging/release-notes.md:55.

## 6. Verification

- [x] 6.1 `cargo xtask ci` green in a clean worktree; every scenario mapped. (GitHub's Linux (full CI) job stands in for a local clean worktree, which does not fit the free disk.)
  Done: not run in a local clean worktree (about 18 GB free; a clean GPUI build needs about 50 GB). GitHub's Linux (full CI) job ran `cargo xtask ci` on a fresh checkout of ae215fb: green (run 41219524741). Local checks: `cargo fmt --check`; clippy -D warnings (all targets) of ps-scan, ps-catalog, ps-model, ps-service, photosquirrel-cli, ui-workspace and photosquirrel; `cargo xtask layering` (17 crates) and `tokens` OK; `openspec validate --all --strict` 32/32 plus the change; `cargo xtask spec-coverage` 684 scenarios, 684 mapped; every test binary of ps-scan, ps-catalog, ps-model, ps-service, photosquirrel-cli and ui-workspace green.
- [x] 6.2 Corpus gate unchanged: release CLI, medium preset seed 42, drive-a/b/c: precision, recall and keeper accuracy 1.0000, 0 violations, 19,741 pairs, 1,903 groups, 8,834 grouped files, 4,166,020,278 reclaimable bytes, 6,338 suggestions.
  Done: release CLI, medium preset seed 42, drive-a/b/c: 10,173 files (5.8 GB) scanned in 21.7 s, 0 errors; `eval-corpus` precision, recall and keeper accuracy 1.0000, 0 must-not-merge violations, 19,741/19,741 pairs, 1,903 keeper groups, gate PASS; `groups list` totals: 8,834 grouped files, 1,903 groups, 4,166,020,278 reclaimable bytes, 6,338 suggestions (the baseline exactly). Each `sources add` printed "not scanned yet"; after the scan `sources list` showed the three sizes (2,026,386,842 + 1,908,887,998 + 1,899,951,228 bytes = the corpus); the unchanged rescan read 0 B; adding drive-a to a fresh catalog took 30 ms (the process included). Corpus removed.
- [x] 6.3 Budgets: `rescan-nochange-100k` within budget.
  Done: `cargo xtask bench --check --only rescan-nochange-100k`: 0.45 s (budget 4 s), within budget.
- [x] 6.4 Screenshots with `cargo xtask screenshot` on a fixture catalog: the pending "Adding…" row if it can be captured, the new setting, the "Not scanned yet" state and the queue; under `target/agent-review/screenshots/add-source/`, not committed.
  Done: `cargo xtask screenshot` (release app; it captured although `loginctl` reports the session locked) on a
  `photosquirrel-cli demo` catalog plus a fixture folder "Holiday 2024" added by `sources add` (small preset, seed 7):
  `target/agent-review/screenshots/add-source/sources-not-scanned-yet-dark.png` and `-light.png` (the new row "Not
  scanned yet", the primary "Scan “Holiday 2024”", the secondary "Scan 4 sources (1 new)"), and
  `setting-scan-new-sources-dark.png` (Settings, Scanning: the new toggle at the foot of the section, on). The
  pending "Adding…" row and the queue were not captured: neither exists by the first frames a scripted capture draws
  (docs/gpui-notes.md), and the headless tests cover both. Not committed; data removed.
- [x] 6.5 Pushed; GitHub CI green on all five jobs.
  Done: fast-forward to origin main (5e33d0a..ae215fb); run 41219524741 green on all five jobs: Linux (full CI), Build and test (windows-2022), Build and test (macos-15), Oldest libheif (Ubuntu 22.04, libheif 1.12), Performance budgets (relative to the parent commit). This tasks-only commit's run: in the hand-off report.
- [ ] 6.6 Gate review by a fresh agent that did not write the code.

- [x] 6.7 Gate-review fixes (PASS-WITH-FIXES): M1-M3 and L1-L8 as listed in design.md, "Gate-review fixes", each
  with its test.
  Done: the 6 new scenarios and their tests pass: sources "Source empty at its first scan records the fingerprint
  later"; scan-progress "Onboarding add unavailable while pending", "Removed queued source leaves the queue",
  "Offline queued source is dropped", "Start refused by another operation says why" and "Source offline during a
  completed run stays never scanned". The changed tests pass: setting off, primary action, status bar follows Add
  Source (the fake engine reports activity), scan_real `exercise`, settings_scan battery.
  Local checks: `cargo fmt --check`; clippy -D warnings (all targets) of ps-catalog, ps-scan, ps-service,
  photosquirrel-cli, ui-workspace and photosquirrel; `cargo xtask layering` and `tokens` OK; `openspec validate
  --all --strict` 32/32 plus the change; spec coverage 690 scenarios, 690 mapped. Every test binary of ps-catalog,
  ps-scan, ps-service, photosquirrel-cli and ui-workspace is green, including `dashboard_controls_on_the_real_engine`
  with `--ignored`.
  Corpus gate (release CLI, medium preset seed 42, drive-a/b/c), exactly the baseline: precision, recall and keeper
  accuracy 1.0000, 0 must-not-merge violations, 19,741/19,741 pairs, 1,903 groups, 8,834 grouped files,
  4,166,020,278 reclaimable bytes, 6,338 suggestions. First scan 23.0 s; the unchanged rescan read 0 B.
  `rescan-nochange-100k`: 0.51 s (budget 4 s).
  Screenshots retaken, not committed, under `target/agent-review/screenshots/add-source/`:
  `sources-not-scanned-yet-{dark,light}.png` (the new row shows only "Not scanned yet") and
  `setting-scan-new-sources-{dark,light}.png` (the toggle heads the Scanning section).
  CI for these commits: in the hand-off report.

