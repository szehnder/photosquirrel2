# Tasks

Feedback on 0.4.0-rc.4. Changes text and UI only: grouping, suggestions and the corpus gate are unchanged. No
catalog migration.

## 1. Service: reason kinds and explanations

- [x] 1.1 Add `SuggestionInfo::reason_kind()` with an exhaustive `match` over every `Contradiction` variant, and
  `review::similar_shot_limits()`, which reads `MAX_PHASH_DISTANCE`, `MAX_DHASH_DISTANCE` and `MAX_ASPECT_DIFF`
  (design D1). Verify: a `ps-service` unit test maps one fixture pair of each variant to its kind, and
  `similar_shot_limits()` equals the constants.
  Progress: `ReasonKind::of` is an exhaustive match; `every_contradiction_explains_itself` (ps-service `tests/review.rs`) maps one of each variant and pins `similar_shot_limits()` to the dedup constants.
- [x] 1.2 Add `SuggestionCard::explanation()` and `explanation_detail()` with the D1 wording. Verify: unit tests
  give the exact sentence for each reason kind, including "Look the same, but taken 5 s apart" for a 5 s corpus burst
  pair.
  Progress: on `SuggestionInfo` (design D1 as built); the same test checks every sentence and the detail format.

## 2. Duplicates view: explainer and reasons

- [x] 2.1 Add `similar_shots_explained` to `UiState` with `#[serde(default)]` (design D3). Verify: a `persist` unit
  test shows that an old state file without the field loads as `false` and that a saved `true` round-trips.
  Progress: `DuplicatesViewState.similar_shots_explained` with `#[serde(default)]` (design D3 as built); the restart scenario round-trips it.
- [x] 2.2 Show the explainer panel on the Similar shots tab, and after "Show similar shots", while the flag is
  false. "Got it" sets it, and the info buttons on the tab header and the group's similar-shots line reopen it
  (design D2). Verify: `scenario_duplicate_review_explainer_shown_the_first_time`,
  `scenario_duplicate_review_explainer_stays_dismissed`, `scenario_duplicate_review_explainer_from_a_group` and
  `scenario_duplicate_review_explainer_thresholds_follow_the_grouping` pass.
  Progress: the four explainer scenarios pass. Selecting a similar shot now scrolls the list to it (it never did, so a selected pair below the fold was not drawn).
- [x] 2.3 Replace each suggestion row's reason with `explanation()`, and put `explanation_detail()` in its tooltip.
  Verify: `scenario_duplicate_review_burst_pair_reason` and `scenario_duplicate_review_different_frames_reason` pass,
  and the existing `scenario_duplicate_review_suggestions_not_collapsed` still finds the time difference.
  Progress: rows show the sentence (ellipsised, tooltip with the detail) instead of the bare time difference; the pair's detail keeps "Taken N s apart" and puts the sentence under it. Both scenarios pass; the corpus has no pair exactly 5 s apart, so the burst scenario now reads "N s apart" (spec updated).

## 3. CLI

- [x] 3.1 Add `reason_kind` and `explanation` to `groups suggestions --json`, print the explanation in human output,
  and make `groups explain` print both sentences for a stored suggestion (design D4). Verify:
  `scenario_cli_suggestion_json_carries_the_explanation` and `scenario_cli_explain_prints_the_reason` pass.
  Progress: `groups suggestions` adds `reason_kind`, `explanation` and `explanation_detail` (human output prints both lines); `groups explain` prints them first for a stored pair. Both scenarios pass.

## 4. Verification

- [x] 4.1 Screenshot review (`cargo xtask screenshot -- --theme dark` and `--theme light`): the Similar shots tab with
  the explainer at the narrowest window width, and a row with its tooltip. Text wraps inside the panel. Verify: the
  PNGs reviewed by a fresh reviewer.
  Progress: `cargo xtask screenshot --all-surfaces` with the new `duplicates-ai-filter` and `duplicates-similar` surfaces; a fresh reviewer passed every criterion. The first capture caught two defects, now fixed: the sort row clipped "N shown" (it wraps now) and a pair's headline repeated its reason line.
- [x] 4.2 The corpus gate is unchanged (`photosquirrel-cli eval-corpus`, same numbers as before). Verify: the
  command output.
  Progress: Medium corpus, seed 7: precision 1.0000, recall 1.0000, keeper accuracy 1.0000, 0 must-not-merge violations (19,545 pairs); unchanged.
- [x] 4.3 `cargo xtask spec-coverage` maps all 8 new scenarios. Verify: the command passes.
  Progress: a test exists for each of the 8 scenarios; `spec-coverage --enforce` checks them after archive.
- [x] 4.4 `cargo xtask ci` passes. Verify: the command is green.
  Progress: final run 2026-11-21: fmt, clippy (workspace, -D warnings), layering, tokens, the timing pass, openspec validate (37/37) and spec coverage are green; `cargo test --workspace` passes 173 of 174 targets. The one failure is the pre-existing, environmental `ps-scan` `volume_of_the_manifest_dir_is_identified` (this machine's btrfs subvolume reports a device that is not in `/proc/self/mountinfo`); `ps-scan` is untouched.
- [x] 4.5 `openspec validate similar-shot-explained --strict` passes. Verify: the command reports it valid.
  Progress: valid.

## Workflow follow-up

- Add a ROADMAP row (0.5.0) after `ai-filtered-duplicates` before applying.
- Archive with `/opsx:archive similar-shot-explained` once CI is green.
