# Tasks

Feedback on 0.4.0-rc.4. Depends on `review-progress-anchor` (the progress card). No catalog migration. Tests use
the small corpus with recorded or seeded insights (`seed_demo`) in a temp dir, and no live LLM.

## 1. Service: insight filter on the review list

- [ ] 1.1 Add `insight: Option<InsightFilter>` and `undescribed: bool` to `ReviewFilter`, and `matching` /
  `matching_reviewed` to `ReviewList` (design D1). Read effective insights for the groups' contents in one batch,
  and only when an AI filter is set. Verify: `ps-service` unit tests show that a group matches on a non-keeper's
  insight, that undescribed groups are listed, that `matching` ignores the unreviewed chip, and that a list without
  an AI filter reads no insights.
- [ ] 1.2 Write `scenario_duplicate_review_filtering_never_changes_groups` in `ps-service`: snapshot every group's
  members and keeper, apply and clear each AI filter, and compare. Verify: it passes.
- [ ] 1.3 Measure `groups(filter)` with "contains people" on the 1M bench catalog with seeded insights (design
  risk). Verify: record p95 in this task's progress note. If it is over 150 ms, stop and propose a budget and ADR
  before continuing.

## 2. Duplicates view: AI filter, progress and navigation

- [ ] 2.1 Add the "AI" filter menu and removable chips in the header (design D6). Verify:
  `scenario_duplicate_review_filter_groups_with_children`, `scenario_duplicate_review_a_user_correction_decides_the_match`,
  `scenario_duplicate_review_groups_not_described_yet` and `scenario_duplicate_review_ai_filter_combines_with_unreviewed`
  pass.
- [ ] 2.2 Add the "Matching: N of M reviewed" line to the progress card. R advances to the next unreviewed matching
  group, and J and K stay within the matching groups. Verify: `scenario_duplicate_review_matching_progress_shown` and
  `scenario_duplicate_review_review_advances_within_the_filter` pass.
- [ ] 2.3 Poll the insight stamp while an AI filter is set and the view is visible, and reload while keeping the
  selection (design D2). Verify: `scenario_duplicate_review_newly_described_group_joins_the_list` passes with a
  recording provider, and a test shows no polling without an AI filter.
- [ ] 2.4 Persist the Duplicates view's filter and sort (design D3). Verify:
  `scenario_duplicate_review_filter_kept_across_restarts` passes, and a corrupt state file opens the view
  unfiltered with a logged warning.

## 3. AI Review: open matching duplicates

- [ ] 3.1 Add "Review duplicates of these" (a button and a command-palette entry) that dispatches
  `OpenDuplicatesFiltered` (design D4). Verify: `scenario_ai_review_review_duplicates_of_filtered_photos` passes.

## 4. CLI

- [ ] 4.1 Add the insight flags and `--unreviewed` to `groups list`, and add `matching` / `matching_reviewed` to its
  JSON output (design D5). Document them in the command's `--help`. Verify:
  `scenario_cli_cli_lists_groups_with_pets` and `scenario_cli_cli_lists_undescribed_groups` pass, and `groups list
  --help` shows the flags.

## 5. Verification

- [ ] 5.1 Screenshot review (`cargo xtask screenshot -- --theme dark` and `--theme light`): the Duplicates header
  with two AI chips and the Matching line at the narrowest window width. Chips wrap, and nothing crosses the panel
  edge. Verify: the PNGs reviewed by a fresh reviewer.
- [ ] 5.2 `cargo xtask spec-coverage` maps all 12 new scenarios. Verify: the command passes.
- [ ] 5.3 `cargo xtask ci` passes. Verify: the command is green.
- [ ] 5.4 `openspec validate ai-filtered-duplicates --strict` passes. Verify: the command reports it valid.

## Workflow follow-up

- Add a ROADMAP row (0.5.0) after `review-progress-anchor` before applying.
- Archive with `/opsx:archive ai-filtered-duplicates` once CI is green.
