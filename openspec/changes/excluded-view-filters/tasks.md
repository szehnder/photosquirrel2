# Tasks

## 1. Decisions

- [x] 1.1 Number the two ADRs in design.md ("Exclusion categories", D1; "Excluded view budget", D6) from the next free number after `heic-system-decoders`. Add them to `docs/adr/` as accepted, with index rows. Confirm that migration 21 is free (`heic-system-decoders` took 20, ADR-0098). Verify: `docs/adr/README.md` lists both, and `ls crates/ps-catalog/migrations` has no 0021.

## 2. Model and catalog

- [x] 2.1 `ps-model::ExclusionCategory` and `AiKind`: storage keys, labels, priority order, and `of(class, reasons, format)`. Verify: unit tests for each row of the D1 table, including a recognised format with `ProbeFailed` mapping to `unreadable`.
- [x] 2.2 Migration 21: `file.exclusion`, `user_inclusion.origin` and `ai_kind`, the partial index, and the SQL backfill. Verify: `scenario_catalog_old_catalog_gets_categories` (a fixture catalog written by migration 20, with a byte-counting file layer that reads 0 bytes), and the migration round-trip tests still pass.
- [x] 2.3 Write `file.exclusion` in the statements that write `class` and `reasons` (analysis, re-classification, refresh). Verify: `scenario_catalog_category_follows_re_classification`, `scenario_catalog_recognised_format_that_cannot_be_read_is_not_unsupported`, and the property test that re-derives the category of every row of the scanned small corpus after scan, re-classify and refresh.
- [x] 2.4 `Filter::exclusion` (categories, AI kinds, text), sorts by name, folder, size and category, and `exclusion_counts`. Verify: `scenario_gallery_each_excluded_file_has_one_category`, and filter and count agreement tests (the count for each chip equals the length of `id_list` with that chip alone).

## 3. Service and CLI

- [x] 3.1 `excluded_page`, `ExcludedRow::{category, thumb}`; remove the all-rows `excluded()`; move callers over. Verify: `scenario_gallery_excluded_view_lists_reasons` (updated: every category on) and `cargo test -p ps-service`.
- [x] 3.2 AI accept writes `origin='ai'` and `ai_kind`. Put back sets include and rejects the AI suggestion, batched. Verify: `scenario_ai_review_accept_exclusion_suggestion` (updated), `scenario_ai_review_put_back_stays_put_after_another_ai_run` (recorded-fixture AI queue), and `scenario_gallery_put_back_a_classifier_exclusion`.
- [x] 3.3 CLI `report excluded [--category ...] [--json]`. Verify: a CLI test on the small corpus that the counts match `exclusion_counts`, and that the `report` output is otherwise unchanged.

## 4. Excluded view

- [x] 4.1 Toolbar: chips with live counts, the AI sub-chip menu, the hidden count with "Show all", defaults. Verify: headless `scenario_gallery_unsupported_files_hidden_by_default`, `scenario_gallery_filter_to_ai_excluded_documents`, `scenario_gallery_show_unsupported_files_on_request`, `scenario_gallery_counts_follow_decisions`.
- [x] 4.2 Search and sort. Verify: `scenario_gallery_search_by_folder` and `scenario_gallery_sort_by_size`.
- [x] 4.3 List mode with the paged id list and a thumbnail column; Grid mode through the `ui-gallery` grid; selection drives the inspector. Verify: `scenario_gallery_grid_mode_shows_excluded_thumbnails` and `scenario_gallery_list_mode_shows_a_thumbnail_per_row`.
- [x] 4.4 Persist chips, sort and mode per catalog. Verify: `scenario_gallery_filter_choice_remembered`.
- [x] 4.5 Put back for the selection and for everything shown, with confirmation. Verify: `scenario_gallery_put_back_ai_excluded_documents` (with a rescan afterwards) and `scenario_gallery_put_back_needs_confirmation`.
- [x] 4.6 Tokens only, and no stray dividers when the toolbar wraps. Verify: `cargo xtask tokens`, and the existing toolbar-wrap test pattern applied to the Excluded toolbar.

## 5. Budget

- [x] 5.1 Bench `excluded-view-open` on a 1M-row catalog with 200,000 excluded, and add it to `perf-budgets.toml`. Verify: `scenario_performance_budgets_excluded_view_open_within_budget`, `scenario_gallery_excluded_view_opens_at_scale` (the timing test listed in `xtask/src/timing.rs`, or the bench check, as ADR-0075 requires), and `cargo xtask bench --check`.

## 6. Docs

- [x] 6.1 README: the Excluded view section explains categories, the hidden unsupported files, and put back. Update the screenshot. Verify: `grep -n "Not a supported file" README.md`, and the screenshot file is refreshed by `cargo xtask screenshot`.

## 7. Verification

- [x] 7.1 `cargo xtask ci` green in a clean worktree; every scenario of this change mapped (`cargo xtask spec-coverage`).
- [x] 7.2 Corpus gate unchanged: precision, recall and keeper accuracy 1.0000; 0 must-not-merge violations; 19,381 pairs, 1,843 groups, 8,594 files; 4,048,886,433 reclaimable bytes; 5,570 suggestions.
- [x] 7.3 Screenshot review of the Excluded view in List and Grid modes, light and dark, with chips and a hidden count.
- [x] 7.4 Pushed; GitHub CI green on all four jobs.
- [ ] 7.5 Gate review by a fresh agent that did not write the code: every scenario, the budget, the screenshots and the put-back safety.

## 8. Gate review fixes

- [x] 8.1 Header wording: "Showing N of M set aside" while chips hide files, "M files set aside" otherwise; "1 file" and "Put back 1 file" singular in the header, the confirmation and the context menu (D8). Verify: `excluded_header_says_shown_of_total`.
- [x] 8.2 Damaged photos and unreadable files are "could not be read", not "not a supported file" (ADR-0104): the amended rule in `ExclusionCategory::of` and `writer::exclusion_sql`, migration 22 recomputing stored categories, `preview-placeholders` moved to migration 23. Verify: `scenario_catalog_damaged_photo_is_not_unsupported`, `exclusion_rule_is_the_same_in_rust_the_writer_and_migration_22`, `damaged_photos_and_unreadable_files_are_not_unsupported` (the probe's I/O error record), `migration_21_backfills_categories_and_ai_origins` and `writer_stores_unreadable_for_a_recognised_format_that_failed_to_probe` (updated).
- [x] 8.3 "Put back all shown" keeps its confirmation through live refreshes, shows the new count when the list changes, and puts back only the set it showed. Verify: `put_back_confirmation_survives_live_refreshes`.
- [x] 8.4 `report excluded --json` prints a path that is not UTF-8 as a string with `path_raw`. Verify: `report_excluded_json_prints_a_non_utf8_path_as_a_string`.
- [x] 8.5 Search matches the displayed source root and the path under it, Unicode case-insensitively. Verify: `search_matches_the_displayed_root_and_any_script`, `display::tests::folding_is_unicode`, `scenario_gallery_search_by_folder`.
- [x] 8.6 Grid tiles carry a category badge, and files without a thumbnail draw the no-preview icon. Verify: `grid_tiles_show_their_category_and_a_no_preview_icon`.
- [x] 8.7 Keyboard: List mode focus, arrows, Shift-extend, select all, P and Shift+P, Enter and Escape; chips are tab stops that Space and Enter toggle. Verify: `list_mode_keyboard`, `chips_toggle_from_the_keyboard`.
- [x] 8.8 `excluded-view-open` measured through `Library::excluded_open` and `excluded_page` (decisions in one query), first run and median printed (ADR-0104). Verify: `scenario_performance_budgets_excluded_view_open_within_budget` in `ps-service/tests/budgets.rs` under `cargo xtask bench --check`.
- [x] 8.9 A refresh's category follows the new probe's format; the drift test derives categories by probing the files again. Verify: `refresh_files_the_category_by_the_new_probe_format`, `stored_category_matches_the_rule_after_scan_reclassify_and_refresh`.
- [x] 8.10 The context menu counts the set the action puts back. Verify: `excluded_header_says_shown_of_total` (`put_back_label`) and the shared `selected_ids` in the code.
- [x] 8.11 A failed AI rejection after the decisions commit is reported as put back with a warning. Verify: `put_back_reports_an_ai_rejection_that_could_not_be_saved`, `put_back_warns_when_the_ai_rejection_is_not_saved`.
- [x] 8.12 No AI decision without its kind: migration 22 repairs, triggers refuse, readers file any as yours. Verify: `ai_decision_without_a_kind_is_refused_and_repaired`.
- [x] 8.13 Screenshot polish: the AI chip's outlined style, the put back row, the inspector loaded before capture; List and Grid captured dark and light, `docs/screenshots/gallery-excluded-dark.png` refreshed. Verify: the PNGs looked at.
- [ ] 8.14 `cargo xtask ci` green in a clean worktree, the corpus gate unchanged, GitHub CI green.
