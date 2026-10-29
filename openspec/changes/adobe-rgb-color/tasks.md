# Tasks

Ships in 0.3.x (ROADMAP row 31), after `stable-ids-on-fat`. Takes **ADR-0113** and **catalog migration 26**.

## 1. Decision

- [x] 1.1 Write ADR-0113 "Previews are converted to sRGB; hashes are not" (D1-D4; amends 0016 and 0012; the direct `moxcms` dependency), accepted, with an index row. Confirm migration 26 is free. Verify: the index lists it; `ls crates/ps-catalog/migrations` ends at 0025.
  Done: ADR-0113 accepted with an index row (0113 and 0114 were free; 0114 stays `video-posters-after-ffmpeg`'s); `ls crates/ps-catalog/migrations` ended at 0025_video_placeholders.sql.

## 2. Fixtures and detection

- [x] 2.1 `ps-fixtures` colour writers (D6): R03 JPEG, Display P3 ICC JPEG, Canon colour-space CR2-style file, DNG with `PreviewColorSpace` 3, sRGB-profile JPEG. Verify: the generator stays deterministic for a seed; the corpus presets do not use them (corpus unchanged).
  Done: `ps_fixtures::colour` (`adobe_rgb_jpeg`, `exif_jpeg`, `icc_jpeg`/`with_icc` with `display_p3_icc`, `srgb_icc`, `adobe_rgb_icc`, `pro_photo_icc`, `canon_cr2`/`canon_adobe_rgb_cr2`, `dng_preview_colour`); `writers_are_deterministic` passes; no corpus module calls them (the corpus gate in 7.2 is the end check).
- [x] 2.2 `ps_model::ColourSpace`, `ProbeRecord.colour` (serde default), and detection in `probe.rs`, `jpeg.rs`, `frame.rs` and the DNG preview IFD, in D1's order, inside the existing probe reads. Verify: detection unit tests per source and order; `scenario_media_probe_*` and the byte-counting probe tests unchanged (no extra read).
  Done: `colour::tests::detection_order`, `detection_by_source_and_order`, `dng_preview_colour_space_decides`, `malformed_profile_is_ignored_with_a_warning`, `colour_detection_reads_no_more_than_the_probe_window`; every ps-media test binary green (media_probe 31, the counting-reader probe tests unchanged). CR3 (`CMT2`/`CMT3`) and RAF read the same tags.

## 3. Conversion

- [x] 3.1 `moxcms` as a direct `ps-media` dependency at the version `image` uses; `colour::to_srgb` with cached transforms; `thumbnail_from_image`, `make_thumbnail`, `analyze_still_held` and `make_large_preview` pass the stream's colour. Verify: `scenario_thumbnails_canon_adobe_rgb_jpeg_thumbnail_is_converted`, `scenario_thumbnails_raw_preview_in_adobe_rgb_is_converted`, `scenario_thumbnails_embedded_profile_is_applied`, `scenario_thumbnails_srgb_thumbnails_are_unchanged` (golden thumbnails written by the build before this change).
  Done: `moxcms` 0.8 (0.8.1 in the lock, the version `image` 0.25 uses) is a direct dependency of `ps-media` (and `ps-fixtures`, test profiles). `colour::to_srgb` runs after the resize in `thumbnail_from_image` and `make_large_preview`; `make_thumbnail` and `analyze_still_held` take the stream's colour from the probe (`ColourSource::of`), so their signatures did not change (see Deviations). The four scenarios pass in `ps-media/tests/colour.rs` and `colour_golden.rs` (8 golden thumbnails written by commit 2212ccf's ps-media before any change, compared byte for byte). Colour check: Adobe RGB patch decoded [40, 170, 61], shown before as is (ΔE76 8.11 from the reference conversion), now [0, 172, 46] against the reference [0.0, 171.5, 46.0] (ΔE 0.26); Display P3 ΔE 11.38 before, 0.40 after.
- [x] 3.2 Hashes from the unconverted image (a test hook disables conversion). Verify: `scenario_thumbnails_hashes_unchanged_by_conversion`.
  Done: `ps_model::set_colour_conversion_enabled` (feature `test-hooks`); `ps-scan/tests/colour.rs` scans the same five files with the conversion on and off: identities and perceptual hashes equal, Adobe RGB thumbnails differ, sRGB ones identical.

## 4. Catalog and rescan

- [x] 4.1 Migration 26 `0026_colour.sql` with D4's backfill; the analysis statement writes `colour`; the thumbnail replace command. Verify: a migration test on a migration-25 fixture catalog (`_MG_` rows NULL, others `unchecked`).
  Done: `0026_colour.sql` (only thumbed `_` JPEG/RAW rows with a real thumbnail stay NULL), `AnalysisResult.colour` written by both analysis statements, `FileRecord.colour`, `file.colour` cleared with the other analysis columns on a change, and `Command::SetColour` (every row of the content). The thumbnail itself is replaced through the thumbnail store, as ADR-0117's placeholder is (Deviations). `ps-catalog/tests/colour.rs`: the migration-25 catalog test (9 rows) and the writer test; the upgrade test in `open.rs` now expects 'unchecked'; all ps-catalog tests green.
- [x] 4.2 Rescan colour re-check in `diff.rs` and the pipeline; summary count. Verify: `scenario_thumbnails_rescan_recolours_old_adobe_rgb_thumbnails_only`, `scenario_thumbnails_the_re_check_happens_once`, and every existing `scenario_incremental_rescan_*` test.
  Done: `diff::needs_colour_check` (NULL colour, real thumbnail, JPEG or RAW, name starting with `_`) queues the row on the ADR-0105/0117 header re-read (`Todo.colour_check`; one open, the probe window when both are due, each stamp written on its own: `a_raw_due_both_checks_is_opened_once`); `DiffCounts.colour_checked` and `recoloured`, and a CLI summary line. Both scenarios pass (3 files opened of 5, the sRGB `_MG_` one read 262,144 bytes or less, the Adobe RGB ones read whole once, dedup stamps and hashes unchanged; the second rescan opens nothing). All 20 ps-scan test binaries green.

## 5. Service and UI

- [x] 5.1 Large preview: conversion and the new cache key (D4). Verify: a ps-service test that a preview cached under the old key is not returned.
  Done: `ps_service::library::preview_cache_key` = hex(blake3(identity hex ‖ ":srgb-1")) (64 hex characters); `blake3` moved from dev- to normal dependency of ps-service (already in the workspace). `ps-service/tests/colour.rs` `a_large_preview_cached_under_the_old_key_is_not_returned`: a JPEG cached under the identity hex is not served, the new preview equals `make_large_preview` (converted) and is cached under the new key; all ps-service test binaries green.
- [x] 5.2 Inspector "Colour" row. Verify: `scenario_photo_inspector_adobe_rgb_photo_names_its_colour_space`, `scenario_photo_inspector_srgb_photo_shows_no_colour_row`; `cargo xtask tokens`.
  Done: `ItemDetail.colour_space` from `file.colour` (not the metadata JSON: Deviations), the "Colour" row (`inspector-colour`) after Dimensions only when it is set. Both scenarios pass in `ui-workspace/tests/colour.rs` (the loupe's preview from the backend is converted: centre [0..8, 168..176, 40..52]); `cargo xtask tokens` OK, `cargo xtask layering` OK (17 crates).

## 6. Budgets and docs

- [x] 6.1 Bench: an Adobe RGB 12 MP case in `analysis-decode-thumb`. Verify: `cargo xtask bench --check`, with `analysis-decode-thumb`, `scan-analysis-12mp` and `gallery-scroll-p95` within budget and within 15% of baseline.
  Done: a 12 MP Canon Adobe RGB JPEG is the fourth file of `scenario_thumbnails_decode_within_per_file_budget` (its own median 35.0 ms). `cargo xtask bench --check --only analysis-decode-thumb --only scan-analysis-12mp --only rescan-nochange-100k`: 35.39 ms (budget 100, baseline 37.46), 4.41 ms (6.67, baseline 4.833), 0.41 s (4, baseline 0.408), all 3 met and within 15% of baseline. `gallery-scroll-p95` needs a visible focused window and is left to the owner (the grid paints stored JPEGs; nothing on its path changed).
- [x] 6.2 README: remove the Adobe RGB limitation; say HEIC and TIFF profiles are not applied yet. The 0.3.x release-notes line. Verify: `grep -n 'Adobe RGB' README.md`.
  Done: the known-limitations bullet now says HEIC, AVIF, TIFF, PNG and WebP profiles are not applied yet, that Adobe RGB JPEGs, RAW previews and profiled JPEGs are converted, and that renamed Adobe RGB files of an old catalog wait for their next analysis; `grep -n 'Adobe RGB' README.md` shows only that bullet (lines 130, 132). Release notes: a "What's new since 0.3.0" entry.

## 7. Verification

- [ ] 7.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [x] 7.2 Corpus gate unchanged: medium preset seed 42, precision, recall and keeper accuracy 1.0000, 0 violations, and the same pair, group and reclaimable-byte counts as `preview-placeholders`.
  Done: release CLI, medium preset seed 42, drive-a/b/c: 10,173 files (5.8 GB) in 17.2 s, 0 errors; `eval-corpus` precision, recall, keeper accuracy 1.0000, 0 must-not-merge violations, 19,741/19,741 pairs, 1,903 keeper groups, gate PASS; `groups list`: 8,834 grouped files, 1,903 groups, 4,166,020,278 reclaimable bytes, 6,338 suggestions (the numbers of `damaged-raw-reason`); every analysed row recorded `srgb` (8,881; the 1,292 others not analysed); an unchanged rescan read 0 B. Corpus removed.
- [ ] 7.3 Owner check (read-only, owner-run; screenshots of the card only with owner approval): the `_MG_` JPEGs and CR2 previews look like their `IMG_` neighbours in the grid and the loupe; a rescan of an older catalog of the card recolours only the `_MG_` files.
- [x] 7.4 Screenshot review (fixtures): grid and loupe with Adobe RGB, Display P3 and sRGB patches side by side, light and dark.
  Done: a fixture card (Canon Adobe RGB patch JPEG, Adobe RGB photo, Adobe RGB CR2, a Display P3-profile JPEG, the same green patch untagged sRGB, the reference conversions [0, 172, 46] and [0, 173, 38] as sRGB JPEGs, the photo in sRGB) scanned with the release CLI; `cargo xtask screenshot` grid dark and light, loupe dark (`_MG_0001`) and light (`_MG_0002`), inspector of the P3 and the sRGB file (`--gallery-filter no-stacks --gallery-select N`). Pixels sampled from the PNGs: the Adobe RGB JPEG and CR2 tiles and the loupe (0, 172, 46), identical to the reference tile; the Display P3 tile (0, 173, 39) beside its reference (0, 173, 39); the untagged sRGB green (40, 170, 61), the old muted look. The inspector shows "Colour Adobe RGB (1998)" and "Colour Display P3", and no Colour row for the sRGB file. PNGs in target/agent-review/screenshots/adobe/ (not committed).
- [ ] 7.5 Pushed; GitHub CI green on all five jobs.
- [ ] 7.6 Gate review by a fresh agent that did not write the code.
