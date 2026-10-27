# Tasks

Ships in 0.3.x (ROADMAP row 30), after `stable-ids-on-fat`. Takes **ADR-0113** and **catalog migration 26**.

## 1. Decision

- [ ] 1.1 Write ADR-0113 "Previews are converted to sRGB; hashes are not" (D1-D4; amends 0016 and 0012; the direct `moxcms` dependency), accepted, with an index row. Confirm migration 26 is free. Verify: the index lists it; `ls crates/ps-catalog/migrations` ends at 0025.

## 2. Fixtures and detection

- [ ] 2.1 `ps-fixtures` colour writers (D6): R03 JPEG, Display P3 ICC JPEG, Canon colour-space CR2-style file, DNG with `PreviewColorSpace` 3, sRGB-profile JPEG. Verify: the generator stays deterministic for a seed; the corpus presets do not use them (corpus unchanged).
- [ ] 2.2 `ps_model::ColourSpace`, `ProbeRecord.colour` (serde default), and detection in `probe.rs`, `jpeg.rs`, `frame.rs` and the DNG preview IFD, in D1's order, inside the existing probe reads. Verify: detection unit tests per source and order; `scenario_media_probe_*` and the byte-counting probe tests unchanged (no extra read).

## 3. Conversion

- [ ] 3.1 `moxcms` as a direct `ps-media` dependency at the version `image` uses; `colour::to_srgb` with cached transforms; `thumbnail_from_image`, `make_thumbnail`, `analyze_still_held` and `make_large_preview` pass the stream's colour. Verify: `scenario_thumbnails_canon_adobe_rgb_jpeg_thumbnail_is_converted`, `scenario_thumbnails_raw_preview_in_adobe_rgb_is_converted`, `scenario_thumbnails_embedded_profile_is_applied`, `scenario_thumbnails_srgb_thumbnails_are_unchanged` (golden thumbnails written by the build before this change).
- [ ] 3.2 Hashes from the unconverted image (a test hook disables conversion). Verify: `scenario_thumbnails_hashes_unchanged_by_conversion`.

## 4. Catalog and rescan

- [ ] 4.1 Migration 26 `0026_colour.sql` with D4's backfill; the analysis statement writes `colour`; the thumbnail replace command. Verify: a migration test on a migration-25 fixture catalog (`_MG_` rows NULL, others `unchecked`).
- [ ] 4.2 Rescan colour re-check in `diff.rs` and the pipeline; summary count. Verify: `scenario_thumbnails_rescan_recolours_old_adobe_rgb_thumbnails_only`, `scenario_thumbnails_the_re_check_happens_once`, and every existing `scenario_incremental_rescan_*` test.

## 5. Service and UI

- [ ] 5.1 Large preview: conversion and the new cache key (D4). Verify: a ps-service test that a preview cached under the old key is not returned.
- [ ] 5.2 Inspector "Colour" row. Verify: `scenario_photo_inspector_adobe_rgb_photo_names_its_colour_space`, `scenario_photo_inspector_srgb_photo_shows_no_colour_row`; `cargo xtask tokens`.

## 6. Budgets and docs

- [ ] 6.1 Bench: an Adobe RGB 12 MP case in `analysis-decode-thumb`. Verify: `cargo xtask bench --check`, with `analysis-decode-thumb`, `scan-analysis-12mp` and `gallery-scroll-p95` within budget and within 15% of baseline.
- [ ] 6.2 README: remove the Adobe RGB limitation; say HEIC and TIFF profiles are not applied yet. The 0.3.x release-notes line. Verify: `grep -n 'Adobe RGB' README.md`.

## 7. Verification

- [ ] 7.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [ ] 7.2 Corpus gate unchanged: medium preset seed 42, precision, recall and keeper accuracy 1.0000, 0 violations, and the same pair, group and reclaimable-byte counts as `preview-placeholders`.
- [ ] 7.3 Owner check (read-only, owner-run; screenshots of the card only with owner approval): the `_MG_` JPEGs and CR2 previews look like their `IMG_` neighbours in the grid and the loupe; a rescan of an older catalog of the card recolours only the `_MG_` files.
- [ ] 7.4 Screenshot review (fixtures): grid and loupe with Adobe RGB, Display P3 and sRGB patches side by side, light and dark.
- [ ] 7.5 Pushed; GitHub CI green on all five jobs.
- [ ] 7.6 Gate review by a fresh agent that did not write the code.
