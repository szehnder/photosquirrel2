# Proposal

## Why

The owner ran the AI classifier on a real library, accepted every exclusion it suggested (screenshots, documents,
graphics), and opened the Excluded view to check them. It was one long list with no filters and no pictures. Files
set aside because PhotoSquirrel cannot read them at all sat between AI-excluded documents, tiny icons, thumbnail
caches and the user's own exclusions. There was no way to see what the AI had excluded, so no way to check it.

ADR-0015 says nothing disappears silently. The Excluded view is where that promise is kept, and today it only lists
the files. It also loads every excluded row, with its path, before it draws anything. At 1M files with a large share
excluded, that is slow and uses a lot of memory.

## What Changes

- **One exclusion category per file**, in a fixed order: excluded by you, an AI suggestion you accepted (screenshot,
  document, graphic, not a photo), thumbnail cache, graphic, too small (icons, tiny images, very short videos),
  could not be read (a photo or video format whose file could not be read here), and not a supported file. The
  catalog stores the category, so the view can filter and count it with SQL.
- **Filter chips with counts** at the top of the Excluded view, one per category. The AI chip has sub-chips for its
  kinds. "Not a supported file" is off by default, and the view says how many files the filters hide. There is also
  a name or folder search box and a sort menu. The chosen filters are remembered per catalog.
- **Pictures:** a Grid mode, which is the Library grid limited to excluded files, and a thumbnail column in the List
  mode. The mode is remembered.
- **Put back in Library**, for the selection or for everything the filters show (confirmed with the count). It works
  the same way as the inspector's Include. Putting back an accepted AI suggestion marks that suggestion rejected, so
  it does not come back after the next AI run.
- **AI provenance is stored, not parsed:** accepted AI exclusions record that they came from the AI and their kind.
  A catalog migration fills these in for existing rows from the stored reason text "<kind> (AI suggestion)".
- **Paged and fast at scale:** the view reads pages of ids like the Library grid does, and gets its counts from one
  aggregate query. A new budget, `excluded-view-open`, covers it, with an ADR.

## Capabilities

### Modified Capabilities
- `gallery`: the Excluded view gains categories, filters, search, sort, Grid and List modes, put back, and paging.
- `ai-review`: an accepted exclusion stores its origin and kind; putting one back rejects the suggestion.
- `catalog`: a migration adds the exclusion category and the decision's origin and kind, and fills them in for
  existing rows.
- `performance-budgets`: the `excluded-view-open` budget.

## Impact

- `ps-model` (`ExclusionCategory`), `ps-catalog` (migration 21, category maintenance, paged excluded queries and
  counts), `ps-service` (`library.rs` excluded API, put back, AI accept), `ui-gallery` (filter in the grid source),
  `ui-workspace` (`views/library.rs` Excluded view, persisted view state), `photosquirrel-cli` (`report excluded`
  with `--category`), `perf-budgets.toml`, and a new ADR.
- This change does not touch files on disk. Putting a file back only records a decision, and that decision is
  undoable.
- **Release:** ships in 0.3.0 together with `tiff-support` and `heic-system-decoders` (owner decision, 2026-10-25). "What's new in 0.3.0" in `packaging/release-notes.md` gains this change's user-facing line; the version bump and tag happen after all four are archived, with the owner's go-ahead.
