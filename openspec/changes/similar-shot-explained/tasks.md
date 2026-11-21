# Tasks

Feedback on 0.4.0-rc.4. Changes text and UI only: grouping, suggestions and the corpus gate are unchanged. No
catalog migration.

## 1. Service: reason kinds and explanations

- [ ] 1.1 Add `SuggestionInfo::reason_kind()` with an exhaustive `match` over every `Contradiction` variant, and
  `review::similar_shot_limits()`, which reads `MAX_PHASH_DISTANCE`, `MAX_DHASH_DISTANCE` and `MAX_ASPECT_DIFF`
  (design D1). Verify: a `ps-service` unit test maps one fixture pair of each variant to its kind, and
  `similar_shot_limits()` equals the constants.
- [ ] 1.2 Add `SuggestionCard::explanation()` and `explanation_detail()` with the D1 wording. Verify: unit tests
  give the exact sentence for each reason kind, including "Look the same, but taken 5 s apart" for a 5 s corpus burst
  pair.

## 2. Duplicates view: explainer and reasons

- [ ] 2.1 Add `similar_shots_explained` to `UiState` with `#[serde(default)]` (design D3). Verify: a `persist` unit
  test shows that an old state file without the field loads as `false` and that a saved `true` round-trips.
- [ ] 2.2 Show the explainer panel on the Similar shots tab, and after "Show similar shots", while the flag is
  false. "Got it" sets it, and the info buttons on the tab header and the group's similar-shots line reopen it
  (design D2). Verify: `scenario_duplicate_review_explainer_shown_the_first_time`,
  `scenario_duplicate_review_explainer_stays_dismissed`, `scenario_duplicate_review_explainer_from_a_group` and
  `scenario_duplicate_review_explainer_thresholds_follow_the_grouping` pass.
- [ ] 2.3 Replace each suggestion row's reason with `explanation()`, and put `explanation_detail()` in its tooltip.
  Verify: `scenario_duplicate_review_burst_pair_reason` and `scenario_duplicate_review_different_frames_reason` pass,
  and the existing `scenario_duplicate_review_suggestions_not_collapsed` still finds the time difference.

## 3. CLI

- [ ] 3.1 Add `reason_kind` and `explanation` to `groups suggestions --json`, print the explanation in human output,
  and make `groups explain` print both sentences for a stored suggestion (design D4). Verify:
  `scenario_cli_suggestion_json_carries_the_explanation` and `scenario_cli_explain_prints_the_reason` pass.

## 4. Verification

- [ ] 4.1 Screenshot review (`cargo xtask screenshot -- --theme dark` and `--theme light`): the Similar shots tab with
  the explainer at the narrowest window width, and a row with its tooltip. Text wraps inside the panel. Verify: the
  PNGs reviewed by a fresh reviewer.
- [ ] 4.2 The corpus gate is unchanged (`photosquirrel-cli eval-corpus`, same numbers as before). Verify: the
  command output.
- [ ] 4.3 `cargo xtask spec-coverage` maps all 8 new scenarios. Verify: the command passes.
- [ ] 4.4 `cargo xtask ci` passes. Verify: the command is green.
- [ ] 4.5 `openspec validate similar-shot-explained --strict` passes. Verify: the command reports it valid.

## Workflow follow-up

- Add a ROADMAP row (0.5.0) after `ai-filtered-duplicates` before applying.
- Archive with `/opsx:archive similar-shot-explained` once CI is green.
