# Tasks

Starts after `heic-system-decoders`, `excluded-view-filters` and `burst-frame-identity` are archived. The TIFF generation is 1, HEIF and AVIF are 1, and every other format is 0.

## 1. Decisions

- [x] 1.1 Number the ADR "Placeholders carry a reason and a decoder generation" (D1) from the next free number. Add it to `docs/adr/` as accepted, with an index row. Confirm that migration 24 is free (`heic-system-decoders` took 20 for `file.decode_failure`, ADR-0098; `excluded-view-filters` took 21 and 22, ADR-0102 and ADR-0104; `burst-frame-identity` took 23). Verify: the index lists it, and `ls crates/ps-catalog/migrations` has no 0024. Done: ADR-0109 (accepted, index row); migrations ended at 0023 before `0024_preview_gen.sql`.

## 2. Model and media

- [x] 2.1 `PlaceholderReason::{UnsupportedVariant, Unreadable, NeedsNewerVersion}`; `PlaceholderInfo.generation` with serde default; `GENERATIONS` for each format. Verify: `scenario_thumbnails_old_placeholders_read_as_an_older_generation`, and the generation constants test. Done: `ps_model::preview` (`PreviewReason`, `DecodeFamily`, `Generations`, `PreviewStamp`), `ps_media::decoders::GENERATIONS` (TIFF 1, HEIF 1, AVIF 1) and `decoder_generations_of_this_build`; `NeedsNewerVersion` is `#[serde(other)]`.
- [x] 2.2 Map decode errors to reasons (a `DecodeError::Unsupported` detail from `tiff_decode` and the JPEG decoder). Verify: `scenario_thumbnails_bigtiff_placeholder_names_the_variant` and `scenario_thumbnails_damaged_jpeg_placeholder_keeps_the_error` (ps-fixtures writers, in temp dirs). Done: `thumb::reason_for_error`; the probe recognises BigTIFF (first directory's dimensions) and the decoder refuses it with `Unsupported("BigTIFF")`; new writer `tiff_image::bigtiff_rgb`; tests in `ps-media/tests/placeholders.rs`.

## 3. Catalog

- [x] 3.1 Migration 24: `file.preview_gen` and `preview_reason`, the partial index, and the backfill rule from D1 (record the exact rule in the ADR). Verify: a migration test on a catalog fixture written by migration 23 that has placeholder rows. Done: `migration_24_marks_placeholder_stills_of_older_catalogs_at_generation_0` (`ps-catalog/tests/previews.rs`); the rule is in ADR-0109; also `scan_run.kind` and `fill_scope`.
- [x] 3.2 Write and clear both columns in the analysis statement. Add the `fillable(registry, generations)` predicate and the counts by format and reason. Verify: unit tests for each row of the waiting rule, including that current-generation `unreadable` rows are not fillable. Done: `ps_catalog::WaitingRule` (SQL and Rust), `Readers::preview_status`, `fillable_files`; `the_waiting_rule_row_by_row`.

## 4. Fill in job

- [x] 4.1 `previews::fill(scope)` as a targeted `scan_run(kind='fill')` on the analysis lanes, with sampled progress and pause, resume and cancel. Verify: `scenario_incremental_rescan_fill_in_reads_only_waiting_files` (byte-counting layer, no directory listing), `scenario_incremental_rescan_filled_in_heic_joins_its_jpeg_group` (fake HEIF), `scenario_incremental_rescan_offline_files_are_skipped_and_counted`, `scenario_incremental_rescan_fill_in_can_be_cancelled`. Done: `Scanner::start_fill`, `ScanService::fill_previews`, `Library::fill_previews`; tests in `ps-scan/tests/previews.rs`.
- [x] 4.2 Rescan diff: the generation rule for unchanged rows, beside `decoder_missing`. Verify: `scenario_incremental_rescan_rescan_after_an_update_fills_in_old_tiff_placeholders` (test-only generation override), `scenario_incremental_rescan_current_generation_failures_are_not_retried`, and every existing `scenario_incremental_rescan_*` test still passes. Done: `diff::outdated_placeholder` (`DiffCounts::outdated_previews`); `cargo test -p ps-scan` green.

## 5. CLI

- [x] 5.1 `previews status` and `previews fill`, with `--json` and Ctrl-C exit 130. Verify: `scenario_cli_status_counts_by_reason` and `scenario_cli_fill_one_format`. Done: `cmd/previews.rs` (`--format`, `--reason`, `--source`), the job runs in the scan's foreground loop (same Ctrl-C, control requests and pause on battery; NDJSON `previews fill` snapshot and summary lines); tests in `photosquirrel-cli/tests/previews.rs`.

## 6. UI

- [x] 6.1 `ui-gallery` placeholder tile paint with levels of detail, the summary badge, and tokens (`placeholder_surface`, `tile_lod_small`, `tile_lod_tiny`). Use it in the grid, the loupe, the Excluded list and the inspector. Verify: `scenario_gallery_tiff_without_a_preview_shows_a_placeholder_tile`, `scenario_gallery_small_tiles_keep_glyph_and_badge`, `scenario_gallery_loupe_shows_the_large_placeholder`, and `cargo xtask tokens`. Done: `ui_gallery::placeholder` (`layout`, `Layers`, `paint`, `placeholder_element`), `Colors::placeholder_surface`/`placeholder_stroke`, `Tokens::TILE_LOD_SMALL`/`TILE_LOD_TINY`; grid, loupe (and filmstrip), inspector header and Excluded list; tests in `ui-gallery/tests/placeholders.rs`; tokens OK.
- [x] 6.2 Info hit region, `GridEvent::PlaceholderInfo`, the `I` shortcut and the help overlay entry. Verify: `scenario_gallery_info_button_opens_the_dialog`. Done: `GalleryEvent::PlaceholderInfo`, `LoupeEvent::PlaceholderInfo`; the shortcut is Shift+I (I already toggles the inspector) with the palette command "Why No Preview?"; test in `ui-workspace/tests/previews.rs`.
- [x] 6.3 `previews::explain` and the "Why no preview?" dialog for each reason; the inspector link replaces the HEIC disclosure. Verify: `scenario_photo_inspector_missing_decoder_shows_install_steps`, `scenario_photo_inspector_decoder_available_offers_fill_in_previews`, `scenario_photo_inspector_unsupported_variant_explains_the_limit`, `scenario_photo_inspector_damaged_file_shows_the_error`, `scenario_photo_inspector_show_files_with_the_same_reason`, and that the `heic-system-decoders` inspector scenarios still pass. Done: `Library::explain`, `views/preview_help.rs` (a shell overlay); the inspector keeps its HEIC note and adds "Why no preview?" for every reason; `cargo test -p ui-workspace` green.
- [x] 6.4 "No preview" Library filter by reason and format. Verify: `scenario_gallery_filter_to_heic_files_waiting_for_a_decoder`. Done: `Filter::no_preview` (partial index `file_preview`), `FilterBar::no_preview`, the Preview menu and its chip, `--gallery-filter no-preview=<reason>[:<format>]`.
- [x] 6.5 Library notice and the scan-dashboard row for the fill job. Verify: `scenario_gallery_notice_after_an_update_that_adds_tiff_variants`. Done: `LibraryView::fill_notice` (read at start-up and when a run ends, dismissed for the session), `scan_dashboard::run_title` ("Filling in previews: 12 of 37"), the status bar says "Filling in previews".

## 7. Budgets and docs

- [ ] 7.1 Gallery scroll bench case with 30% placeholders. Verify: `cargo xtask bench --check`, with the `gallery-scroll` p95 within budget. Not yet verified: `--render-bench` (and so `gallery-scroll-p95`) gets no frames from an agent-launched window on the dev machine ("frames stalled", see `docs/performance.md`), so it must be measured from an unlocked desktop session. Done so far: `--bench-placeholders` and the budget's new args (ADR-0110); the headless proxy (`render_cost`, release) p95 0.143 ms plain and 0.161 ms with 30% placeholders; `catalog-id-list-1m` 126 ms, `excluded-view-open` 106 ms, `scan-analysis-12mp` 4.47 ms, `rescan-nochange-100k` 0.43 s, `first-gallery-frame` 499 ms, all within budget.
- [x] 7.2 README "Photos without a preview" section; `docs/` note on bumping decoder generations. Verify: `grep -n "Fill in previews" README.md`. Done: README "Photos without a preview" (and the HEIC, TIFF-limits and command-line sections), `docs/decoder-generations.md`, and the 0.3.0 release-notes line.

## 8. Verification

- [ ] 8.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [x] 8.2 Corpus gate unchanged (the numbers from `tiff-support`). Done: medium preset seed 42, release CLI: precision, recall and keeper accuracy 1.0000, 0 violations; 19,741 pairs, 1,903 groups, 8,834 grouped files, 4,166,020,278 reclaimable bytes, 6,338 suggestions (unchanged).
- [x] 8.3 Format folder: with `PHOTOSQUIRREL_HEIF_DECODER=none`, the HEIC files show "needs a library" tiles and the dialog shows this system's steps. Then fill in with the system decoder (if one is installed): only the HEIC files are read, and each joins its JPEG. A catalog made by the 0.2.0 CLI on the TIFF variants, opened by this build, offers Fill in previews, and the job thumbs and groups them. Done: with `none` 4 HEICs show needs-a-library tiles and the dialog shows this system's steps; with the system libheif 1.23.3 `previews fill --format heic` filled 4, read only them; a catalog written by the 0.2.0 CLI (TIFF placeholder rows) opened by this build showed `tiff could not be read 1, can fill in now 1`, and `previews fill` filled it and grouped it with its resized JPEG (TIFF keeper).
- [x] 8.4 Screenshot review: the grid with mixed placeholders at three tile sizes, the loupe, the dialog for each reason, light and dark. Done: captures of the grid at 96, 156 and 300 px (dark and light), the loupe, and the dialog for a missing decoder, a damaged JPEG, a BigTIFF and a fillable TIFF; review fixes committed (inspector row, repeated sentence, lighter hatch in light).
- [ ] 8.5 Pushed; GitHub CI green on all four jobs.
