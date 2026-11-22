# Design

## Context

- **The overflow.** In the Duplicates view's "Why this copy" panel, each "In this group" row is a horizontal flex
  row: a fixed-width count column (`Tokens::size(56.0)`, no shrink) and a description `div` with no `flex_1()` and
  no `min_w_0()`. In GPUI a flex child without `min_w_0()` keeps its content's minimum width, so the text pushes
  past the row instead of wrapping. The keeper-reason rows above already use `flex_1().min_w_0()` and behave
  correctly. See `docs/gpui-notes.md` on flex shrinking.
- **The progress.** `ReviewList` already returns `groups` and `reviewed` for every group, whatever the filter. The
  header renders them as `text_sm` muted text next to the title, with a thin `progress_bar` below.
- **The sidebar count.** `chrome.rs` `nav_count` returns `CatalogSummary::duplicate_groups`. `CatalogSummary` has no
  reviewed figure, and review state lives in the review store (ADR-0042), keyed by group membership.

## Goals / Non-Goals

**Goals:**
- The navigator, the progress card and the list header use one source for "reviewed", so they never disagree.
- No extra catalog query on the UI thread. The summary is still built on the background executor.

**Non-Goals:**
- Moving review state from the JSON store into the catalog (still ADR-0042's interim store).
- Per-filter progress. That comes with `ai-filtered-duplicates`, which adds a "matching" line to this card.

## Decisions

### D1. Fix the composition row the same way as the reason rows

Give the description `div` `flex_1().min_w_0()` and set the row to `items_start()`, so a wrapped description lines
up with the first line of its count. Keep the count column's token width. Don't shrink the count with
`whitespace_nowrap` or an ellipsis: these lines explain the group, so cutting them would hide information that the
"Member badges fit their card" requirement only accepted for short badges.
*Alternative:* move the count into the text ("1 keeper: ..."). Rejected because the right-aligned count column is
what makes the composition easy to scan.

### D2. A progress card, tokens only (ADR-0028)

The card is a `v_flex` with `colors.accent_soft` as background, a `Tokens::RADIUS_MD` corner and
`Tokens::space(...)` padding. Its main figure ("28 left") is `text_xl` semibold in `colors.accent_text`. Below it,
"12 of 40 reviewed" is `text_sm` muted, then the existing `progress_bar`. With 0 left, the card uses the
success tone's existing tint, `Colors::chip_bg(colors.success)`, with `tone_text(colors.success)` and a check icon. Its
contrast is already checked by the theme's status-chip test, so no new palette token is needed. The card has test ids
`duplicates-progress-card`, `duplicates-progress-left` and `duplicates-progress` (the existing id, kept for the
"N of M reviewed" text so the current tests still find it).

### D3. A shared review-progress value, published by the view and the shell

`ps-service::review::progress(svc, store)` returns `ReviewProgress { groups, reviewed }`. It uses the same
`load_groups` pass and the same membership-key rule as `ReviewList::reviewed`, without the suggestion pass.
`ReviewSource::progress()` exposes it, using the review source's single `ReviewStore`, so the navigator and the
Duplicates view cannot read two copies of the store.

The value lives in a GPUI global, `DuplicatesProgress`:
- the Duplicates view publishes it after every list load (open, mark reviewed or unreviewed, keeper and merge
  edits);
- the shell publishes it after each summary refresh (start-up, after a scan or a regroup), on the background
  executor, *after* the summary arrives. That keeps the extra pass over the groups out of the start-up summary
  and its budget.

The shell observes the global and re-renders. The navigator shows `groups - reviewed`, a check icon when
`groups > 0` and none is left, and the tooltip "N left to review of M groups". Until the first value arrives,
it falls back to the summary's group total. No per-group event is added (ADR-0019).

*Alternatives:*
- Add `duplicates_reviewed` to `CatalogSummary`. Rejected because the summary is waited for before the first
  frame, so an extra pass over the groups would count against start-up time.
- The navigator subscribes to the Duplicates view. Rejected because the view may never have been opened.

## Risks / Trade-offs

- [Computing the reviewed count on every summary refresh costs a pass over the groups] → It reuses the review list's
  one-pass `load_groups`, which already runs whenever the Duplicates view opens. On the 1M bench catalog, measure the
  summary refresh before and after. If it grows by more than 20 ms, cache the reviewed count by the review store's
  revision and the catalog's group stamp.
- [A wrapped line makes the "Why this copy" panel taller] → The panel already scrolls. Check it in the screenshot
  review at the narrowest width.

## Migration Plan

No data migration. The summary struct is internal to the UI crate.

## Verification

```
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ui-workspace --test duplicates -- --test-threads=4 \
  scenario_duplicate_review_pair_keeper_composition_wraps_inside_the_panel \
  scenario_duplicate_review_composition_lines_fit_at_the_default_size \
  scenario_duplicate_review_card_shows_groups_left \
  scenario_duplicate_review_card_shows_all_reviewed \
  scenario_duplicate_review_card_ignores_the_list_filter
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ui-workspace -- --test-threads=4 scenario_app_shell_
cargo xtask tokens
cargo xtask screenshot -- --theme dark    # and --theme light: Duplicates view, narrow window, pair group open
cargo xtask ci
openspec validate review-progress-anchor --strict
```
