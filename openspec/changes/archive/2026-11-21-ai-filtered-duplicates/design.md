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

### D1. `ReviewFilter` gains `insight: Option<InsightCriteria>` and `undescribed: bool`, matched through a `GroupMatcher`

The AI part of the filter is the gallery's own `InsightCriteria` (category, children, pets, people), made `Copy`
and serialisable. The owner chose (2026-11-20) to offer exactly the AI filters the gallery and AI Review offer.
`review::list_with(svc, store, filter, sort, &dyn GroupMatcher)` decides each group's match from its member files:
- `matches(files)`: true when any member file's effective insight meets the criteria;
- `described(files)`: true when any member file has an insight (for "Not described yet").

There are two matchers:
- **The app** uses the gallery's insight index (`Library::insight_matches`, the same effective-insight facets the
  gallery filters on, built once and updated in place while the AI queue runs). `CatalogReview::groups` asks for
  the `ItemSet` of the criteria and of the empty criteria ("described"), and membership is a binary search per
  member file. The Duplicates list and the gallery therefore cannot disagree on the same filter.
- **The CLI** (and `review::list` without an index) uses a lookup matcher: `insights::effective` per member
  content, cached per content for the call. It also takes `--pet-kind` and `--people-min/--people-max`, applied
  with the AI Review predicate (`InsightFilter::keeps`).

*Counts:* `ReviewList` gains `matching` and `matching_reviewed`, which count the rows kept by the AI part of the
filter alone. That way "Matching: 2 of 5 reviewed" does not shrink when the "unreviewed" chip is also set.
`groups` and `reviewed` keep their meaning: every group.

*Alternative:* read each group's insights per call (the first draft of this design). Rejected because on 1M
files it repeats the 2.4 s pass over every result that the gallery's index already pays once.

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

An "AI" dropdown button next to Sort opens a menu: every category, "Contains people", "Contains children",
"Contains pets", and "Not described yet". Each active AI filter shows as a removable chip in the existing wrapping
chip group (`docs/gpui-notes.md`: chips in a `flex_1().min_w_0().flex_wrap()` group). Tokens only (ADR-0028).

### D7. A review cache: reloads re-filter the loaded groups (ADR-0133)

`ps-service::review::ReviewCache` holds the last `load_groups` result. That is each group's members with their
digests, precomputed membership keys, keeper name, thumbnail and capture time, and the open suggestion counts per
group. It is stored under a `GroupsKey`: one query over `dup_group`, `dup_member`, `dup_suggestion` and `scan_run`
(counts and id sums), plus the review store's dismissal revision. `ReviewCache::list(svc, store, filter, sort,
matcher)` reuses the cached groups when the key matches. Otherwise it reloads them. In both cases it then applies
the review state, the AI match, the filter and the sort in memory. `ReviewCache::progress` replaces the
`ProgressKey` cache that `review-progress-anchor` added: progress is the same pass without rows.

`CatalogReview` owns one `ReviewCache`. The CLI keeps calling `review::list`, uncached, once per command.

*Key coverage:* every column the list shows comes from a table the key covers. Keeper overrides change
`dup_group.keeper_file_id` and `reasons`, merges and separations change members, dismissals change the store's
dismissal revision, and scans change the run counters. A test makes each edit and checks that the reload shows it.

### D8. The insight stamp recounts results only when they changed (ADR-0133)

`insights::stamp` still returns `results`, but `InsightStore` remembers the last `(max(seq), max(at), count)`. It
runs `count(*)` again only when either maximum moved. No code deletes `ai_result` rows, and every write takes the
next `seq`, so this is exact. On 1M results the stamp drops from 235 ms to the cost of two indexed `max` lookups.

### D9. `Library::insight_matches_many`

The Duplicates list needs two file sets per load: its criteria, and "any insight". One call checks freshness once
and answers both. The index keeps its last two answers, so alternating between the two never rebuilds a set.

## Risks / Trade-offs

- [The first AI-filtered list builds the gallery's insight index (~2.4 s on 1M, plus 7.7 s the very first time
  in the bench), if the gallery has not already] → It runs on the background executor, and the list header shows
  "Loading…" meanwhile. Later filters reuse the index. Reloads are budgeted (`duplicates-list-reload`, D7); the
  first load is not.
- [The review cache holds the loaded groups in memory] → About 40 MB for 100,000 groups of 3. It is freed with the
  catalog.
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
