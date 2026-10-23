# 0071. A date-logic version re-dates catalog rows from stored facts; exported libraries keep their layout (migration 17)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0069 (folder-name dates) and ADR-0070 (local day of an instant) change the capture date of files that have no
embedded date. A rescan only re-analyses new and changed files (incremental-rescan spec), so on an existing
catalog nothing would change until each file's content changed: the same gap ADR-0051 left for re-classification
of rows probed before migration 13. Users with existing catalogs are exactly the ones with big, already exported
libraries.

What the new logic needs is already in the catalog for every analysed row: the relative path, the modification
time (`mtime_ns`), the embedded-date provenance and value (`capture_provenance`, `capture_ts`,
`capture_offset_min`, since migration 1) and the metadata JSON (tag list, video creation time). No file has to be
read.

The export ledger (ADR-0023, ADR-0044) decides "already exported" **by content**: the planner looks up `done`
ledger rows for the keeper's content identity and trusts a copy that is still at its recorded path with its
recorded size and time, whatever folder today's layout would choose (`ps_export::plan`; ADR-0043 already relies on
this). A re-dated keeper that is already in the library is therefore not copied again; a Live Photo or RAW+JPEG
bundle that gains a file completes next to its already exported primary. The ledger lives in the catalog, so this
holds for the catalog that made the library. A *new* catalog exporting into an existing library has no ledger rows
and only adopts identical files at the exact target path; any file whose target folder differs is copied again.
That gap exists today (renamed keepers, different keepers); this change widens it, because many undated files'
target folders move.

## Decision
- **Date-logic version.** `ps_media` defines `DATE_LOGIC_VERSION` (2 for ADR-0069 and ADR-0070; rows written before
  are version 1). Catalog migration 17 adds `file.capture_precision` (0 second, 1 day, 2 month; default 0) and
  `file.date_version` (default 1), and sets `date_version = 2` in the same migration for rows whose date cannot
  change: provenance `DateTimeOriginal` or `CreateDate`, or `container` with a non-zero offset.
- **Re-dating on the next rescan.** The rescan diff re-dates every unchanged, analysed row of the source being
  scanned whose `date_version` is below the current one, from the stored facts only (path, `mtime_ns`, provenance,
  metadata JSON), with the run's local zone, and writes the five date columns and the version in chunks
  (`Command::Redate`, like ADR-0051's `Command::Reclassify`). The diff reports `redated` (rows whose date changed).
  Re-dating never opens a source file, so it also runs while a source's other work is unchanged; offline sources
  are re-dated on their next rescan. New and changed files are dated by the new logic during analysis.
- **The catalog columns are the date.** Readers that need a file's resolved date (export filing, inspector, review)
  read the catalog columns, not the `capture_date` inside the metadata JSON, which keeps describing the analysis as
  it was. Grouping keeps reading the embedded capture time from the metadata; the dedup change stamp stops
  covering `capture_ts`, so re-dating does not trigger a regroup.
- **Existing libraries keep their layout.** Export never moves or renames library files (ADR-0004, ADR-0022).
  Copies exported under the earlier rules stay where they are and stay "exported"; only keepers not yet in the
  library use the new folders. The plan counts already exported copies whose folder differs from the folder today's
  rules would choose ("filed under earlier date rules"), so the user sees it, and changes nothing.
- **Libraries this catalog did not write.** When the chosen library holds `.photosquirrel/manifest.jsonl` and this
  catalog has no ledger rows for it, the plan carries a warning (Export view, CLI text and `--json`): the library was
  written by another catalog and files already in it may be copied again into different folders. The export can
  still run; nothing is overwritten (ADR-0038). Adopting such a library from its manifest is a separate, later
  change.
- **Pre-migration-13 rows.** Re-dating covers them (it needs no class facts). Their missing class facts (ADR-0051)
  stay a separate, opt-in refresh: a rescan with "refresh analysis" re-probes, read-only, the first 256 KB of every
  unchanged row without facts, then re-classifies it as ADR-0051 does. It is never automatic because of its cost.

## Cost at 1M files (reference machine, docs/performance.md)
- Re-dating: one pass over the stale rows' path, time, date columns and metadata JSON. Only rows without an embedded
  date are stale after migration 17; on a typical archive that is 10-30% (100-300k rows), all of them in the worst
  case. JSON parse and resolution are well under 50 µs per row; writes run at the catalog's measured insert rate
  (budget `catalog-insert`, 50k rows/s). Expected one-time cost: about 5-15 s for 100-300k stale rows, at most
  about 40-60 s if all 1M rows are stale, on top of the ~40 s no-change rescan of 1M files (10 x the 4 s/100k
  budget). Zero bytes of file content read. Every later rescan pays nothing.
- Opt-in refresh of class facts: up to 256 KB read per row without facts. For 1M such rows that is up to 256 GB:
  minutes on an SSD, roughly 1-3 hours on a USB HDD or a gigabit NAS. Hence opt-in, resumable and paused and
  cancelled like any scan stage.

## Alternatives considered
- **Re-analyse every row on an analysis-version bump** - reads every file again (hours to days at 1M files on NAS
  and HDD) to recompute values derivable from the catalog.
- **A catalog-wide flag instead of a per-row version** - rows of offline sources would be marked done without being
  re-dated, or the flag would never clear; a per-row version is exact and resumable.
- **Move already exported copies to their new folders** - a new mutation of user-visible library files, against
  ADR-0004's copy-only export; photo applications that already imported them would see them vanish.
- **Re-export into the new folders and leave the old copies** - duplicates in the library, the one outcome the user
  must never get from a re-export.
- **Adopt files from another catalog's manifest now** - correct but needs hash verification of the whole library
  for a fresh catalog; out of scope here.

## Consequences
- Existing catalogs pick up folder dates and local days on their next rescan with no file reads.
- A library can mix earlier-rule folders (for copies exported before) and new folders (for later keepers); the
  plan reports how many copies are filed under earlier rules.
- Two more columns per file row (a few MB at 1M files).
- Future date-rule changes bump `DATE_LOGIC_VERSION` and, if needed, mark rows whose date cannot change in their
  migration.

## Revisit when
A date rule needs facts the catalog does not store (then the bump must re-read, and needs its own ADR), or users ask
to re-file an existing library (a separate "reorganise library" feature with its own safety design).
