# Tasks

Ships in 0.3.x (ROADMAP row 33, priority 1). **ADR-0115**, **no migration** (`file.preview_reason` and the
`file_preview` index are migration 24's; new summary and report fields are serde-default JSON). Independent of
`adobe-rgb-color` (migration 26).

## 0. Decision record

- [x] 0.1 Write ADR-0115 "A RAW file cut short is damaged; damaged files are counted and flagged" (D1-D7; amends 0109: a generation is also bumped when a change corrects a family's placeholder reason, RAW decode outcomes; amends 0023: the report's damaged count, dispositions unchanged), accepted, with an index row. Verify: `grep -n "0115" docs/adr/README.md`. Done: `docs/adr/0115-a-raw-file-cut-short-is-damaged-and-damaged-files-are-counted.md`, accepted; index row 119 (also amends 0024 for D10).

## 1. Truncation evidence (`ps-media`)

- [x] 1.1 `ps-fixtures`: `dng::cut(bytes, Cut::InPreview | Cut::AfterPreview)` (D9); `AfterPreview` keeps the whole preview JPEG and cuts the sensor strip in half. Verify: a `ps-fixtures` unit test asserts the preview range lies inside the `AfterPreview` cut and past the `InPreview` cut for `cr2_like` and `cr2_unknown_lens`. Done: `dng::cut` returns `Option` (`None` for `AfterPreview` of `cr2_unknown_lens`, which has no sensor strip) and `InPreview` cuts halfway through the preview (Deviations); `dng::cut_tests::cuts_keep_or_cut_the_preview` passes (1 test).
- [x] 1.2 `ps_media::raw_check::truncation(bytes, probe)` (D1): TIFF-based RAWs (IFD chain, SubIFDs, EXIF pointer; strips, tiles, JPEGInterchangeFormat; never maker notes or other tag values), CR3 top-level boxes, RAF header ranges; judged only when the buffer is the whole file. Verify: unit tests: both CR2 cuts found with the right `needed`; hand-built CR3 (`mdat` past the end) and RAF (CFA past the end) found; a `size = 0` CR3 box and a partial buffer not judged. Done: `raw_check::tests` 5/5 (both CR2 cuts with `needed` = the sensor strip's end, the unknown-lens preview end, CR3 `mdat` 1,044 > 444, RAF CFA 1,400 > 900, `size = 0`, partial buffer, message units).
- [ ] 1.3 No false positives. Verify: `scenario_thumbnails_complete_raw_fixtures_are_never_damaged` (every complete RAW writer in `ps-fixtures`, `cr2_unknown_lens` with its 0xC5D8 tag included); and `truncation` over the read-only sample photos folder (a local, ignored test run by hand; record the file count and 0 found here).

## 2. Outcomes of `decode_raw_preview`

- [ ] 2.1 The outcomes in D2 order: truncation -> `Failed`; damaged in-range preview with no rawler image -> `Failed`; rawler `Unsupported` -> `Unsupported` naming make and model; `Ok(None)` -> `NoRawPreview`; other rawler error or panic -> `Unsupported` with rawler's message. The `contain` closure returns an outcome enum. Verify: one `ps-media` unit test per outcome; `scenario_thumbnails_raw_cut_inside_its_preview_is_damaged`, `scenario_thumbnails_raw_cut_after_an_intact_preview_is_damaged` (no pixels, no hashes), `scenario_thumbnails_unknown_camera_is_still_not_supported`.
- [x] 2.2 `GENERATIONS` RAW 1, the `decoder_generations_of_this_build` test, `docs/decoder-generations.md` (the RAW row and "a change that corrects a family's placeholder reason" under When to bump). Verify: `cargo test -p ps-media generation`. Done: RAW 1 in `GENERATIONS` and the test; docs row and the new "corrects a family's placeholder reason" bullet; `cargo test -p ps-media` all green (lib 38, every test binary).

## 3. Pipeline (`ps-scan`)

- [ ] 3.1 One RAW arm in the analysis (D3) replacing `Err(e @ DecodeError::NoRawPreview)`: a placeholder for every RAW decode error, `progress.error` only for `Failed`, never `Persist::Failed`. Verify: the cut CR2 scan of `photosquirrel-cli/tests/decoder_logs.rs` (4.1) has a row with identity at stage thumbed; a `ps-scan` test: a cut CR2 without probe dimensions is catalogued, not failed; the cut CR2 and its camera JPEG (same stem, same time) are in different groups and both are keepers.
- [ ] 3.2 `Redecode::Undecodable` reports an error for a RAW only when the new reason is `Unreadable` (D5); HEIC and AVIF unchanged. Verify: `scenario_incremental_rescan_rescan_re_classifies_an_old_damaged_raw`, `scenario_incremental_rescan_re_read_unknown_camera_is_not_an_error` (old-catalog fixtures with generation-0 RAW placeholders written through `ScanOptions::generations` at RAW 0); the existing HEIC/AVIF re-decode tests pass unchanged.
- [ ] 3.3 `RunSummary.damaged` from `CatalogReader::damaged_count(sources)` over `file_preview` (D6), filled beside `undecoded`, also for Fill runs. Verify: `scenario_scan_progress_damaged_files_counted_again_on_an_unchanged_rescan`; a `ps-catalog` test that the query plan uses `file_preview`; an old `stats` JSON without the field reads as 0.

## 4. CLI

- [ ] 4.1 `print_summary` damaged line and `--json` `damaged` (D6). Update `scenario_scan_progress_damaged_raw_file_still_reported_under_the_default_filter` to pin `unreadable`, `errors 1`, `damaged 1` (and the same outcome with `RUST_LOG=rawler=warn`). Verify: that test, `scenario_cli_summary_names_a_damaged_raw`, `scenario_cli_no_damaged_line_for_intact_files`.
- [ ] 4.2 `report`: `counts.damaged`, `damaged_files`, the badge suffix, the hint line and up to 20 paths (D7). Verify: `scenario_completion_report_exported_damaged_raw_is_still_flagged`; the existing report tests (badge strings) pass, with an intact corpus showing no suffix.

- [ ] 4.3 Cleanup keeps damaged originals (D10): the planner skips them with the reason, and `cleanup run` re-checks it. Verify: `scenario_cleanup_cleanup_keeps_a_damaged_original` (a plan made before the rescan that found the file damaged also leaves it in place).

## 5. App

- [ ] 5.1 `RunOverview.damaged`; the dashboard's damaged stat and warning notice with Show (D6); `FakeScans` gets a setter like `set_undecoded`. Verify: `scenario_scan_progress_dashboard_warns_about_damaged_files` (headless `#[gpui_kit::test]`); `cargo xtask tokens`.
- [ ] 5.2 Reports view damaged line and drill-down (D7). Verify: `scenario_completion_report_reports_view_drills_into_damaged_files` (headless).
- [ ] 5.3 The outdated-placeholder sentence of the "Why no preview?" dialog (D8). Verify: a `ps-service` test on a generation-0 RAW placeholder's `PreviewHelp.sentence`; `grep -rn "an older version could not, so" crates` finds nothing.
- [ ] 5.4 Screenshot review: the dashboard summary and the Reports view with a fixture catalog holding cut CR2s (`cargo xtask screenshot`, light and dark), fixture data only. Verify: PNGs under `target/` reviewed; no colour or size literals.

## 6. Verification

- [ ] 6.1 Docs: release notes "What's new since 0.3.0" line ("A RAW file cut short, for example by a card pulled while writing, is shown as damaged and counted, not as 'not supported yet'"); README unchanged unless it mentions placeholder reasons. Verify: `grep -n "cut short" packaging/release-notes.md`.
- [ ] 6.2 `cargo xtask ci` green in a clean worktree; every scenario of this change mapped and passing.
- [ ] 6.3 Corpus gate unchanged: release CLI, medium preset seed 42, `eval-corpus` precision, recall and keeper accuracy 1.0000, 0 must-not-merge violations (the numbers of `quiet-decoder-logs`); the scan reports `damaged 0`.
- [ ] 6.4 Budgets unchanged: `cargo xtask bench --check` (`scan-analysis-12mp`, `rescan-nochange-100k`; the truncation check is one IFD walk over a buffer already in memory).
- [ ] 6.5 Owner check (read-only, owner-run): a CLI scan of the Canon card into a scratch catalog under `target/` shows `damaged 0` and the same thumbnails as before; a copy of one CR2 cut with `head -c` in a scratch folder under `target/` shows as damaged with the cut-short message. Scratch data removed.
- [ ] 6.6 Pushed; GitHub CI green on all five jobs (Linux full CI, macOS 14, Windows 2022, oldest libheif, performance budgets).
- [ ] 6.7 Gate review by a fresh agent that did not write the code.
