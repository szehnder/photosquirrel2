# Design

## Context

- Suggestions are produced only inside `ps-dedup`'s derivative-pair test. A pair that passes
  `MAX_PHASH_DISTANCE` (6), `MAX_DHASH_DISTANCE` (8) and `MAX_ASPECT_DIFF` (0.02) but has a `Contradiction`
  (`CaptureSecond`, `SubSecond`, `CameraSerial`, `CameraModel`, `TwoCameraOriginals` or `FrameId`) becomes a
  `Suggestion` with its distances and reason (ADR-0013, ADR-0035, ADR-0105). Pairs that fail the hash test leave
  no record.
- `ps-service::review` turns suggestions into `SuggestionCard { info, a, b, time_delta_ms }`. Today its
  `delta_label()` ("5 s apart") is the only reason text the UI shows. The CLI's `groups suggestions` and
  `groups explain a b` print the raw reason.
- `UiState` (`persist.rs`) is the per-catalog UI state file, already used for layout and the Excluded view.

## Goals / Non-Goals

**Goals:**
- One function writes the explanation sentence, and the app and the CLI both use it.
- The thresholds in the text are the engine's constants, never copied numbers.

**Non-Goals:**
- Any change to grouping, suggestions or the corpus gate.
- Localisation. Strings stay in English, built in one place so they can be translated later.

## Decisions

### D1. `ps-service` builds the explanation from the stored contradiction

Add `SuggestionInfo::reason_kind()`, returning `capture_time | sub_second | camera_serial | camera_model |
two_camera_originals | frame`, and `SuggestionCard::explanation()` / `explanation_detail()`:

| reason kind | one line | tooltip adds |
|---|---|---|
| capture time | "Look the same, but taken {delta}" | "pHash {p} of {6}, dHash {d} of {8}; a different capture time means a different shot" |
| sub-second | "Look the same, but {delta} apart in one burst" | same, "... different sub-second time ..." |
| camera serial / model | "Look the same, but taken with different cameras ({a} / {b})" | "... a different camera means a different shot" |
| two camera originals | "Two camera originals of the same size ({model})" | "... a camera writes one original per shot" |
| frame | "Look the same, but frames {a} and {b}" | "... each frame is its own shot" |

The limits are read from `ps_dedup::{MAX_PHASH_DISTANCE, MAX_DHASH_DISTANCE, MAX_ASPECT_DIFF}`. `ps-service` already
depends on `ps-dedup`, and the UI gets the text from the service, so the layering stays intact (ADR-0003).

*Alternative:* write the text in the UI crate. Rejected because the CLI needs the same sentence (ADR-0027) and the
UI should not depend on `ps-dedup` constants directly.

### D2. The explainer is a dismissible panel, not a modal

Above the suggestion list, a `v_flex` card on `colors.accent_soft` (ADR-0028 tokens) shows:

> **How similar shots are found.** Two photos are listed here when they look alike as closely as copies do
> (pHash within 6, dHash within 8, shape within 2%) but their details show two different shots: different capture
> times, cameras or frame numbers. PhotoSquirrel never merges them; every shot stays in your library and is
> exported. Shots of the same moment that look different, because someone moved or the camera turned, are not
> listed here.

It has a "Got it" button. The numbers are interpolated from D1's service accessor
`review::similar_shot_limits()`. A modal was rejected: it would interrupt reviewing, and the AGENTS.md dialog
caution applies to the UI too.

### D3. Dismissal in `UiState`

Add `similar_shots_explained: bool` to `UiState` (`#[serde(default)]`, so old files read as `false`). It is set and
saved on "Got it". The tab header's info button (`IconName::Info`) and the info button on the group's "similar shots
nearby" line show the panel again without clearing the flag.

### D4. CLI

`groups suggestions --json` adds `reason_kind` and `explanation` to each item. The human output prints the
explanation line. `groups explain a b` prints `explanation()` and `explanation_detail()` when the pair is a stored
suggestion.

## Risks / Trade-offs

- [The explainer's wording drifts from what grouping does when a later change adds a contradiction kind] → A test
  matches every `Contradiction` variant to a reason kind. A new variant fails to compile in the `match` until it
  gets wording.
- [Users read "not listed here" as a promise of a future feature] → The text describes today's behaviour only. The
  planned scenes work will change this panel's text when it lands.

## Migration Plan

`UiState` gains a defaulted field. Old state files load unchanged, and the explainer shows once for existing users,
which is the intent.

## Verification

```
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ps-service -- --test-threads=4 review::explanation
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ui-workspace --test duplicates -- --test-threads=4 \
  scenario_duplicate_review_explainer_ scenario_duplicate_review_burst_pair_reason \
  scenario_duplicate_review_different_frames_reason
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p photosquirrel-cli -- --test-threads=4 \
  scenario_cli_suggestion_json_carries_the_explanation scenario_cli_explain_prints_the_reason
cargo run --release -p photosquirrel-cli -- groups suggestions --json   # on the scanned corpus
cargo xtask screenshot -- --theme dark    # Similar shots tab with the explainer, and a row tooltip
cargo xtask ci
openspec validate similar-shot-explained --strict
```
