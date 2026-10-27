# 0111. Video placeholder rows of older catalogs are marked from the thumbnail store at open

- Status: accepted
- Date: 2026-10-27
- Deciders: maintainers
- Amends: [0109](0109-placeholders-carry-a-reason-and-a-decoder-generation.md) (migration 24 left videos unmarked)

## Context
ADR-0109's migration 24 marks the placeholder rows of older catalogs from the file row alone: a thumbed still
without perceptual hashes. Videos have no perceptual hashes with or without a poster frame, so their rows were left
unmarked and "take the reason from the stored placeholder record". But the grid's badge, the No preview filter, the
counts and the "Why no preview?" button all read the row (`file.preview_reason`), never `thumbs.db`. 0.2.0 made a
placeholder for every video when no poster-frame tool (ffmpeg, optional) was set up, so those videos stayed plain
blank tiles with no way to learn why. The thumbnail kind lives in `thumbs.db`, a separate database (ADR-0016), so a
catalog migration cannot read it.

## Decision
- **Catalog migration 25** (`0025_video_placeholders.sql`) adds `catalog_task (name TEXT PRIMARY KEY)`, one-time
  tasks left for the engine, and records `video_placeholder_reasons` when the catalog has a thumbed video row
  (`stage = 3 AND media_kind = 1 AND content_id IS NOT NULL`) without a placeholder reason. A new catalog records
  nothing.
- **The owner does the task at open** (`ps_scan::Scanner::with_catalog`, never an observer): for each such row whose
  stored thumbnail kind is `placeholder` (a lookup by content id; only those records are read in full), it writes
  the placeholder's generation and reason (`Command::SetPreview`), "no poster-frame tool" when the record has none
  (0.2.0), then removes the task's row (`Command::FinishCatalogTask`). Videos with a poster frame are left as they
  are. A failure is logged and the task is tried again at the next open.
- The rows then behave as videos catalogued by this version: the tile shows the placeholder with the "needs a
  poster-frame tool" mark and the info button, the No preview filter and the counts list them, and the dialog shows
  the poster-frame tool's guidance. The waiting rule is unchanged: the video poster generation is 0, so they are not
  offered for Fill in previews.

## Alternatives considered
- **Edit migration 24 in place** (it is unreleased) - SQL still cannot see `thumbs.db`, and catalogs of development
  builds already at 24 would never be marked.
- **ATTACH `thumbs.db` in the migration** - the migration runner knows one database; a missing or newer thumbnail
  store would fail the catalog's migration.
- **Read the thumbnail kind for every summary page** - the badge would differ from the filter and the counts (SQL),
  and every page of 512 would pay a second store query.
- **Mark on every open without a task row** - a library with many videos would scan them all on every start.

## Consequences
- A catalog of 0.2.0 shows its videos without a poster frame as explained placeholders after its first open.
- `catalog_task` is there for later one-time tasks that need both databases.

## Revisit when
Another one-time task needs both databases, or the thumbnail store moves into the catalog.
