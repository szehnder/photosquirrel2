# Design

## Context

- `ps-service::review` builds `ReviewList` in one pass over `dup_group` / `dup_member` / `file` (`load_groups`).
  It already carries each member's `content_id` digest (`digest_of`). `ReviewFilter` is a set of booleans checked
  per `ReviewRow`.
- `ps-service::insights` stores AI results per content identity (`ai-queue` "One result per content"). The user's
  corrections live in the interim JSON store (ADR-0042), also keyed by content. `effective(...)` merges the two, and
  `InsightFilter::keeps(&InsightItem)` is the gallery and AI Review predicate. `stamp(...)` is the value the live
  views poll (ADR-0019).
- By default the AI queue describes keepers only, photos first (`ai-queue` "Persisted queue order"). So a group's
  match usually rests on its keeper, and on other members only when the user opted to describe non-keepers.
- The Excluded view keeps its chips in a small JSON file in the catalog's data directory (`persist.rs`, ADR-0102).
  The Duplicates view's filter is not persisted today.

## Goals / Non-Goals

**Goals:**
- One predicate for "does this photo match", shared by the gallery, AI Review, the Duplicates list and the CLI.
  The Duplicates list must never disagree with the AI Review view on the same filter.
- The filtered list costs one extra keyed read of insights for the groups' contents, off the UI thread.

**Non-Goals:**
- A new insight index or catalog table.
- Putting a group's match into the catalog. The match is computed when the list is built.

## Decisions

### D1. `ReviewFilter` gains `insight: Option<InsightFilter>` and `undescribed: bool`

`ReviewFilter::keeps` gets one more clause. For a row with an insight filter, look up each member content's
effective insight and keep the row if `InsightFilter::keeps` holds for any of them. `undescribed` keeps rows where
no member content has an insight. Building an `InsightItem` for this means no new matching logic, so the Duplicates
list and AI Review cannot drift apart. Its `suggestions` field (open AI exclusion suggestions) is not offered in the
Duplicates UI and is ignored there.

*Loading:* when `insight` or `undescribed` is set, `groups(...)` collects the distinct member contents from
`digest_of` and reads their effective insights in one batch with `effective(...)`, chunked by content id like the
existing keyed reads. Without an AI filter, nothing extra is read.

*Counts:* `ReviewList` gains `matching: u64` and `matching_reviewed: u64`, which count the rows kept by the AI part
of the filter alone. That way "Matching: 2 of 5 reviewed" does not shrink when the "unreviewed" chip is also set.
`groups` and `reviewed` keep their meaning: every group.

*Alternative:* match on the keeper only. Rejected because, with "include non-keepers" on, a group whose resized
copy was described as "pets" but whose keeper wasn't described yet would be hidden for no visible reason.

### D2. Live refresh by polling the insight stamp (ADR-0019)

While an AI filter is set and the Duplicates view is visible, the view polls `insights::stamp` on the same 500 ms
cadence as AI Review. When it changes, the view reloads the list on the background executor and keeps the
selection the way the existing reload does (`want.filter(...)`). With no AI filter set, the view does not poll.

### D3. Persist the Duplicates view's filter like the Excluded view's (ADR-0102 pattern)

Add `DuplicatesViewState { filter: ReviewFilter, sort }` to `persist.rs`, saved next to the Excluded view's state
in the catalog's data directory. It is written when the filter changes and read when the view is built. If the
file is missing or unreadable, the view uses the defaults and logs a warning. It is per catalog, because insights
are.

### D4. "Review duplicates of these" carries the filter, not a list of ids

The AI Review view dispatches `OpenDuplicatesFiltered(InsightFilter)`. The shell switches view and calls
`DuplicatesView::set_filter` with `insight: Some(filter)` and the other chips cleared. Passing the filter, not
the visible ids, keeps the Duplicates list live (D2) and lets the user change it afterwards.

### D5. CLI flags mirror the filter (ADR-0027)

`groups list` gets `--category <c>`, `--people`, `--children`, `--pets`, `--pet-kind <k>`, `--people-min <n>`,
`--people-max <n>`, `--undescribed` and `--unreviewed`, defined with clap derive as the CLI already does. These map
one-to-one onto `ReviewFilter`. The JSON output adds `matching` and `matching_reviewed`.

### D6. Header layout

An "AI" dropdown button next to Sort opens a menu: categories, "Contains people / children / pets", pet kinds
seen in the catalog, people count, and "Not described yet". Each active AI filter shows as a removable chip in the
existing wrapping chip group (`docs/gpui-notes.md`: chips in a `flex_1().min_w_0().flex_wrap()` group). Tokens
only (ADR-0028).

## Risks / Trade-offs

- [Reading insights for every group's contents on a 1M-file catalog could make the filtered list slow to open] →
  Measure `groups(filter)` with an AI filter on the 1M bench catalog, with insights seeded with `seed_demo`. If p95
  is above the Excluded view's 150 ms (`excluded-view-open`), propose a `duplicates-ai-filter-open` budget and its
  ADR in this change, rather than shipping a slow filter.
- [The AI has described only keepers, so a group can match on a keeper the user then overrides] → The match uses
  every member with an insight. After an override, the old keeper still has its insight, so the group keeps
  matching. This is the any-member rule working as intended.
- [Polling while filtered costs a little CPU] → Only while an AI filter is set and the view is visible. `stamp` is
  a constant-time read.

## Migration Plan

No catalog migration. The new `persist.rs` file is optional: without it the view starts unfiltered. Rolling back
leaves an unused small JSON file.

## Verification

```
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ps-service -- --test-threads=4 review::
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ui-workspace --test duplicates -- --test-threads=4 \
  scenario_duplicate_review_filter_groups_with_children scenario_duplicate_review_groups_not_described_yet \
  scenario_duplicate_review_matching_progress_shown scenario_duplicate_review_review_advances_within_the_filter \
  scenario_duplicate_review_newly_described_group_joins_the_list
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ui-workspace --test ai_review -- --test-threads=4 \
  scenario_ai_review_review_duplicates_of_filtered_photos
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p photosquirrel-cli -- --test-threads=4 scenario_cli_cli_lists_
cargo run --release -p photosquirrel-cli -- groups list --pets --json    # on a seeded corpus catalog
cargo xtask screenshot -- --theme dark    # Duplicates header with AI chips and the Matching line
cargo xtask ci
openspec validate ai-filtered-duplicates --strict
```
