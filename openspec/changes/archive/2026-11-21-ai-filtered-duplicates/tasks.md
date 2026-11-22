# Tasks

Feedback on 0.4.0-rc.4. Depends on `review-progress-anchor` (the progress card). No catalog migration. Tests use
the small corpus with recorded or seeded insights (`seed_demo`) in a temp dir, and no live LLM.

## 1. Service: insight filter on the review list

- [x] 1.1 Add `insight: Option<InsightCriteria>` and `undescribed: bool` to `ReviewFilter`, `matching` /
  `matching_reviewed` to `ReviewList`, and `review::list_with` with the `GroupMatcher` trait, an index matcher
  over `ItemSet`s and a lookup matcher (design D1). Verify: `ps-service` tests show that a group matches on a
  non-keeper's insight, that undescribed groups are listed, that `matching` ignores the unreviewed chip, that a
  list without an AI filter consults no matcher, and that the index matcher and the lookup matcher list the same
  groups on a seeded corpus.
- [x] 1.2 Write `scenario_duplicate_review_filtering_never_changes_groups` in `ps-service`: snapshot every group's
  members and keeper, apply and clear each AI filter, and compare. Verify: it passes.
- [x] 1.3 Measure the app's filtered list with "contains people" on the 1M catalog with 100k groups and seeded
  insights, after the index is built (design risk). Verify: record p95 in this task's progress note. If it is over
  150 ms, stop and propose a budget and ADR before continuing.
  Progress: `ai_filtered_list_over_1m` measured a 1.14-1.30 s median: the unfiltered list (~620 ms), two insight
  stamps (~235 ms each) and ~40 ms of filtering. Over 150 ms, so the owner chose (2026-11-21) to speed up reloads.
  `insight_matches_many` (D9) brought it to 911 ms. The rest is group 1b, under ADR-0133 and the
  `duplicates-list-reload` budget.

## 1b. Fast reloads (ADR-0133)

- [x] 1b.1 Owner: review ADR-0133 (written `proposed` with this change). Verify: it reads `Status: accepted` and its
  index row drops "(proposed)".
  Progress: accepted by the owner on 2026-11-21.
- [x] 1b.2 `insights::stamp` recounts `ai_result` only when `max(seq)` or `max(at)` moved (design D8). Verify: a
  `ps-service` test shows the stamp changes when a result is added or replaced in place, and the 1M bench prints
  the stamp below 20 ms.
  Progress: `insight_stamp_sees_new_and_replaced_results` passes; on 1M results the stamp went from 235 ms to 0.0 ms (median, `ai_filtered_list_over_1m`).
- [x] 1b.3 `ReviewCache` with `GroupsKey`, used by `CatalogReview` for lists and progress, replacing the
  `ProgressKey` cache (design D7). Verify: `ps-service` tests show a reload reuses the groups when nothing changed,
  and that each edit (review, keeper override, merge, separate, dismiss a suggestion, rescan) appears in the next
  reload. Existing Duplicates UI tests pass.
  Progress: `review_cache_reloads_see_every_edit` compares the cached reload with an uncached list after a review mark (and checks the groups were reused), a keeper override, a merge, a separation, a dismissed suggestion and a rescan. On 1M files with 100k groups a reload takes 57 ms (was 634 ms; 911 ms with an AI filter), and cached progress 64 ms; the key query is 45 ms of that.
- [x] 1b.4 Add the `duplicates-list-reload` budget: `perf-budgets.toml` (median <= 150 ms) and
  `scenario_performance_budgets_duplicates_list_reload_within_budget` in `ps-service` `tests/budgets.rs` (1M files,
  100,000 groups of 3, an insight per keeper; a reload after R, and a reload with "contains people"). Verify: the
  release run is within budget, and its numbers are recorded here.
  Progress: release run: first (uncached) load 674 ms; reload after a review mark 50.6 ms, with "contains people" 49.9 ms; `measure duplicates-list-reload=50.643 ms` (budget 150 ms).

## 2. Duplicates view: AI filter, progress and navigation

- [x] 2.1 Add the "AI" filter menu and removable chips in the header (design D6). Verify:
  `scenario_duplicate_review_filter_groups_with_children`, `scenario_duplicate_review_a_user_correction_decides_the_match`,
  `scenario_duplicate_review_groups_not_described_yet` and `scenario_duplicate_review_ai_filter_combines_with_unreviewed`
  pass.
  Progress: an "AI insight" menu next to Sort (contains people/children/pets, category, not described yet; "not described yet" and the criteria exclude each other) and removable chips; the four scenarios pass.
- [x] 2.2 Add the "Matching: N of M reviewed" line to the progress card. R advances to the next unreviewed matching
  group, and J and K stay within the matching groups. Verify: `scenario_duplicate_review_matching_progress_shown` and
  `scenario_duplicate_review_review_advances_within_the_filter` pass.
  Progress: `duplicates-progress-matching` on the card; R, J and K work on the filtered rows. Both scenarios pass.
- [x] 2.3 Poll the insight stamp while an AI filter is set and the view is visible, and reload while keeping the
  selection (design D2). Verify: `scenario_duplicate_review_newly_described_group_joins_the_list` passes with a
  recording provider, and a test shows no polling without an AI filter.
  Progress: `watch_insights` polls `ai_stamp` every 500 ms (the AI Review cadence) only while an AI filter is set; the join scenario advances the test clock and checks the group appears within 1 s with the selection kept, then that clearing the filter stops the poll.
- [x] 2.4 Persist the Duplicates view's filter and sort (design D3). Verify:
  `scenario_duplicate_review_filter_kept_across_restarts` passes, and a corrupt state file opens the view
  unfiltered with a logged warning.
  Progress: `UiState.duplicates` (`DuplicatesViewState`), handed to the view through `ViewContext` and saved by the shell when the view publishes `DuplicatesState`; `ReviewFilter` and `InsightCriteria` deserialize with defaults, so an older or partial file still loads (an unreadable one is already logged and ignored by `UiStateFile::load`). The restart scenario passes.

## 3. AI Review: open matching duplicates

- [x] 3.1 Add "Review duplicates of these" (a button and a command-palette entry) that dispatches
  `OpenDuplicatesFiltered` (design D4). Verify: `scenario_ai_review_review_duplicates_of_filtered_photos` passes.
  Progress: an `ai-review-duplicates` button and a palette command (AI › Review Duplicates of These) open Duplicates via `Shell::open_duplicates_filtered` with the category and contains criteria.

## 4. CLI

- [x] 4.1 Add `--category`, `--people`, `--children`, `--pets`, `--pet-kind`, `--people-min`, `--people-max`,
  `--undescribed` and `--unreviewed` to `groups list`. With any of them set, it lists through `review::list_with`
  and the lookup matcher, and the JSON adds `matching` / `matching_reviewed` (design D5). Document them in the
  command's `--help`. Verify: `scenario_cli_cli_lists_groups_with_pets` and `scenario_cli_cli_lists_undescribed_groups`
  pass, and `groups list --help` shows the flags.
  Progress: flags documented in `--help` (checked by the undescribed scenario); the pets scenario also checks `--pet-kind dog`.

## 5. Verification

- [x] 5.1 Screenshot review (`cargo xtask screenshot -- --theme dark` and `--theme light`): the Duplicates header
  with two AI chips and the Matching line at the narrowest window width. Chips wrap, and nothing crosses the panel
  edge. Verify: the PNGs reviewed by a fresh reviewer.
  Progress: `cargo xtask screenshot --all-surfaces` with the new `duplicates-ai-filter` and `duplicates-similar` surfaces; a fresh reviewer passed every criterion. The first capture caught two defects, now fixed: the sort row clipped "N shown" (it wraps now) and a pair's headline repeated its reason line.
- [x] 5.2 `cargo xtask spec-coverage` maps all 12 new scenarios. Verify: the command passes.
  Progress: a `scenario_<capability>_<slug>` test exists for every scenario of this change (13, with the budget scenario); `spec-coverage --enforce` checks them after archive.
- [x] 5.3 `cargo xtask ci` passes. Verify: the command is green.
  Progress: final run 2026-11-21: fmt, clippy (workspace, -D warnings), layering, tokens, the timing pass, openspec validate (37/37) and spec coverage are green; `cargo test --workspace` passes 173 of 174 targets. The one failure is the pre-existing, environmental `ps-scan` `volume_of_the_manifest_dir_is_identified` (this machine's btrfs subvolume reports a device that is not in `/proc/self/mountinfo`); `ps-scan` is untouched.
- [x] 5.4 `openspec validate ai-filtered-duplicates --strict` passes. Verify: the command reports it valid.
  Progress: valid.

## Workflow follow-up

- Add a ROADMAP row (0.5.0) after `review-progress-anchor` before applying.
- Archive with `/opsx:archive ai-filtered-duplicates` once CI is green.
