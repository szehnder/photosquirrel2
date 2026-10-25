# Tasks

Starts after `heic-system-decoders` and `excluded-view-filters` are archived. The TIFF generation is 1, HEIF and AVIF are 1, and every other format is 0.

## 1. Decisions

- [ ] 1.1 Number the ADR "Placeholders carry a reason and a decoder generation" (D1) from the next free number. Add it to `docs/adr/` as accepted, with an index row. Confirm that migration 21 is free. Verify: the index lists it, and `ls crates/ps-catalog/migrations` has no 0021.

## 2. Model and media

- [ ] 2.1 `PlaceholderReason::{UnsupportedVariant, Unreadable, NeedsNewerVersion}`; `PlaceholderInfo.generation` with serde default; `GENERATIONS` for each format. Verify: `scenario_thumbnails_old_placeholders_read_as_an_older_generation`, and the generation constants test.
- [ ] 2.2 Map decode errors to reasons (a `DecodeError::Unsupported` detail from `tiff_decode` and the JPEG decoder). Verify: `scenario_thumbnails_bigtiff_placeholder_names_the_variant` and `scenario_thumbnails_damaged_jpeg_placeholder_keeps_the_error` (ps-fixtures writers, in temp dirs).

## 3. Catalog

- [ ] 3.1 Migration 21: `file.preview_gen` and `preview_reason`, the partial index, and the backfill rule from D1 (record the exact rule in the ADR). Verify: a migration test on a catalog fixture written by migration 20 that has placeholder rows.
- [ ] 3.2 Write and clear both columns in the analysis statement. Add the `fillable(registry, generations)` predicate and the counts by format and reason. Verify: unit tests for each row of the waiting rule, including that current-generation `unreadable` rows are not fillable.

## 4. Fill in job

- [ ] 4.1 `previews::fill(scope)` as a targeted `scan_run(kind='fill')` on the analysis lanes, with sampled progress and pause, resume and cancel. Verify: `scenario_incremental_rescan_fill_in_reads_only_waiting_files` (byte-counting layer, no directory listing), `scenario_incremental_rescan_filled_in_heic_joins_its_jpeg_group` (fake HEIF), `scenario_incremental_rescan_offline_files_are_skipped_and_counted`, `scenario_incremental_rescan_fill_in_can_be_cancelled`.
- [ ] 4.2 Rescan diff: the generation rule for unchanged rows, beside `decoder_missing`. Verify: `scenario_incremental_rescan_rescan_after_an_update_fills_in_old_tiff_placeholders` (test-only generation override), `scenario_incremental_rescan_current_generation_failures_are_not_retried`, and every existing `scenario_incremental_rescan_*` test still passes.

## 5. CLI

- [ ] 5.1 `previews status` and `previews fill`, with `--json` and Ctrl-C exit 130. Verify: `scenario_cli_status_counts_by_reason` and `scenario_cli_fill_one_format`.

## 6. UI

- [ ] 6.1 `ui-gallery` placeholder tile paint with levels of detail, the summary badge, and tokens (`placeholder_surface`, `tile_lod_small`, `tile_lod_tiny`). Use it in the grid, the loupe, the Excluded list and the inspector. Verify: `scenario_gallery_tiff_without_a_preview_shows_a_placeholder_tile`, `scenario_gallery_small_tiles_keep_glyph_and_badge`, `scenario_gallery_loupe_shows_the_large_placeholder`, and `cargo xtask tokens`.
- [ ] 6.2 Info hit region, `GridEvent::PlaceholderInfo`, the `I` shortcut and the help overlay entry. Verify: `scenario_gallery_info_button_opens_the_dialog`.
- [ ] 6.3 `previews::explain` and the "Why no preview?" dialog for each reason; the inspector link replaces the HEIC disclosure. Verify: `scenario_photo_inspector_missing_decoder_shows_install_steps`, `scenario_photo_inspector_decoder_available_offers_fill_in_previews`, `scenario_photo_inspector_unsupported_variant_explains_the_limit`, `scenario_photo_inspector_damaged_file_shows_the_error`, `scenario_photo_inspector_show_files_with_the_same_reason`, and that the `heic-system-decoders` inspector scenarios still pass.
- [ ] 6.4 "No preview" Library filter by reason and format. Verify: `scenario_gallery_filter_to_heic_files_waiting_for_a_decoder`.
- [ ] 6.5 Library notice and the scan-dashboard row for the fill job. Verify: `scenario_gallery_notice_after_an_update_that_adds_tiff_variants`.

## 7. Budgets and docs

- [ ] 7.1 Gallery scroll bench case with 30% placeholders. Verify: `cargo xtask bench --check`, with the `gallery-scroll` p95 within budget.
- [ ] 7.2 README "Photos without a preview" section; `docs/` note on bumping decoder generations. Verify: `grep -n "Fill in previews" README.md`.

## 8. Verification

- [ ] 8.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [ ] 8.2 Corpus gate unchanged (the numbers from `tiff-support`).
- [ ] 8.3 Format folder: with `PHOTOSQUIRREL_HEIF_DECODER=none`, the HEIC files show "needs a library" tiles and the dialog shows this system's steps. Then fill in with the system decoder (if one is installed): only the HEIC files are read, and each joins its JPEG. A catalog made by the 0.2.0 CLI on the TIFF variants, opened by this build, offers Fill in previews, and the job thumbs and groups them.
- [ ] 8.4 Screenshot review: the grid with mixed placeholders at three tile sizes, the loupe, the dialog for each reason, light and dark.
- [ ] 8.5 Pushed; GitHub CI green on all four jobs.
