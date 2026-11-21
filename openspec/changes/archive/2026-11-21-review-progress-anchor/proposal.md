# Proposal

## Why

Testing 0.4.0-rc.4, the owner reported three problems with reviewing duplicates:

- **The "In this group" lines in "Why this copy" often run past the panel's edge.** Longer lines such as
  "keepers (RAW+JPEG pair): export takes both together" are cut off at the panel border instead of wrapping.
- **Review progress is hard to see.** "12 of 40 reviewed" and its bar are small, muted text in the list header,
  so the owner did not notice them at first. On a screen whose main job is working through a backlog, the size of
  that backlog should be the first thing the eye lands on.
- **The sidebar's Duplicates count never goes down.** It shows every group, so after an hour of review it reads
  the same as before. The owner wants it to show the work left: groups minus groups reviewed.

## What Changes

- **"In this group" lines wrap inside the panel.** Every composition line stays inside the "Why this copy" panel at
  every window width and UI font size. A long description wraps onto further lines under its own text, and the
  count column stays aligned. The same applies to the keeper reasons above it.
- **A review progress card.** The Duplicates list header shows progress as a card on a tinted surface (theme
  tokens, light and dark). Its main number is the groups **left to review** ("28 left"). Below it are "12 of 40
  reviewed" and the progress bar. When every group is reviewed, the card switches to a success state ("All 40
  groups reviewed"). The reclaimable-bytes and similar-shots line stays below the card, unchanged.
- **The sidebar counts groups left to review.** The Duplicates item's count is groups not yet reviewed. Its tooltip
  gives both numbers ("28 left to review of 40 groups"). When none are left, the count is replaced by a check mark
  and the item stays visible. The count updates when a group is marked reviewed or unreviewed and after a
  regroup, without reopening the view.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `duplicate-review`: adds the requirements "Group composition fits its panel" and "Review progress card".
- `app-shell`: adds the requirement "Duplicates count shows groups left to review".

## Impact

- **UI:** the Duplicates view's "Why this copy" panel and list header; the navigator's count for Duplicates.
- **Service:** a review-progress query (groups and groups reviewed) that reads the same review state as the
  Duplicates list (ADR-0042), so the two numbers always agree. It runs after the start-up summary, not as part of
  it.
- **No catalog migration, no settings change, no file I/O, no new dependency.**
- **ADRs relied on:** ADR-0028 (design tokens only, no raw colours or sizes), ADR-0042 (where review state is
  stored, and why it is keyed by group membership), ADR-0019 (sampled snapshots: the count refreshes with the
  existing summary refresh, not with one event per change).

### Out of scope

- AI filters on the Duplicates list and progress for a filtered set (change `ai-filtered-duplicates`).
- Explaining similar shots (change `similar-shot-explained`).
- Scenes and the "Best of" mode, which picks the best shot among similar shots (not yet proposed).
- Counts for other sidebar items (Library, Excluded, Sources) stay as they are.
