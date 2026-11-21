# Tasks

Feedback on 0.4.0-rc.4. No catalog migration, no settings change. UI tests are headless `#[gpui_kit::test]` on the
small corpus in a temp dir.

## 1. "In this group" wraps inside its panel

- [ ] 1.1 Write `scenario_duplicate_review_pair_keeper_composition_wraps_inside_the_panel` and
  `scenario_duplicate_review_composition_lines_fit_at_the_default_size` in `crates/ui-workspace/tests/duplicates.rs`.
  They assert that each `duplicates-why-group` line's bounds lie inside the panel's content bounds, and that in the
  first test the pair line is taller than one line. Verify: both fail on the current code.
- [ ] 1.2 Give the composition description `flex_1().min_w_0()` and the row `items_start()` (design D1). Verify:
  both tests pass, and the existing `scenario_duplicate_review_why_this_copy_*` tests still pass.
- [ ] 1.3 Add a note to `docs/gpui-notes.md` under the existing `min_w_0` section: a fixed-width column next to a
  text child still needs `min_w_0()` on the text. Verify: the note is there.

## 2. Review progress card

- [ ] 2.1 Add `success_soft` to the `ui-theme` palette for the dark and light themes, alongside `accent_soft`.
  Verify: `cargo xtask tokens` passes, and the `ui-theme` contrast test covers the new token.
- [ ] 2.2 Write `scenario_duplicate_review_card_shows_groups_left`,
  `scenario_duplicate_review_card_shows_all_reviewed` and `scenario_duplicate_review_card_ignores_the_list_filter`.
  Verify: they fail on the current code.
- [ ] 2.3 Replace the header's muted progress text with the card (design D2): the `duplicates-progress-card`,
  `duplicates-progress-left` and `duplicates-progress` ids, and the success state at 0 left. Update
  `scenario_duplicate_review_filter_unreviewed_groups` only if its text assertion changes. Verify: the three new
  tests and every existing `scenario_duplicate_review_*` test pass.

## 3. Navigator counts groups left to review

- [ ] 3.1 Add `duplicates_reviewed` to `CatalogSummary`. The summary loader computes it with the review list's own
  "is this membership reviewed" function (design D3), on the background executor. Verify: a `ps-service` unit test
  shows the summary's reviewed count equals `ReviewList::reviewed` after reviewing 3 corpus groups.
- [ ] 3.2 `nav_count(Duplicates)` shows groups minus reviewed, with the tooltip "N left to review of M groups", and a
  check icon when groups > 0 and none is left. Marking a group reviewed or unreviewed re-requests the summary.
  Verify: `scenario_app_shell_count_goes_down_as_groups_are_reviewed` and
  `scenario_app_shell_check_mark_when_every_group_is_reviewed` pass.
- [ ] 3.3 Write `scenario_app_shell_count_rises_again_when_a_reviewed_group_changes`: review every group, add a copy
  of one keeper to the corpus source, rescan, and the count reads 1. Verify: it passes.
- [ ] 3.4 Measure the summary refresh on the 1M bench catalog before and after 3.1 (design risk). If it grows by
  more than 20 ms, cache the reviewed count by review-store revision and group stamp. Verify: record both numbers in
  this task's progress note.

## 4. Verification

- [ ] 4.1 Screenshot review (`cargo xtask screenshot -- --theme dark` and `--theme light`): the Duplicates view at the
  narrowest window width with a pair group open, the progress card at "N left" and at "All reviewed", and the
  navigator count and check mark. No text crosses a panel edge. Verify: the PNGs reviewed by a fresh reviewer.
- [ ] 4.2 `cargo xtask spec-coverage` maps all 8 new scenarios. Verify: the command passes.
- [ ] 4.3 `cargo xtask ci` passes. Verify: the command is green.
- [ ] 4.4 `openspec validate review-progress-anchor --strict` passes. Verify: the command reports it valid.

## Workflow follow-up

- Add a ROADMAP row (0.5.0) for this change before applying.
- Archive with `/opsx:archive review-progress-anchor` once CI is green.
