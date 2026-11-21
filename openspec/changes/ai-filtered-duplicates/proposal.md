# Proposal

## Why

The owner's main use of PhotoSquirrel is pulling the photos they care about (people, their son, pets, travel) out
of a large pile of backups. Today the Duplicates view makes them review **every** group, including groups of icons,
receipts and landscapes they don't care about. The AI already labels photos with category, people, children and
pets, and those labels can filter the gallery and the AI Review view (`ai-review` "Insight filters"), but not the
Duplicates list.

The owner wants a two-step flow that works in either order:

- **Filter, then review:** "show me only the groups with people or pets", then review that smaller set.
- **Review, then filter:** review everything first, then use the AI labels to look again at a subset.

Both need the same thing: the AI insight filters on the Duplicates list, and progress counted for the filtered set.

## What Changes

- **AI filters on the Duplicates list.** A filter control in the Duplicates list header offers the same filters
  as the gallery: category, contains people, contains children, contains pets, pet kind, a people-count range, and
  "Not described yet". They combine with the existing chips (unreviewed, exact only, has derivatives, overridden,
  has suggestions). A group matches when **any** of its files' effective insights (with the user's corrections)
  matches. Usually that is just the keeper, since the AI queue describes keepers first (`ai-queue` "Persisted queue
  order"). "Not described yet" matches groups in which no file has an insight.
- **Progress for the filtered set.** With an AI filter set, the review progress card (from
  `review-progress-anchor`) adds a second line, "Matching: 4 of 12 reviewed". The main figure still counts every
  group. Marking a group reviewed moves to the next unreviewed **matching** group, and J and K stay within the
  matching groups.
- **The list follows the AI queue.** Groups appear in, or drop out of, a filtered list as insights arrive or are
  corrected while the view is open, at most 1 s after storage. The selected group and the scroll position are kept.
- **From AI Review to Duplicates.** The AI Review view gets "Review duplicates of these", which opens the Duplicates
  view with the AI Review view's current insight filter applied.
- **The filter is kept across restarts,** like the Excluded view's chips.
- **CLI parity (ADR-0027).** `groups list` gains `--category`, `--people`, `--children`, `--pets`, `--pet-kind`,
  `--people-min`, `--people-max`, `--undescribed` and `--unreviewed`. With `--json`, the output reports `matching`
  and `matching_reviewed` alongside `groups` and `reviewed`.
- **Advisory only, unchanged.** A filter only hides groups from the list. It never changes a group, a keeper, an
  exclusion or what is exported (ADR-0021).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `duplicate-review`: adds the requirements "AI insight filters on groups", "Filtered review progress" and "Filtered
  list follows new insights".
- `ai-review`: adds the requirement "Open matching duplicates".
- `cli`: adds the requirement "Groups filtered by insight".

## Impact

- **Service:** the review list's filter takes an optional insight filter. A group's match is decided from its
  members' effective insights, read in the same pass that loads the groups.
- **UI:** the Duplicates list header (an AI filter menu and chips), the progress card's "Matching" line, the
  review-flow navigation, the AI Review view's new action, and persistence of the Duplicates view's filter.
- **CLI:** new flags on `groups list`, and new JSON fields.
- **No catalog migration.** Insights and corrections are read where they are stored today (ADR-0042 for
  corrections). No new dependency.
- **Depends on:** `review-progress-anchor` (the progress card this change extends).
- **ADRs relied on:** ADR-0021 (AI output is advisory), ADR-0042 (corrections store), ADR-0019 (the list refreshes
  on the insight stamp, not on per-result events), ADR-0027 (CLI as verification surface), ADR-0028 (tokens).

### Out of scope

- Free-text search over AI descriptions and tags.
- Changing the AI queue's order, for example describing the groups that match a filter first.
- Using AI labels to choose keepers or to mark groups reviewed automatically. The AI stays advisory.
- Scenes and the "Best of" mode, which picks the best shot among similar shots (not yet proposed).
