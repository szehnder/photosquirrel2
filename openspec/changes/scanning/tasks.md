# Tasks

## 1. Read-only file layer

- [ ] 1.1 `ps-scan::fs::SourceFs` (read-only opens) and test-only `InstrumentedFs`; verify a unit test that `SourceFs` exposes no write path
- [ ] 1.2 Extend `cargo xtask layering` with the `ps-scan` forbidden-call check (ADR-0004); verify an xtask unit test that feeds a source snippet containing `File::create` and asserts the check fails, and `cargo xtask layering` passes on the workspace

## 2. Sources

- [ ] 2.1 Add source with volume identity, device class and 64-file fingerprint; verify `scenario_sources_add_a_source` and `scenario_sources_missing_folder_rejected`
- [ ] 2.2 Prove no writes into the tree; verify `scenario_sources_adding_writes_nothing_to_source`
- [ ] 2.3 Relative path storage; verify `scenario_sources_paths_are_relative`
- [ ] 2.4 Offline detection and re-link by fingerprint; verify `scenario_sources_remounted_drive_relinked`, `scenario_sources_wrong_drive_rejected`, `scenario_sources_unplugged_source_is_offline`
- [ ] 2.5 Overlap folding; verify `scenario_sources_inner_source_folded_into_outer` and `scenario_sources_inner_of_existing_reported`
- [ ] 2.6 Remove source (catalog only); verify `scenario_sources_remove_keeps_files_on_disk`

## 3. Pipeline

- [ ] 3.1 Walker, diff and stage graph with checkpoints; verify `scenario_scanning_corpus_files_reach_final_stages`
- [ ] 3.2 Read-only guarantees on the full pipeline; verify `scenario_scanning_scanner_never_opens_for_writing` and `scenario_scanning_read_only_tree_scans_fully`
- [ ] 3.3 Probe-only junk and single full read; verify `scenario_scanning_junk_costs_only_a_probe` and `scenario_scanning_one_full_read_per_candidate`
- [ ] 3.4 Adaptive lanes with a simulated-device trait; verify `scenario_scanning_lane_starts_by_device_class`, `scenario_scanning_lane_grows_while_throughput_improves`, `scenario_scanning_lane_shrinks_on_latency_rise`
- [ ] 3.5 Byte semaphore and per-reader slots; verify `scenario_scanning_memory_cap_respected`
- [ ] 3.6 Failure records and retry limit; verify `scenario_scanning_unreadable_file_recorded` and `scenario_scanning_failed_retried_at_most_twice`
- [ ] 3.7 Progress counters and recent ring; verify `scenario_scanning_snapshot_counts_match_catalog` and `scenario_scanning_recent_lists_bounded`

## 4. Incremental rescan

- [ ] 4.1 Stat-fingerprint diff report before analysis; verify `scenario_incremental_rescan_diff_counts_before_analysis` and `scenario_incremental_rescan_unchanged_files_not_read`
- [ ] 4.2 Changed-file re-analysis; verify `scenario_incremental_rescan_edited_file_re_analyzed`
- [ ] 4.3 Missing and returning files; verify `scenario_incremental_rescan_missing_file_kept_in_history` and `scenario_incremental_rescan_returning_file_restored`
- [ ] 4.4 Move re-identification by content identity; verify `scenario_incremental_rescan_moved_file_keeps_history`
- [ ] 4.5 Hash upgrade from `thumbs.db`; verify `scenario_incremental_rescan_hash_version_bump_uses_thumbnails`

## 5. Scan control

- [ ] 5.1 Run rows, single-active-run rule; verify `scenario_scan_control_completed_run_recorded` and `scenario_scan_control_second_scan_rejected_while_running`
- [ ] 5.2 Cancel token, pause condvar and flush-on-stop; verify `scenario_scan_control_cancel_within_250_ms`, `scenario_scan_control_pause_within_250_ms`, `scenario_scan_control_no_partial_results_visible`
- [ ] 5.3 Resume; verify `scenario_scan_control_resume_skips_finished_files`
- [ ] 5.4 Interrupted detection at start-up (child-process kill test); verify `scenario_scan_control_killed_scan_becomes_interrupted`
- [ ] 5.5 Source lost mid-scan; verify `scenario_scan_control_source_vanishes_mid_scan`

## 6. Service facade

- [ ] 6.1 `ps-service::sources` and `ps-service::scans` (add/list/relink/remove, start/pause/resume/cancel/snapshot/runs); verify the scenarios above run through the facade (integration tests in `ps-service/tests/`)

## 7. Benchmarks

- [ ] 7.1 Analysis throughput budget; verify `scenario_scanning_analysis_throughput_within_budget`
- [ ] 7.2 No-change rescan budget; verify `scenario_incremental_rescan_no_change_rescan_under_4_s`
- [ ] 7.3 Control latency budget; verify `scenario_scan_control_control_latency_within_budget`
- [ ] 7.4 Register `scan-analysis-12mp` (max 6.67 ms per file wall time = 150 files/s; budgets are upper bounds), `rescan-nochange-100k` (max 4 s) and `control-latency-p95` (max 250 ms) in `perf-budgets.toml`; verify `cargo xtask bench --check`

## 8. Verification

- [ ] 8.1 `cargo xtask ci` is green
- [ ] 8.2 Gate: scan, cancel, resume and rescan integration tests pass on the small and medium corpora
- [ ] 8.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 8.4 `openspec validate scanning --strict` passes
