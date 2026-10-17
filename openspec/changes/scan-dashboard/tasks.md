# Tasks

## 1. Service additions

- [ ] 1.1 ETA estimator in `ps-service::scans::eta`; verify `scenario_scan_progress_eta_after_warm_up` and `scenario_scan_progress_eta_estimating_at_start`
- [ ] 1.2 Run summaries and full error listing; verify `scenario_scan_progress_run_summary_on_completion`
- [ ] 1.3 `FakeScanService` for UI tests; verify a unit test that it emits snapshots at the configured rate

## 2. Dashboard

- [ ] 2.1 Start scan with source picker; verify `scenario_scan_progress_start_scan_from_dashboard`
- [ ] 2.2 Rescan diff panel; verify `scenario_scan_progress_rescan_diff_shown`
- [ ] 2.3 Stage bars and per-source rows with sparklines; verify `scenario_scan_progress_stage_counts_shown` and `scenario_scan_progress_per_source_rows_shown`
- [ ] 2.4 Recent finds strip and errors list; verify `scenario_scan_progress_recent_finds_show_thumbnails` and `scenario_scan_progress_errors_listed_with_reason`
- [ ] 2.5 Controls; verify `scenario_scan_progress_pause_from_dashboard`, `scenario_scan_progress_cancel_from_dashboard`, `scenario_scan_progress_resume_from_dashboard`
- [ ] 2.6 100 ms sampling loop and constant cost; verify `scenario_scan_progress_snapshot_sampled_at_most_10_hz` and `scenario_scan_progress_ui_cost_independent_of_rate`
- [ ] 2.7 Status-bar progress; verify `scenario_scan_progress_status_bar_shows_progress`

## 3. Verification

- [ ] 3.1 `cargo xtask ci` is green
- [ ] 3.2 Gate: live progress, pause/resume/cancel from the UI on the medium corpus (headless UI test with the real service) and screenshots reviewed
- [ ] 3.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 3.4 `openspec validate scan-dashboard --strict` passes
