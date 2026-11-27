# Tasks

## 1. Stack rule and calibration (core)

- [x] 1.1 `StackThresholds` and `StackSimilarity` presets, and the time-sorted sweep with union-find and the 120 s span cap, as a pure function over a grouping. Videos, untimed and excluded groups are skipped. Verify: ps-dedup unit tests for the sweep, the span cap and `scenario_photo_stacks_shots_without_capture_time_are_not_stacked` and `scenario_photo_stacks_stack_span_is_limited`.
- [x] 1.2 Separations honoured under any thresholds. Verify: unit test `taken_out_stays_out_under_any_thresholds` (all three presets at a 60 s window); the persisted scenario `scenario_photo_stacks_taken_out_shot_stays_out` is checked in 3.2.
- [x] 1.3 Pick ranking (user pick, sharpness with a 10% tie band, clipping in 1% steps, keeper rank, earliest, file id) and reason labels. Verify: `scenario_photo_stacks_the_sharp_frame_is_picked`, `scenario_photo_stacks_clipping_breaks_a_sharpness_tie` and `scenario_photo_stacks_pick_is_stable`, as ps-dedup unit tests over synthetic bursts (the corpus-level checks are in 1.4 and 3.2).
- [ ] 1.4 Calibrate the similarity table on the small and medium corpora (`burst` sets in one stack, no `minutes_apart` set in one) and on the owner's card. Update ADR-0135's table and the presets to the chosen values. Verify: `scenario_photo_stacks_corpus_bursts_form_stacks` passes at the defaults, the table in ADR-0135 matches the code, and the measured figures are recorded in this task.
  - Corpus figures (small corpus, seed 42, `cargo test -p ps-service --test stacks -- --nocapture`): at every similarity level, burst sets are whole in one stack at 1 s 10/14, 2 s 11/14, and from 5 s on 14/14. No `minutes_apart` or `crop` set is stacked at any window up to 60 s. `different_camera` sets stack at 1/10 up to 5 s and 5/10 at 60 s (balanced or loose). The corpus does not separate the similarity levels, because its burst frames are very alike; the window decides. The defaults (5 s, balanced) pass the gate. Still to do: the owner's card.
- [x] 1.5 Groups are untouched. Verify: `scenario_photo_stacks_stacks_leave_groups_unchanged` (stored groups, keepers and suggestions are identical before and after the loosest stacks) and `scenario_photo_stacks_no_source_read_for_stacks`. Stacks are a pure function of the inputs, so `eval-corpus` scores cannot change; 8.1 runs it.

## 2. Shot quality

- [x] 2.1 `shot_quality` from an RGB thumbnail (luma, Laplacian variance per pixel, clipped fractions). Verify: ps-media unit tests on a sharp and a blurred thumbnail, and on a blown-out one.
- [x] 2.2 Catalog migration 27: shot-quality columns, `stack_separations` and `stack_picks`, with writer commands. Verify: a migration test from a version-26 fixture, and writer round-trip tests.
- [x] 2.3 Score new photos from their stored thumbnails at the end of every completed scan, after the regroup (design decision 5: one path from the stored thumbnail, about 1 ms per file). Verify: `scenario_photo_stacks_scores_for_new_photos` (a box-blurred copy scores lower). `rescan-nochange-100k` and `catalog-insert` are measured in 8.2.
- [x] 2.4 Background fill from stored thumbnails: starts `ScanOptions::shot_quality_delay` (10 s) after open, owner process only, in batches of 1,000, paused while any scan runs, ended when the scanner is dropped, never reading a source; undecodable thumbnails are recorded as tried. Verify: `scenario_photo_stacks_existing_catalog_filled_in_from_thumbnails` (sources renamed away, open returns at once, every thumbed photo is scored). The `shot-quality-fill` budget is in 3.3.

## 3. Service and settings

- [x] 3.1 `review` settings section (`stack_window_s`, `stack_similarity`) with validation and `doctor` output. Verify: `scenario_settings_review_defaults` and `scenario_settings_invalid_stack_window`.
- [x] 3.2 Stacks service: `StackCache` keeps the loaded inputs and scores, keyed by the stored groups, scan runs, exclusions and scored count, and recomputes stacks from them on every threshold change, pick or take-out. Pick, clear pick, take out and put back are written under the cleanup edit guard. Review state is kept by anchor content in the review store. (The UI side holds the generation counter and `Arc` snapshots; see 5.x.) Verify: `scenario_photo_stacks_looser_thresholds_make_fewer_larger_stacks`, `scenario_photo_stacks_window_of_1_s_keeps_a_5_s_pair_apart`, `scenario_photo_stacks_no_source_read_for_stacks`, `scenario_photo_stacks_your_pick_is_kept` and `scenario_photo_stacks_taken_out_shot_stays_out` (with put back).
- [x] 3.3 Benchmarks `stacks-compute` (1M photos, 60 s and loose, inputs in memory), `stacks-retune` (100k) and `shot-quality-fill` (100k rows sharing 1,000 stored thumbnails), added to `perf-budgets.toml`. Verify, release, full scale: `scenario_performance_budgets_stacks_computed_within_budget` 0.377 s against 1.0 s (1.214 s before precomputing each shot's hashes, aspect and orientation), `scenario_performance_budgets_retune_within_budget` 24 ms against 300 ms, `scenario_performance_budgets_shot_quality_filled_within_budget` 49.9 s for 89,918 thumbnails against 60 s.

## 4. Export, report, cleanup

- [ ] 4.1 Export plan `BestOf { Everything, PicksOnly, Both }`, the `Best of/` destination with a `best_of` ledger role, the last export's mode in the ledger, and the plan counts per part. Verify: `scenario_export_best_of_only`, `scenario_export_both` and `scenario_export_default_unchanged`.
- [x] 4.2 The `not picked` disposition (after `excluded`, before `pending`; accounted for, never cleanup-eligible), decided by the library's recorded best-of choice and its thresholds (`export_library.best_of`, migration 27). Verify: `scenario_completion_report_shots_not_picked_after_a_best_of_export` and `scenario_completion_report_not_picked_shots_are_exported_by_a_full_export` (scenario corrected: planning writes nothing, so it is the export of everything that brings them back). The existing completion-report scenarios run in CI.
- [x] 4.3 Cleanup never plans not picked files: `unsafe_now` refuses them and `is_accounted_safe` excludes them. Verify: `scenario_cleanup_shots_not_picked_are_not_eligible`. The existing cleanup scenarios run in CI.

## 5. UI

- [ ] 5.1 Stacks tab (T): the stack list, a filmstrip in time order, the pick marked "Best of" with reasons, the score words, and the progress card. Verify: `scenario_duplicate_review_open_a_stack`.
- [ ] 5.2 Keys in a stack: digits pick, Space compares, S takes out, R reviews, J and K move; plus a "Taken out" disclosure with Put back. Verify: `scenario_duplicate_review_pick_with_a_number_key`, `scenario_duplicate_review_review_stacks_with_r` and `scenario_duplicate_review_take_out_with_s`.
- [ ] 5.3 Header threshold controls with a live count, writing settings debounced by 300 ms; and the Settings > Review page. Verify: `scenario_duplicate_review_retune_from_the_header`.
- [ ] 5.4 Gallery "Best of" filter chip over the stack snapshot. Verify: `scenario_gallery_best_of_shows_one_shot_per_stack` and `scenario_gallery_best_of_follows_a_new_pick`; `gallery-scroll` p95 unchanged.
- [ ] 5.5 Export view: the "What to export" choice with per-part counts and bytes in the plan. Verify: a UI test that the plan shows the three parts, plus the export scenarios from 4.1 driven from the view.

## 6. CLI

- [ ] 6.1 `stacks list [--window] [--similarity] [--json]`, `stacks show`, `stacks pick`, `stacks take-out`, `export run --best-of only|both`, and `not picked` in `report`. Verify: `scenario_cli_list_stacks_as_json`, `scenario_cli_thresholds_from_the_command_line`, `scenario_cli_pick_from_the_cli` and `scenario_cli_best_of_export_from_the_cli`.

## 7. Docs and review

- [ ] 7.1 README (Stacks, the thresholds, Best of, the export choice, the CLI), keyboard help, and "What's new in 0.6.0". Verify: the text names T, the two thresholds, the three export choices and `not picked`.
- [ ] 7.2 Screenshot surfaces `duplicates-stacks`, `gallery-best-of` and `export-best-of` in light and dark, reviewed by a fresh reviewer. Verify: PNGs under `target/screenshots/` and the review notes.
- [ ] 7.3 The owner accepts ADR-0135 with the calibrated table. Verify: its status reads accepted, and the README index row has no "(proposed)".

## 8. Verification

- [ ] 8.1 `cargo xtask ci` passes, every new scenario maps to a passing test (`cargo xtask spec-coverage`), and `photosquirrel-cli eval-corpus` passes the gate unchanged.
- [ ] 8.2 `cargo xtask bench --check` passes with the three new budgets, and `rescan-nochange-100k`, `duplicates-list-reload` and `gallery-scroll` are unchanged.
- [ ] 8.3 `openspec validate best-of-stacks --strict` passes.
