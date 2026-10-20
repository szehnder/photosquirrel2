# Tasks

## 1. Schema and model

- [x] 1.1 Migration 6 (export_library, export_ledger) and plan/ledger types in `ps-model`; verify migration upgrade test in `ps-catalog` (registered after 0004/0005; `export_migration_applies_on_latest_schema` upgrades a v5 catalog; ledger `ps_service::export::CatalogLedger`)
- [x] 1.2 Apple ContentIdentifier extraction in `ps-media::metadata`; verify a unit test on a fixture HEIC/MOV pair (`ps_media::apple`: maker note tag 0x0011, XMP `…:ContentIdentifier`, QuickTime `com.apple.quicktime.content.identifier`; `media_metadata.rs` `live_photo_still_and_video_share_the_apple_content_identifier` on a generated iPhone JPEG still and MOV (deviation: a JPEG still, the fixtures have no HEIC writer; HEIC goes through the same EXIF maker-note path) and `content_identifier_from_xmp`; wired into `Exports::input` so `ps_export::pairs` matches Live Photos by identifier)

## 2. Planner

- [x] 2.1 Plan of unexported keepers with target paths and totals; verify `scenario_export_dry_run_writes_nothing`
- [x] 2.2 Dated and undated layout; verify `scenario_export_dated_file_placed_by_capture_date` and `scenario_export_undated_file_placed_in_undated`
- [x] 2.3 Collision planning and adoption of identical files; verify `scenario_export_name_collision_gets_suffix` and `scenario_export_identical_existing_file_adopted`
- [x] 2.4 Free-space check; verify `scenario_export_insufficient_space_blocks_export`
- [x] 2.5 Library/source overlap check; verify `scenario_export_library_inside_source_refused`
- [x] 2.6 Pairs (RAW+JPEG, Live Photo); verify `scenario_export_raw_jpeg_pair_keeps_stem` and `scenario_export_live_photo_pair_kept_together`
- [x] 2.7 Offline sources; verify `scenario_export_offline_source_files_deferred`
- [x] 2.8 Filing date from the duplicate group (ADR-0043): an EXIF-less keeper is filed by another copy's embedded date, then a file-name or XMP-sidecar date, mtime last; the plan, CLI dry run, Export view and manifest say where the date came from; verify `scenario_export_stripped_keeper_filed_by_group_capture_date` (ps-service, real scan of a stripped and an EXIF copy) and `ps_export::date` unit tests (file-name patterns, sidecar parsing, resolution order)

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

- [x] 5.1 `file_disposition` view; verify `scenario_completion_report_dispositions_derived_after_export` and `scenario_completion_report_covered_requires_exported_keeper` (deviation: no SQL view; `ps_service::export::Exports::dispositions` feeds catalog facts (files, `dup_member`, `user_inclusion`, `export_ledger`) to the single Rust implementation `ps_export::derive_dispositions`, so the two can never disagree; end-to-end in `ps-service/tests/export_service.rs`)
- [x] 5.2 Library existence check and verify mode; verify `scenario_completion_report_deleted_library_copy_becomes_pending` and `scenario_completion_report_verify_detects_altered_copy`
- [x] 5.3 Per-source/per-folder aggregation; verify `scenario_completion_report_report_per_folder` and `scenario_completion_report_excluded_reasons_summarized`
- [x] 5.4 Badge logic; verify `scenario_completion_report_fully_accounted_badge` and `scenario_completion_report_pending_blocks_badge`

## 6. UI and CLI

- [x] 6.1 Export view (plan review, profile, confirm, progress); verify screenshot review and a headless UI test that confirming starts the export (`ui-workspace/tests/export_reports.rs`: `confirming_the_export_starts_it`, `insufficient_space_blocks_the_export_button`; `docs/screenshots/export-dark.png`)
- [x] 6.2 Reports view with drill-down; verify `scenario_completion_report_drill_into_pending_files` (deviation: the drill-down lists the files, with their reasons, inside the Reports view rather than in the gallery, which cannot show reasons and has no disposition filter yet; also Verify copies with the suspect-copy list, `verify_lists_an_altered_library_copy`; `docs/screenshots/reports-dark.png`)
- [x] 6.3 Disposition in inspector and gallery filter; verify `scenario_completion_report_inspector_shows_disposition` (inspector: Export section with disposition, reason and library copy; gallery: "Export status" menu (any of exported, covered, excluded, pending, unreadable, missing), a disposition-aware ID list via `Readers::id_list_within` with the cached dispositions of `ps_service::library` (before any export every file is derived against an empty ledger, so the Library is pending); a disposition filter widens the class and missing-file filters so excluded and missing files can be listed: `gallery_filter_by_export_disposition` (UI) and `disposition_filter_lists_exactly_the_files_with_that_disposition` (ps-service); 1M bench: 1.95 s for the first query (derives every disposition), then 168 ms)
- [x] 6.4 CLI `export` and `report`; verify `scenario_export_cli_dry_run_prints_plan` and `scenario_completion_report_cli_report_matches_ui` (`export dry-run|run|check` and `report` in photosquirrel-cli)

## 7. Verification

- [x] 7.1 `cargo xtask ci` is green
- [x] 7.2 Gate: export the small corpus to a temp library, re-export copies nothing, `report --verify` shows every drive 100% accounted for (small corpus via `photosquirrel-cli export run` then `report --verify`: all three drives 100% accounted for, re-export copies 0; also `export_small_corpus_reexport_copies_nothing_and_every_drive_is_accounted_for` in ps-service)
- [x] 7.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 7.4 `openspec validate export --strict` passes

## 8. M4 gate fixes

- [x] 8.1 Re-plans trust the ledger (ADR-0044): ledger rows record the library copy's modification time (catalog migration 9); `ps_export::library::check_copy` (shared by the planner and `check_library`) trusts a copy with its recorded size and time, re-hashes only a copy whose time changed, and every copy with `--verify`; verify `scenario_export_re_plan_reads_no_file_contents` (counting probe: 0 reads for 24 exported files, 1 after touching one, 24 with verify) and the `export-replan-50k` budget (`ps-export/tests/budgets.rs`, <= 800 ms; measured 398 ms release on NVMe with sidecar rows)
- [x] 8.2 Sidecars have ledger rows (ADR-0045, catalog migration 10 `export_ledger.kind`): `in_progress` before the partial, `done` after the rename, so recovery removes a crashed sidecar's partial; exported bundles without a sidecar get a sidecar-only write; a recorded sidecar keeps its name when its deleted copy is copied again; only `file` rows make a content exported; verify `scenario_export_interrupted_sidecar_is_finished_on_resume` (child process killed before the sidecar rename), `scenario_export_deleted_copy_restored_to_its_name`, `sidecars_are_added_to_an_exported_library`, `ledger_sidecars_migration_marks_old_rows_as_files`
- [x] 8.3 Library copies changed by another app (ADR-0046): a recorded copy at its path with another size (or other bytes) is listed in `Plan::modified`, not copied again; `--recopy-modified` (CLI) / "Copy them again" (Export view) makes a fresh copy beside it; reports keep it pending with the mismatch; verify `scenario_export_library_copy_changed_by_another_app_is_not_copied_again` and `same_size_different_bytes_is_not_already_exported`
- [x] 8.4 A registered export library (or a folder inside or containing it) is refused as a source by `add_source` and `relink_source` (CLI `sources add`, the Sources view): `ScanError::ExportLibrary`; verify `scenario_export_library_refused_as_a_source`
