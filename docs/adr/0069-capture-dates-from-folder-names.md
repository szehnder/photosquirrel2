# 0069. Capture dates from folder names, with honest precision (amends 0022, 0043)

- Status: proposed
- Date: 2026-10-22
- Deciders: maintainers

## Context
Many photos that lost their EXIF (scans, messaging-app copies, old phone exports, files re-saved by tools) still
sit in folders that say when they were taken: `2019-07-04`, `2019_07_04 Beach`, `20190704`, a dated tree such as
`2019/07/04`, or a month folder such as `2019-07 Italy`. Today such a file's only date is its modification time
(the media-metadata spec marks it undated), so the gallery sorts it by the day it was copied and the export files
it under `Undated/<year of the copy>/`. ADR-0043 already reads dates from file names and XMP sidecars at export
time, but not from folders, and not for the catalog's own per-file date.

A folder date is weaker than an embedded date: folders are renamed, trees are reorganised, and some dated folders
name a later event (`2019-07-04` can be the day of a backup that holds twenty years of photos). A month folder
says nothing about the day.

## Decision
- **Where it ranks.** The per-file capture date is chosen in this order: DateTimeOriginal, CreateDate, container
  creation date, **folder name**, file modification time. A folder date is not "undated" (that word stays reserved
  for the file time), but it is never an embedded date: it does not feed grouping, burst protection, RAW+JPEG
  pairing or the best-copy "metadata completeness" signal, which keep reading the embedded capture time only
  (ADR-0013, ADR-0014, ADR-0035, ADR-0037).
- **Export order (amends ADR-0043).** The filing date of a keeper is: its own embedded date; another copy's
  embedded date; a date in a file name; an XMP sidecar; **a folder date (the keeper's own folder first, then each
  copy's in ADR-0043's copy order)**; the modification time under `Undated/`. File names and sidecars rank above
  folders because they describe one file, a folder describes many.
- **What is recognised.** Only folders inside the source (the file's relative path), never the source root's own
  path or anything above it. A component counts when it *starts* with a date: `YYYY-MM-DD`, `YYYY_MM_DD`,
  `YYYY.MM.DD` (years 1900-2099) or compact `YYYYMMDD` (years 1990-2099, as for file names), followed by the end of
  the name or a non-digit (`2019_07_04 Beach`); or `YYYY-MM` / `YYYY_MM` / `YYYY.MM` for a month. A chain of
  consecutive components `YYYY/MM/DD` or `YYYY/MM` (each a whole component of 4, 1-2 and 1-2 digits, optionally
  followed by a space and text: `07 July`) is a date too. Year-only folders are never used. The deepest dated
  component (closest to the file) wins.
- **Precision.** Every capture date carries a precision: `second` (embedded dates, file times), `day` or `month`.
  A month folder gives the first of the month with precision `month`; nothing displays or files it as a day.
  The export files a month-precision date under `<YYYY>/<YYYY-MM>/` (amends ADR-0022's layout); the inspector shows
  "July 2019 (folder name)".
- **Plausibility.** A folder date more than one day *after* the file's modification time is not used when the
  modification time is on or after 1990-01-01: a photo cannot be changed before it was taken, so such a folder
  names a later event (a backup or an import). A month folder compares against the end of its month.

## Alternatives considered
- **Rank folders above file names** - a folder holds many files and is renamed by people; a camera or phone writes
  the file name per shot.
- **Accept a date anywhere in a folder name** (`Backup 2019-07-04`) - names that start with a date are the photo
  convention; dates later in a name are more often a backup or download stamp. The plausibility rule catches some
  of those, not all.
- **Treat month folders as the 15th, or as day precision** - dishonest: the gallery and the library would claim a
  day nobody recorded.
- **Use folder dates in grouping or ranking** - would let a reorganised tree split or join duplicate groups; dates
  from paths are filing hints, not capture evidence.
- **Year-only folders** - `2019/` trees are common but a year is too coarse to file by and too easily a counter.

## Consequences
- Photos without EXIF in dated folders sort and file on their real day or month instead of the copy date.
- One more provenance (`folder-name`) and a precision value in the catalog, the plan, the manifest (optional
  `date_precision` field; older lines stay valid) and the inspector.
- A library gains `<YYYY>/<YYYY-MM>/` folders beside the day folders for month-precision files.
- A wrong folder name files a photo on the wrong day; the provenance is shown everywhere the date is, and an
  embedded date of any copy still wins.

## Revisit when
Users report false folder dates in practice (tighten the patterns), or a later change adds per-source "folder
naming" settings.
