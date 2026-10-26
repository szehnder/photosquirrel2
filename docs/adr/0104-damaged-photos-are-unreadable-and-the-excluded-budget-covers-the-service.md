# 0104. Damaged photos are "could not be read"; the Excluded view's search and budget follow what it shows

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0102](0102-exclusion-categories.md) (the category rule, the search, AI decisions without a kind, the
  put back outcome), [0103](0103-excluded-view-budget.md) (what `excluded-view-open` measures)

## Context
The gate review of `excluded-view-filters` found that ADR-0102's rule, "junk of no recognised format is not a
supported file", hid a photo whose first bytes are damaged (`IMG_9999.JPG`) and a file that could not be read at all
(the probe's I/O error record has no format) under the one category that is off by default. Those are the files a
user most needs to see. The search matched the path under the source root with ASCII case folding, while the list
prints the source's root (the home folder as `~`) and names in any script. An AI decision stored without its kind
fell in no chip. Putting back reported a plain failure when the AI review's store could not save the rejection,
although the include decisions had committed. And `excluded-view-open` timed the catalog's pass, not the service
calls the view makes (whose page read looked up each row's decision on its own).

Migration 21 is committed and applied in catalogs built since; 0.3.0 is not released. Applied migrations are never
edited.

## Decision
- **The rule.** Junk of a recognised format: "could not be read" (`unreadable`) when its first reason is a probe
  failure, else "too small". Junk of no recognised format: `unreadable` when any reason is a probe failure (the file
  could not be opened or read) or its name's extension is a photo or video extension, else "not a supported file".
  The extension is the text after the name's last `.`, lower case. The photo and video extensions are
  `ps_model::media_extensions()`: every `Format::extensions()` entry (the probe's extension table, moved to
  `ps-model`) of a format other than SVG, ICO and PSD, without `m4a`, `mka` (audio) and `ts` (mostly TypeScript).
  `ExclusionCategory::of` in Rust and `writer::exclusion_sql` in SQL implement it; a test compares both, and
  migration 22, over every class, format, reason and name case (names that are not UTF-8 included).
- **Migration 22** (`0022_exclusion_category_fix.sql`) recomputes `file.exclusion` for the classifier-excluded rows
  with the amended rule (SQL only, no file read), files an AI decision without a kind as the user's own, and adds
  triggers that refuse an `origin='ai'` row without a kind or that is not an exclusion. The readers file any such
  row under "excluded by you". A refresh's re-classification takes the category from the new probe's format.
- **Search** matches the source's root as the app prints it (`ps_catalog::display::display_path`, the home folder as
  `~`; the app's lists use the same function) joined to the path under it, with case folded character by character
  (Unicode). It runs in the streaming pass, not in SQL.
- **Put back** commits the include decisions first and returns `PutBack { files, rejection_error }`: when the AI
  review's store cannot save a rejection, the files are reported as put back with that failure beside them.
- **`excluded-view-open`** is measured in `ps-service/tests/budgets.rs` through `Library::excluded_open` and the
  first 200-row `Library::excluded_page`, on the same 1M-file catalog. `excluded_page` reads the page's decisions in
  one query. The test prints the first run and the median of 5; the budget is the **median of the warm runs**
  (the catalog's pages are in the cache after the build; a cold open after a restart is the first-run figure, which
  is reported, not budgeted). The bound stays 150 ms. A search over the same set is printed and held to that bound.
  Measured on the reference machine: median 105 ms, first run 164 ms, a search 102 ms.

## Alternatives considered
- **Edit migration 21** - it is applied in every catalog built since it was committed; an edited migration would
  never run there.
- **Re-probe damaged files to decide** - a migration that reads files breaks ADR-0102's "reads no file bytes"; the
  stored name and reasons are enough.
- **Every unrecognised file "could not be read"** - the hidden category would be empty and the noise (text files,
  archives) back in the default view.
- **Measure the cold open** - the reference machine's page cache cannot be emptied from a test without privileges;
  the first run is printed so a regression there is visible.

## Consequences
- Existing catalogs move damaged photos and unreadable files from the hidden "not a supported file" to "could not
  be read" on open; counts in the default view go up by those files.
- `preview-placeholders` takes migration 23.
- The search's cost moves from SQLite to Rust string folding over the excluded rows; it is held to the budget's bound.

## Revisit when
The probe records why a file could not be read in a structured form (then read it instead of the reason text and
the extension), or a cold open of the Excluded view is measured on the reference machine.
