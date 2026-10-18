# Tasks

## 1. Schema and model

- [ ] 1.1 Migration 6 (export_library, export_ledger) and plan/ledger types in `ps-model`; verify migration upgrade test in `ps-catalog`
- [ ] 1.2 Apple ContentIdentifier extraction in `ps-media::metadata`; verify a unit test on a fixture HEIC/MOV pair

## 2. Planner

- [x] 2.1 Plan of unexported keepers with target paths and totals; verify `scenario_export_dry_run_writes_nothing`
- [x] 2.2 Dated and undated layout; verify `scenario_export_dated_file_placed_by_capture_date` and `scenario_export_undated_file_placed_in_undated`
- [x] 2.3 Collision planning and adoption of identical files; verify `scenario_export_name_collision_gets_suffix` and `scenario_export_identical_existing_file_adopted`
- [x] 2.4 Free-space check; verify `scenario_export_insufficient_space_blocks_export`
- [x] 2.5 Library/source overlap check; verify `scenario_export_library_inside_source_refused`
- [x] 2.6 Pairs (RAW+JPEG, Live Photo); verify `scenario_export_raw_jpeg_pair_keeps_stem` and `scenario_export_live_photo_pair_kept_together`
- [x] 2.7 Offline sources; verify `scenario_export_offline_source_files_deferred`

## 3. Copier

- [x] 3.1 Partial-hash-fsync-compare-mtime-no-replace-rename protocol; verify `scenario_export_verified_copy_renamed` and `scenario_export_existing_file_never_overwritten`
- [x] 3.2 Mismatch handling; verify `scenario_export_hash_mismatch_aborts_file`
- [x] 3.3 Sampled identities; verify `scenario_export_large_video_verified_fully`
- [x] 3.4 Source safety snapshot test; verify `scenario_export_export_leaves_sources_untouched`
- [x] 3.5 Ledger states, resume and fault injection; verify `scenario_export_re_export_copies_only_new`, `scenario_export_interrupted_export_resumes`, `scenario_export_disk_full_stops_cleanly`

## 4. Sidecars, manifest, profiles

- [x] 4.1 XMP writer with effective insights; verify `scenario_export_sidecar_contains_ai_keywords` (parsed back with `quick-xml`)
- [x] 4.2 Manifest appender; verify `scenario_export_manifest_record_per_file`
- [x] 4.3 Profiles; verify `scenario_export_google_profile_omits_sidecars` and `scenario_export_lightroom_profile_keeps_raw_pairs`

## 5. Dispositions and reports

- [ ] 5.1 `file_disposition` view; verify `scenario_completion_report_dispositions_derived_after_export` and `scenario_completion_report_covered_requires_exported_keeper`
- [x] 5.2 Library existence check and verify mode; verify `scenario_completion_report_deleted_library_copy_becomes_pending` and `scenario_completion_report_verify_detects_altered_copy`
- [x] 5.3 Per-source/per-folder aggregation; verify `scenario_completion_report_report_per_folder` and `scenario_completion_report_excluded_reasons_summarized`
- [x] 5.4 Badge logic; verify `scenario_completion_report_fully_accounted_badge` and `scenario_completion_report_pending_blocks_badge`

## 6. UI and CLI

- [ ] 6.1 Export view (plan review, profile, confirm, progress); verify screenshot review and a headless UI test that confirming starts the export
- [ ] 6.2 Reports view with drill-down; verify `scenario_completion_report_drill_into_pending_files`
- [ ] 6.3 Disposition in inspector and gallery filter; verify `scenario_completion_report_inspector_shows_disposition`
- [ ] 6.4 CLI `export` and `report`; verify `scenario_export_cli_dry_run_prints_plan` and `scenario_completion_report_cli_report_matches_ui`

## 7. Verification

- [ ] 7.1 `cargo xtask ci` is green
- [ ] 7.2 Gate: export the small corpus to a temp library, re-export copies nothing, `report --verify` shows every drive 100% accounted for
- [ ] 7.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 7.4 `openspec validate export --strict` passes
