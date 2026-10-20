# Proposal

## Why

Before exporting, the user wants to see and trust what the engine merged: every group, every copy side by side,
why each copy joined and why the keeper won, with a fast way to correct the keeper or pull out a wrongly merged
photo. Burst and similar shots must be visible as suggestions without ever being collapsed.

## What Changes

- Duplicates view: group list (sortable by reclaimable bytes, member count, date; filters: exact only, has
  derivatives, unreviewed, overridden, has suggestions) with review progress.
- Group compare: members side by side with thumbnails, dimensions, size, format, quality, capture date, source and
  path, join reason and distances, keeper badge and a "Why this copy" panel (ADR-0013, ADR-0014).
- Actions: choose keeper (click or number keys), separate a member ("not a duplicate", ADR-0033 proposed in
  `duplicate-groups`), mark reviewed; keyboard flow (J/K next/previous group).
- Manual merge ("Merge with…"): an escape hatch when automatic matching misses a pair. It changes no files,
  is stored as an ADR-0033 "always pair" constraint that regrouping respects, and can be undone.
- RAW+JPEG keeper sets shown as a pair.
- Similar-shot suggestions panel: pairs shown side by side, dismissable, never merged.

## Capabilities

### New Capabilities
- `duplicate-review`: reviewing duplicate groups and similar-shot suggestions and correcting the engine's choices.

### Modified Capabilities
- None.

## Impact

- ADRs: 0013, 0014, 0018, 0021, 0027, 0028, and ADR-0033 (proposed in `duplicate-groups`).
- Crates: `ui-workspace` (Duplicates view, compare panel, suggestions panel), `ui-gallery` (thumbnail/texture
  reuse, compare loupe), `ps-service` (group list queries, review state, suggestion dismissal), `ps-catalog`
  (migration: `reviewed_at`, `suggestion.dismissed_at`).

## Out of scope

- Accepting a similar-shot suggestion as a merge (bursts stay suggestions, ADR-0013); bulk auto-review rules; similar-shot browsing
  beyond pairs (later `similar-shots`).
