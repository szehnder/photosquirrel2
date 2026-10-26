# Tasks

## 1. Decisions

- [ ] 1.1 Number the two ADRs in design.md ("Exclusion categories", D1; "Excluded view budget", D6) from the next free number after `heic-system-decoders`. Add them to `docs/adr/` as accepted, with index rows. Confirm that migration 21 is free (`heic-system-decoders` took 20, ADR-0098). Verify: `docs/adr/README.md` lists both, and `ls crates/ps-catalog/migrations` has no 0021.

## 2. Model and catalog

- [ ] 2.1 `ps-model::ExclusionCategory` and `AiKind`: storage keys, labels, priority order, and `of(class, reasons, format)`. Verify: unit tests for each row of the D1 table, including a recognised format with `ProbeFailed` mapping to `unreadable`.
- [ ] 2.2 Migration 21: `file.exclusion`, `user_inclusion.origin` and `ai_kind`, the partial index, and the SQL backfill. Verify: `scenario_catalog_old_catalog_gets_categories` (a fixture catalog written by migration 20, with a byte-counting file layer that reads 0 bytes), and the migration round-trip tests still pass.
- [ ] 2.3 Write `file.exclusion` in the statements that write `class` and `reasons` (analysis, re-classification, refresh). Verify: `scenario_catalog_category_follows_re_classification`, `scenario_catalog_recognised_format_that_cannot_be_read_is_not_unsupported`, and the property test that re-derives the category of every row of the scanned small corpus after scan, re-classify and refresh.
- [ ] 2.4 `Filter::exclusion` (categories, AI kinds, text), sorts by name, folder, size and category, and `exclusion_counts`. Verify: `scenario_gallery_each_excluded_file_has_one_category`, and filter and count agreement tests (the count for each chip equals the length of `id_list` with that chip alone).

## 3. Service and CLI

- [ ] 3.1 `excluded_page`, `ExcludedRow::{category, thumb}`; remove the all-rows `excluded()`; move callers over. Verify: `scenario_gallery_excluded_view_lists_reasons` (updated: every category on) and `cargo test -p ps-service`.
- [ ] 3.2 AI accept writes `origin='ai'` and `ai_kind`. Put back sets include and rejects the AI suggestion, batched. Verify: `scenario_ai_review_accept_exclusion_suggestion` (updated), `scenario_ai_review_put_back_stays_put_after_another_ai_run` (recorded-fixture AI queue), and `scenario_gallery_put_back_a_classifier_exclusion`.
- [ ] 3.3 CLI `report excluded [--category ...] [--json]`. Verify: a CLI test on the small corpus that the counts match `exclusion_counts`, and that the `report` output is otherwise unchanged.

## 4. Excluded view

- [ ] 4.1 Toolbar: chips with live counts, the AI sub-chip menu, the hidden count with "Show all", defaults. Verify: headless `scenario_gallery_unsupported_files_hidden_by_default`, `scenario_gallery_filter_to_ai_excluded_documents`, `scenario_gallery_show_unsupported_files_on_request`, `scenario_gallery_counts_follow_decisions`.
- [ ] 4.2 Search and sort. Verify: `scenario_gallery_search_by_folder` and `scenario_gallery_sort_by_size`.
- [ ] 4.3 List mode with the paged id list and a thumbnail column; Grid mode through the `ui-gallery` grid; selection drives the inspector. Verify: `scenario_gallery_grid_mode_shows_excluded_thumbnails` and `scenario_gallery_list_mode_shows_a_thumbnail_per_row`.
- [ ] 4.4 Persist chips, sort and mode per catalog. Verify: `scenario_gallery_filter_choice_remembered`.
- [ ] 4.5 Put back for the selection and for everything shown, with confirmation. Verify: `scenario_gallery_put_back_ai_excluded_documents` (with a rescan afterwards) and `scenario_gallery_put_back_needs_confirmation`.
- [ ] 4.6 Tokens only, and no stray dividers when the toolbar wraps. Verify: `cargo xtask tokens`, and the existing toolbar-wrap test pattern applied to the Excluded toolbar.

## 5. Budget

- [ ] 5.1 Bench `excluded-view-open` on a 1M-row catalog with 200,000 excluded, and add it to `perf-budgets.toml`. Verify: `scenario_performance_budgets_excluded_view_open_within_budget`, `scenario_gallery_excluded_view_opens_at_scale` (the timing test listed in `xtask/src/timing.rs`, or the bench check, as ADR-0075 requires), and `cargo xtask bench --check`.

## 6. Docs

- [ ] 6.1 README: the Excluded view section explains categories, the hidden unsupported files, and put back. Update the screenshot. Verify: `grep -n "Not a supported file" README.md`, and the screenshot file is refreshed by `cargo xtask screenshot`.

## 7. Verification

- [ ] 7.1 `cargo xtask ci` green in a clean worktree; every scenario of this change mapped (`cargo xtask spec-coverage`).
- [ ] 7.2 Corpus gate unchanged: precision, recall and keeper accuracy 1.0000; 0 must-not-merge violations; 19,381 pairs, 1,843 groups, 8,594 files; 4,048,886,433 reclaimable bytes; 5,570 suggestions.
- [ ] 7.3 Screenshot review of the Excluded view in List and Grid modes, light and dark, with chips and a hidden count.
- [ ] 7.4 Pushed; GitHub CI green on all four jobs.
