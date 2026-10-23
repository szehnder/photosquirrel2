# Tasks

## 1. Decisions

- [x] 1.1 ADR-0069, ADR-0070 and ADR-0071 reviewed and moved to accepted before code lands; verify they are listed in `docs/adr/README.md` with their amends notes (0069 amends 0022/0043, 0070 amends 0022)

## 2. Date types, parsers and zone (core)

- [x] 2.1 `ps-model`: `DateProvenance::FolderName` ("folder-name"), `DatePrecision { Second, Day, Month }`, `CaptureDate::precision` (serde default `Second`) and `CaptureDate::is_embedded`; verify a unit test that metadata JSON written before this change still deserialises with precision `Second`
- [x] 2.2 New `ps-media::date` module: move `date_in_name` and `parse_date_text` from `ps_export::date` (re-exported there) and add `date_in_folders`; verify the moved unit tests pass unchanged and `scenario_media_metadata_folder_date_patterns`, `scenario_media_metadata_nearest_folder_wins`
- [x] 2.3 `LocalZone`, `FixedZone`, test `RuleZone` and the feature-gated `OsZone` (`os-zone`, `chrono::Local`); verify `cargo xtask layering` passes, a unit test of `RuleZone` DST switching, and one `OsZone` smoke test that agrees with `chrono::Local` for three instants
- [x] 2.4 `resolve_capture_date` over `EmbeddedDates` (original, create, container, recorded offset), folder date with the plausibility rule, instant conversion for mtime and +00:00 container dates; `extract_metadata` takes a `DateContext`; verify `scenario_media_metadata_folder_date_below_exif_and_above_file_time`, `scenario_media_metadata_source_root_name_not_used`, `scenario_media_metadata_backup_folder_date_ignored`, `scenario_media_metadata_unset_camera_clock_does_not_block_folder_date`, `scenario_media_metadata_month_folder_stays_a_month` (resolver half), `scenario_media_metadata_file_time_in_the_local_zone`, `scenario_media_metadata_daylight_saving_of_the_file_s_date_applies`, `scenario_media_metadata_utc_video_creation_time_made_local`, `scenario_media_metadata_exif_wall_clock_never_shifted`
- [x] 2.5 Existing date tests keep their meaning: `scenario_media_metadata_fallback_to_file_time_flagged` and `scenario_media_metadata_datetimeoriginal_preferred` run with an injected `FixedZone(0)` (their dates are given in UTC); verify `cargo test -p ps-media` is green
- [x] 2.6 `EmbeddedDates::from_stored(metadata)` rebuilds the inputs from stored metadata JSON (embedded time, provenance, `OffsetTime*` tags, video creation); verify a property test that for 1,000 generated files `resolve_capture_date(from_stored(extract(..)))` equals the analysis result

## 3. Catalog

- [x] 3.1 Migration 17 `0017_capture_dates.sql` (`capture_precision`, `date_version`, the UPDATE marking embedded-date rows current) registered in `CATALOG_MIGRATIONS`; verify a migration test upgrading a version-16 fixture: EXIF rows at version 2, file-mtime and +00:00 container rows at version 1
- [x] 3.2 `FileRecord` gains offset, provenance, precision and date version; the upsert writes precision and `DATE_LOGIC_VERSION`; verify `cargo test -p ps-catalog` and a round-trip test of a month-precision row
- [x] 3.3 `Readers::date_facts` and `Command::Redate` (five date columns + version, batched like `Reclassify`); verify a writer test that a redate of 10,000 rows commits once and leaves other columns untouched
- [x] 3.4 Drop `capture_ts` from `Readers::dedup_stamps`; verify a test that a `capture_ts`-only change produces no dedup work and identical groups and keepers

## 4. Scanning

- [x] 4.1 Pipeline dates new and changed files with the run's zone (`OsZone` created once in `Scanner::run`, overridable in `ScanOptions` for tests); verify `scenario_media_metadata_folder_date_does_not_change_duplicate_groups` (corpus with stripped copies moved into dated folders: groups and keepers identical)
- [x] 4.2 Diff re-dating of stale unchanged rows, `DiffCounts::redated`, progress and summary line; verify `scenario_incremental_rescan_old_catalog_re_dated_on_rescan` (byte-counting layer reads 0 bytes) and `scenario_incremental_rescan_re_dating_happens_once`
- [x] 4.3 Cancel during the re-date pass leaves unwritten rows at version 1 and the next rescan finishes them; verify an integration test with a cancel after the first chunk
- [x] 4.4 Measure the re-date pass on a synthetic 1M-row catalog (30% file-mtime rows) and the no-change rescan budget; verify `cargo xtask bench --check` passes `rescan-nochange-100k` and record the measured 1M re-date time here (estimate: 5-15 s, ADR-0071). Measured (release build, dev machine, `cargo test --release -p ps-scan --test redate_scale -- --ignored`): 300,000 stale rows of 1,000,000 re-dated in 14.9 s (Los Angeles zone, every date changed), plus 3.2 s listing the rows (the diff's page reads); `rescan-nochange-100k` 0.43 s (budget 4 s)
- [x] 4.5 Opt-in analysis refresh: `DiffCounts::needs_refresh`, `ScanOptions::refresh_analysis`, the refresh stage reading at most `PROBE_LIMIT` bytes read-only, then re-classification as in ADR-0051; verify `scenario_incremental_rescan_files_without_facts_counted_not_read` and `scenario_incremental_rescan_refresh_re_classifies_old_rows` (byte-counting layer: at most 256 KB per refreshed file, nothing for others)

## 5. Export

- [x] 5.1 `ps_service::export::embedded_of` and the inspector read the catalog date columns; verify existing `cargo test -p ps-service export` stays green and a test that a re-dated row (columns changed, JSON not) files by the columns
- [x] 5.2 `date::resolve` step 5 (folder dates, keeper then copies), `DateSource::FolderName`, precision in `FilingDate`; verify `scenario_export_embedded_date_of_a_copy_beats_the_keeper_s_folder_date`, `scenario_export_folder_date_of_a_copy_files_a_stripped_keeper` and the existing `scenario_export_stripped_keeper_filed_by_group_capture_date`
- [x] 5.3 Layout: month folders and the local day for files without any catalog date; manifest `date_precision`; verify `scenario_export_month_precision_file_placed_in_a_month_folder`, `scenario_export_undated_day_is_the_local_day`, and that the existing `scenario_export_undated_file_placed_in_undated`, `scenario_export_dated_file_placed_by_capture_date` and `scenario_export_copy_named_keeper_gets_the_clean_name` still pass
- [x] 5.4 `PlanTotals::filed_under_earlier_rules` (ledger path folder vs today's folder); verify `scenario_export_re_export_after_date_rules_change_copies_nothing` (library tree snapshot identical before and after the second run) and `scenario_export_new_keeper_uses_the_current_rules_beside_old_copies`
- [x] 5.5 `OtherCatalogLibrary` plan warning (manifest present, no ledger rows), CLI text and `--json`, Export view note; verify `scenario_export_unknown_library_warned_in_the_plan` (CLI test with two catalogs over one library; existing files' bytes and mtimes unchanged)

## 6. CLI, UI and docs

- [x] 6.1 CLI: `scan --refresh-analysis`; `files --json`/`show` and `export plan --json` carry `precision` and the folder-name provenance (the CLI has no `files` command: the per-file date, its precision and provenance are in `export dry-run --json`, fields `date`, `precision`, `date_source`); verify `cargo test -p photosquirrel-cli` and `scenario_media_metadata_month_folder_stays_a_month` (CLI JSON half)
- [x] 6.2 Inspector shows precision ("July 2019", "4 July 2019", full time) with provenance; verify the headless `#[gpui_kit::test]` `scenario_photo_inspector_month_precision_date_shown_as_a_month`
- [x] 6.3 Scan summary shows re-dated count and "N files need an analysis refresh" with a Refresh analysis action that rescans those sources with the option; verify a headless UI test that the action starts a run with `refresh_analysis` set, and `cargo xtask tokens`
- [x] 6.4 README (dates section: folder dates, precision, local days, existing libraries keep their folders, other-catalog warning) and `docs/performance.md` (re-date cost); verify the README commands run as written on the small corpus

## 7. Verification

- [x] 7.1 Corpus gate unchanged: `photosquirrel-cli eval-corpus` reports precision >= 0.995, recall >= 0.97, keeper accuracy >= 0.99 and zero must-not-merge violations, with the same numbers as before this change. Medium corpus, seed 42 (9,933 files), before (eaa98ea) and after: precision 1.0000, recall 1.0000, keeper accuracy 1.0000, 0 must-not-merge violations, 19,381 true pairs = result pairs = correct, 1,843 keeper groups; `groups list --json` totals identical (1,843 groups, 8,594 files, 4,048,886,433 reclaimable bytes, 5,570 suggestions)
- [x] 7.2 Old-catalog round trip on the medium corpus: scan and export with the previous release's catalog (version 15), upgrade, rescan, re-export; verify 0 files copied, library tree snapshot identical, `redated` > 0 reported. Run with the build just before this change (catalog version 16, the latest before 17): 1,972 files exported; upgraded to 17, the rescan read 0 bytes and reported 1,928 capture dates updated; the re-export copied and adopted 0 files, the 3,863-file library tree (paths, sizes, mtimes) was identical, the plan reported 16 copies filed under earlier date rules, and the gate on the upgraded catalog was unchanged
- [x] 7.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 7.4 `cargo xtask ci` is green: clean worktree at 1a8fdb8, 454 of 454 scenarios mapped, "CI passed."
- [x] 7.5 `openspec validate capture-dates --strict` passes

## 8. Gate review fixes

- [x] 8.1 An open or read error of an analysis-refresh file leaves its row (stage, analysis, group) as it was: a recent error, no failure row; verify `scenario_incremental_rescan_unreadable_file_keeps_its_analysis` (ps-service: failing `SourceFs` on the keeper, stage unchanged, the group survives, the export plan copies nothing new)
- [x] 8.2 Duplicate review cards show the catalog date columns at their precision (`CaptureDate::display`); verify `review_cards_show_the_catalog_date_at_its_precision` (ps-service: "July 2019", "4 July 2019", a re-dated row shows its columns)
- [x] 8.3 The re-date pass parks at a pause between chunks; a refresh run remembers its refresh (migration 18, ADR-0082) so a resume refreshes too; verify `pause_parks_the_re_date_pass_between_chunks`, `resumed_refresh_run_still_refreshes` (ps-scan) and `migration_18_records_the_refresh_of_a_run` (ps-catalog)
- [x] 8.4 Scan summaries count the files refreshed this run (`DiffCounts::refreshed`) and only the files still waiting as needing a refresh (CLI and scan dashboard); verify `scan_refresh_analysis_flag_refreshes_rows_without_class_facts` (CLI) and the overview counts in `scenario_incremental_rescan_unreadable_file_keeps_its_analysis`
- [x] 8.5 School-year and season folders (`2011-12 School Year`, `2011-2012`) are not month dates (ADR-0080); verify `scenario_media_metadata_school_year_folder_is_not_a_month`
- [x] 8.6 A keyed QuickTime creation date (`com.apple.quicktime.creationdate`) keeps its wall clock and offset, also at +00:00; only `mvhd` times are UTC instants (ADR-0081); verify `scenario_media_metadata_keyed_quicktime_date_never_shifted`
- [x] 8.7 `Readers::date_facts` deserialises only the date fields and date tags of the metadata JSON (`ps_media::date::STORED_DATE_TAGS`, strings borrowed); verify `date_facts_rebuild_the_same_inputs_as_the_full_metadata` (ps-scan) and the 1M-row re-date measurement with 200 tags per row before and after. Measured (release, dev machine, 1,000,000 rows, 300,000 stale, 13 GB catalog): before, re-date pass 102.7 s (listing 74.9 s); after, 84.5-111.3 s over two runs (listing 77.5-80.6 s), of which reading the date facts is 17.0 s cold against 13.3 s warm for the whole JSON; with the rows in the page cache (200,000-row catalog, 60,000 stale) the slim read takes 1.59 s against 2.61 s. The pass is bound by SQLite rewriting each updated row with its large metadata column, not by JSON; SQLite-side `json_extract`/`json_each` was slower (130.8 s) and was not kept. No budget changes (the re-date pass is a measurement, ADR-0071); `docs/performance.md` updated
- [x] 8.8 Export filing dates from file names without a time and date-only XMP values have day precision; verify `file_name_and_date_only_xmp_dates_have_day_precision` (ps-export)
- [x] 8.9 README states the pre-1990 exception of the folder-date plausibility rule; `OsZone` logs its fallback to offset 0 once; verify the README text and `os_zone_fallback_is_offset_zero_and_logged` (ps-media)
- [ ] 8.10 Corpus gate unchanged after the fixes (medium preset, seed 42); `cargo xtask ci` green in a clean worktree; GitHub CI green
