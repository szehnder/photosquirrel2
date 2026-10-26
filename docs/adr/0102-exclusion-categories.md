# 0102. Exclusion categories: one stored filing key per excluded file

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0015](0015-photo-classifier.md) (how excluded files are filed, not which files are excluded),
  [0023](0023-disposition-ledger.md) (user decisions record where they came from)

## Context
After the owner accepted every AI exclusion suggestion, the Excluded view was one long list: files PhotoSquirrel
cannot read at all sat between AI-excluded documents, icons, thumbnail caches and the user's own exclusions, and
nothing told them apart. The only sign that a decision came from the AI was its reason text ("document (AI
suggestion)"). Junk mixes three different things: an unrecognised format, a recognised format whose probe failed,
and files that are too small or too short. Filtering and counting by any of this in the UI would mean reading every
excluded row with its reasons, which does not scale to a million files.

## Decision
- **One category per excluded file, in priority order** (the first that applies wins): excluded by you (`you`), an
  accepted AI suggestion (`ai`, with its kind `screenshot`, `document`, `graphic` or `not_photo`), thumbnail cache
  (`cache`), graphic (`graphic`), too small (`small`), could not be read (`unreadable`: junk of a recognised format
  whose probe failed) and not a supported file (`unsupported`: junk of no recognised format). An include decision
  means no category. The category is a filing key only: every reason stays in `file.reasons`.
- **Storage (catalog migration 21):** `file.exclusion` holds the classifier's category and is written in the same
  statement that writes `class` and `reasons` (probe, re-classification, analysis refresh, the full insert) and
  cleared with them when a file changes. Stored reasons are display texts, so the rule reads the class code, whether
  the format was recognised and whether the first reason starts with `probe failed:` (`ExclusionCategory::of` in
  Rust, `writer::exclusion_sql` in SQL, the same rule). `user_inclusion.origin` (`user` or `ai`) and
  `user_inclusion.ai_kind` record where a decision came from; accepting an AI suggestion writes them.
- **Backfill:** migration 21 fills `file.exclusion` from the stored class, format and reasons, and marks as `ai`
  only decisions whose reason is exactly `<label> (AI suggestion)` with one of the four labels. It is SQL in the
  migration's transaction and reads no file. Anything else stays `user`, which is still listed.
- **Queries:** the effective category is the exclude decision's origin, else `file.exclusion`. One streaming pass
  over the excluded set (the classifier-excluded rows through a partial covering index
  `file_exclusion (exclusion, id, kflags, source_id, size, name, rel_path) WHERE exclusion IS NOT NULL`, plus the rows
  with an exclude decision) yields every chip's count and the selection's ordered ID list; the counts and the sorts
  are done in Rust, not by SQL `GROUP BY` and `ORDER BY` ([ADR-0103](0103-excluded-view-budget.md)). The search text
  matches the path relative to the source root (name and folders), ASCII case-insensitively.
- **Put back** records the user's own include decision, in transactions of 10,000 files. For a file excluded by
  an accepted AI suggestion, the suggestion is marked rejected in the AI review's decision store, so the AI queue
  does not offer it again for the unchanged content. The inspector's Include is the same operation.

## Alternatives considered
- **Derive the category at query time from `reasons`** - every count would parse JSON of every excluded row; the
  stored key makes it one indexed pass.
- **Parse "(AI suggestion)" from the reason text at query time** - fragile and unindexable; stored provenance is
  explicit, and the text match is confined to the one-off backfill.
- **Several categories per file** - chips would double-count and "put back all shown" would be ambiguous; reasons
  already carry the full story.
- **Store the rejection in the catalog with the decision** - the AI review keeps its suggestion decisions in its
  own store (`insights.json`); moving them is a separate change. The rejection is written right after the include
  decisions commit.

## Consequences
- The Excluded view filters, counts, sorts and searches without reading reasons; the CLI's `report excluded`
  uses the same query.
- A write path that sets `class` must set `exclusion` in the same statement; a property test re-derives it for
  every row of a scanned corpus after a scan, a re-classification and an analysis refresh.
- A real file inside a cache folder that is also a graphic carries only the cache reason (the classifier stops at
  its first rule); the category does not change that.

## Revisit when
The classifier gains reasons that do not fit these categories, reasons are stored as structured data (then read the
kind instead of the text prefix), or AI suggestion decisions move into the catalog (then reject in the same
transaction).
