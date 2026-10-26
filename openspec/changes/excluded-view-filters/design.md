# Design

## Context

- `ps-service::library::excluded()` runs `id_list(excluded_filter())` and then, for every id, loads the details and
  the path, and builds `Vec<ExcludedRow>`. The `ExcludedView` in `ui-workspace/src/views/library.rs` draws that Vec
  in a `uniform_list` with columns for file, class and reasons. It has no filter, search, sort, thumbnail or bulk
  action.
- Exclusion today has two parts. `file.class` and `file.reasons` (a JSON array of `ClassReason`) hold the
  classifier's result. `user_inclusion (file_id, decision, reason, at)` holds the user's include or exclude (migration
  7). The excluded set is the classes ThumbnailCache, Graphic and Junk, plus decisions of `exclude`, minus decisions
  of `include`.
- An accepted AI suggestion records `user_inclusion(decision='exclude', reason='<label> (AI suggestion)')`
  (`insights.rs::AiSuggestion::exclusion_reason`). Today the only sign that the AI made it is that text.
- Junk mixes three different things. `classify_facts` returns Junk for an unrecognised format (`Unsupported`), for a
  recognised format whose probe failed and gave no dimensions (`ProbeFailed`), and for files that are too small or
  too short (`ShortSideBelow`, `PixelsBelow`, `VideoTooShort`, `VideoTooSmall`).
- The Library grid (`ui-gallery`) keeps the whole id list (8 bytes per id) and reads details for visible cells, a
  page at a time. 1M ids is 8 MB, which is fine.
- Migrations 19 (`0019_decoder_missing.sql`) and 20 (`0020_decode_failure.sql`, ADR-0098) belong to `heic-system-decoders`. This change takes 21.

## Goals / Non-Goals

**Goals:** let the user see, at a glance, what was excluded and why; check the AI's exclusions; undo them in bulk;
hide files PhotoSquirrel cannot read at all, unless asked; stay fast at 1M files.

**Non-goals:** changing the classifier's rules; changing which files are excluded; new exclusion reasons;
placeholder art for files with no preview (that is `preview-placeholders`).

## Decisions

### D1. One stored category per file (new ADR "Exclusion categories")

`ps-model::ExclusionCategory`, in this priority order (the first that applies wins):

| key | label (chip) | rule |
|---|---|---|
| `you` | Excluded by you | `user_inclusion.decision='exclude'` and `origin='user'` |
| `ai` (+ `ai_kind`) | AI suggestions: Screenshots / Documents / Graphics / Not a photo | an exclude decision with `origin='ai'` |
| `cache` | Thumbnail caches | class ThumbnailCache |
| `graphic` | Graphics | class Graphic |
| `small` | Too small | class Junk with a size or video-length reason |
| `unreadable` | Could not be read | class Junk, a recognised format, and `ProbeFailed` with no dimensions |
| `unsupported` | Not a supported file | class Junk with `Unsupported` (no recognised format) |

- An include decision always wins. Such a file has no category and is not excluded.
- Every reason stays in `file.reasons`. The category is only a filing key.
- `unreadable` is listed separately from `unsupported` on purpose. A TIFF or HEIC that could not be read is a photo
  the user may care about, so it is shown by default. `preview-placeholders` explains it.
- **Storage (migration 21, `0021_exclusion_category.sql`):**
  - `ALTER TABLE file ADD COLUMN exclusion TEXT` holds the classifier's category (`cache`, `graphic`, `small`,
    `unreadable`, `unsupported`, or NULL). It is written in the same statement as `class` and `reasons`: on
    analysis, on re-classification (ADR-0051), and on refresh.
  - `ALTER TABLE user_inclusion ADD COLUMN origin TEXT NOT NULL DEFAULT 'user' CHECK (origin IN ('user','ai'))` and
    `ADD COLUMN ai_kind TEXT` (`screenshot`, `document`, `graphic`, `not_photo`).
  - Backfill:
    - Map `class` to `exclusion`. For Junk, use `json_extract` over `reasons`: `$[0].kind` (or the serde tag form in
      use; check this against stored rows) of `unsupported`, `probe_failed` together with `format IS NOT NULL`, or
      size and video kinds.
    - Map `reason LIKE '% (AI suggestion)'` to `origin='ai'`, with `ai_kind` taken from the label before the
      bracket. "not a photo" becomes `not_photo`.
    - It is SQL only, reads no file bytes, and runs in one transaction.
  - Index: `CREATE INDEX file_exclusion ON file (exclusion, id) WHERE exclusion IS NOT NULL`.
- The effective category (the decision first, then `file.exclusion`) is one SQL expression in `ps-catalog`, used by
  both the filter and the counts. The CLI and the app use the same helper.

### D2. Query API

- `Filter` gains `exclusion: Option<ExclusionSelection>`: a set of categories with AI kinds, plus a `text` match.
  - `id_list` with that filter and `Sort` by name, folder, size or category gives the ids.
  - The text match is `instr(lower(name || '/' || folder_path), lower(?))`. Folders are joined through the existing
    folder table.
  - At 1M files this is a scan of only the excluded rows, through the partial index. It is measured by the budget in
    D6.
- `exclusion_counts(&Filter-without-categories) -> Vec<(ExclusionCategory, Option<AiKind>, u64)>` is one
  `GROUP BY` query. The counts honour the text search, so the chips show what the search would show.
- `ps-service`:
  - `excluded_page(ids) -> Vec<ExcludedRow>` returns the details for visible rows only.
  - `ExcludedRow` gains `category` and `thumb`.
  - The old all-rows `excluded()` is removed.
  - The CLI's `report` keeps its output and gains `report excluded [--category k ...] [--json]`, through the same
    query.

### D3. The view

- A toolbar row holds:
  - the chips, each "Label · N", with the AI chip opening a menu of its kinds with their own counts
  - "N hidden by filters · Show all" when anything is hidden
  - a search box (debounced 150 ms)
  - a sort menu
  - a Grid/List segmented toggle
- ui-theme tokens only (ADR-0028). The toolbar wraps without stray dividers, following the gallery rule.
- **List mode:** a `uniform_list` over the id list, with details read for visible rows (page cache as in the grid).
  The columns are thumbnail (token `thumb_sm`), name and folder (shortened in the middle, as today), category, and
  reasons.
- **Grid mode:** the `ui-gallery` grid with the excluded filter as its query, so placeholders, selection and the
  inspector come for free.
- The view state (chips, sort, mode) goes into `persist.rs` under the catalog's key. Defaults: every chip on except
  `unsupported`, sort by name, List mode.
- Live counts: the view subscribes to the same catalog-changed notification the Library uses (ADR-0019 sampled, not
  per file) and re-runs the counts and the id list.

### D4. Put back

- **"Put back in Library"** acts on the selection (toolbar button, context menu, and the inspector's existing
  Include). **"Put back all shown…"** opens a confirmation with the count and the active filter description.
- Both call `LibraryService::set_decisions(ids, Include, origin=user)`, one batched transaction per 10,000 ids. A
  progress toast appears above 10,000.
- For a file with `origin='ai'`, the open or accepted suggestion row is set to `rejected` in the same transaction.
  The AI queue does not re-suggest a rejected suggestion for unchanged content. This already holds for rejections,
  and a test proves it holds for put-backs too.
- Undo: excluding again works as today. No separate undo journal is needed, because decisions are reversible data.

### D5. AI accept stores provenance

`insights.rs` accept (single and bulk) writes `origin='ai', ai_kind`. The visible reason text stays the same
("excluded by you: screenshot (AI suggestion)").

### D6. Budget (new ADR, `perf-budgets.toml`)

- `excluded-view-open` is measured by a criterion bench in `ps-catalog`. It times `exclusion_counts` plus `id_list`
  plus the first 200-row `details` page, on a generated 1M-row catalog with 200,000 excluded (mixed categories, 20,000
  AI decisions).
- Starting bound: 150 ms on the dev machine, with a relative regression check in CI like the other catalog budgets.
- It reuses the 1M-row catalog builder from the gallery benches.

## Risks / Trade-offs

- [The stored category goes stale if a write path forgets it] -> it is set in the one statement that writes `class`
  and `reasons`. A property test re-derives the category from `class` and `reasons` for every row of the scanned
  corpus after scan, re-classify and refresh, and compares.
- [A text search over 200k rows] -> it is bounded by the budget. If it is over budget, the fallback is an FTS table,
  decided at build time and recorded in the ADR.
- [The backfill parses free text] -> only the exact suffix " (AI suggestion)" written by `exclusion_reason` is
  matched. Anything else stays `origin='user'`, which is the safe default because it is still shown.

## Testing

- `ps-catalog`: migration test on a catalog fixture written by the current schema, category maintenance tests, count
  and filter tests, the bench.
- `ps-service`: put-back and AI-rejection tests on the small corpus.
- `ui-workspace`: headless `#[gpui_kit::test]` tests for chips, defaults, hidden count, search, sort, modes,
  persistence, live counts and confirmation.
- CLI: `report excluded --category` test.
- Screenshot review of the Excluded view in List and Grid modes, light and dark.
