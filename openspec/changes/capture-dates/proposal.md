# Proposal

## Why

Photos without EXIF are filed by the day they were copied, not the day they were taken: a scan or a
messaging-app copy in `2019-07-04 Beach/` lands in `Undated/2024/`, and every file dated by its file time (and
every phone video with only a UTC container time) is filed on the UTC day, which is the wrong day for a large share
of evening and night captures. Existing catalogs would not pick up any fix until each file's content changed.

## What Changes

- Capture dates from folder names (`2019-07-04`, `2019_07_04 Beach`, `20190704`, `2019/07/04`, and `2019-07` /
  `2019/07` month folders) as a fallback below every embedded date and above the file modification time, with an
  honest precision (second, day or month) and a plausibility check against the file time (ADR-0069).
- The export's filing order gains folder dates after file-name and sidecar dates (amends ADR-0043); month-precision
  files go to `<library>/<YYYY>/<YYYY-MM>/`.
- File modification times and UTC container times become a day in the file's recorded offset, else the machine's
  local zone at that instant, for the catalog date and for `Undated/` folders; EXIF wall clocks are never shifted
  (ADR-0070).
- A date-logic version (catalog migration 17): the next rescan re-dates existing rows from stored facts without
  reading any file; the diff reports how many dates changed (ADR-0071).
- Already exported copies stay where they are; a re-export copies nothing already in the library, and the plan
  reports how many copies are filed under earlier date rules. A library written by another catalog gets a warning
  that files may be copied again.
- Opt-in "refresh analysis" for rows probed before catalog migration 13 (no stored class facts): a read-only
  re-probe of the first 256 KB so they can be re-classified (CLI `scan --refresh-analysis`, app scan summary).
- The inspector shows a month-precision date as a month, with its provenance.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `media-metadata`: capture date resolution gains folder-name dates, precision and the local day of instants.
- `export`: dated layout (month folders, local `Undated/` days), group filing order, existing libraries keep their
  layout, warning for libraries of another catalog.
- `incremental-rescan`: existing rows re-dated from stored facts; opt-in refresh of missing class facts.
- `photo-inspector`: capture precision shown.

## Impact

- ADRs relied on: 0004, 0005, 0013, 0014, 0022, 0023, 0035, 0037, 0038, 0043, 0044, 0046, 0051, 0065. New:
  0069 (folder-name dates), 0070 (local day of an instant), 0071 (date-logic version, migration 17, existing
  libraries).
- Crates: `ps-model` (precision, provenance), `ps-media` (date resolution, folder parser, zone interface; file-name
  parser moves here from `ps-export`), `ps-catalog` (migration 17, `Command::Redate`, date columns on `FileRecord`,
  dedup stamp), `ps-scan` (pipeline dating, diff re-dating, refresh option), `ps-export` (filing order, month
  layout, local mtime day, earlier-rule count, unknown-library warning), `ps-service` (export inputs from columns,
  inspector), `photosquirrel-cli` (`scan --refresh-analysis`, plan output), `ui-workspace` (inspector, scan
  summary, export view note).
- Dependency: `chrono` becomes a direct, feature-gated dependency of `ps-media` for the OS zone rules, enabled
  only by the I/O crates (already in the build through the EXIF parser; ADR-0070).
- Effort: about 6-8 focused days (parser and resolution 1.5, zone 1, migration and re-dating 1.5, export 1.5, refresh
  analysis 1, UI and docs 1), plus the corpus gate run.

## Out of scope

- Moving, renaming or re-filing files already in a library (a future "reorganise library" change would need its
  own safety design).
- Adopting another catalog's library from its manifest (only a warning here).
- Year-only folders, dates in the middle of a folder name, per-source folder-naming settings.
- File-name dates in the catalog's per-file date (they stay an export-time filing source, ADR-0043).
- Writing any date into source files or library copies.
- A fixed "library time zone" setting.
