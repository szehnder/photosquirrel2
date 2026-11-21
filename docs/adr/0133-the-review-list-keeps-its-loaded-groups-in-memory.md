# 0133. The review list keeps its loaded groups in memory; a reload re-filters them

- Status: proposed
- Date: 2026-11-21
- Deciders: maintainers

## Context
The Duplicates list is built by `review::list`: one pass over every group and member (`load_groups`), the open
similar-shot suggestions per group, then the review state, the filter and the sort. It runs on every load: when the
view opens, after every **R** (mark reviewed), after every keeper or merge edit, and on every filter change. With the
AI filters of `ai-filtered-duplicates` it also runs whenever an AI insight is stored while a filter is set.

Measured on a 1M-file catalog with 100,000 groups of 3 (release build, `ps-service` `tests/review.rs`):
- the list without a filter: 614-634 ms median, almost all of it the catalog pass;
- the gallery's insight freshness check (`insights::stamp`), which an AI filter needs: 235 ms, almost all of it
  `count(*)` over `ai_result`;
- the AI filter itself, with its file sets ready: about 40 ms.

So a reviewer on a large catalog waits about 0.6 s after each **R**, and 0.9 s with an AI filter set. None of it
blocks the UI thread (the list loads on the background executor), but the next group's list position and the
progress figures arrive late.

## Decision
The app keeps the last loaded groups in memory (a `ReviewCache` owned by the review source) under a `GroupsKey`: one
cheap query over the catalog (group count and keeper-id and reasons-length sums, member count and member-id sum,
suggestion count and pair-id sum, scan-run counters) plus the review store's dismissal revision. A load with the
same key reuses the groups and recomputes only the review state, the AI match, the filter and the sort. Any change
to the groups (a scan, a regroup, a keeper override, a merge, a separation, a dismissed suggestion) changes the key,
and the next load reads the catalog again. Review progress (`review-progress-anchor`) comes from the same cache.

The insight stamp keeps its `results` count, but `count(*)` is recomputed only when `max(seq)` or `max(at)` of
`ai_result` changed. No code path deletes an AI result, and every write takes the next `seq` (migration 11), so the
count cannot change while both maxima stay the same.

A new budget, `duplicates-list-reload` (median <= 150 ms), measures a reload after a group is marked reviewed and a
reload with an AI filter, on 1M files with 100,000 groups and an insight per keeper.

## Alternatives considered
- **Make `load_groups` faster** (narrower columns, a covering index): the pass is mostly row visits over 300,000
  members joined to `file`. At best it halves, and every reload still pays it.
- **Update the cached rows in place on each edit** (flip `reviewed` on R): faster still, but every edit path would
  have to know the cache. A key that changes with the data is harder to get wrong.
- **Drop `results` from the insight stamp:** the gallery's incremental index update uses it to detect missed
  rows; changing that logic is out of this change's scope.

## Consequences
- Reloads that do not change the groups cost the key query (~10 ms) and an in-memory pass (~tens of ms) on 100,000
  groups. The first load is unchanged.
- The cache holds one copy of the loaded groups: about 100 bytes per member plus names, ~40 MB for 100,000 groups of 3.
  It is dropped with the review source, when the catalog closes.
- A change the key does not see would show a stale list until the next scan. The key covers every column the list
  shows. A test edits each one (review, keeper override, merge, separate, dismiss, rescan) and checks the reload.
- The CLI has no cache. Each command lists once.

## Revisit when
Groups gain a column the list shows that the key does not cover, or `ai_result` rows can be deleted.
