# 0043. Export files a keeper by the best capture date its duplicate group knows

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
ADR-0022 files each exported keeper by its own date: DateTimeOriginal, else the QuickTime creation date, else the
file modification time under `Undated/`. The keeper is the best *pixels* (ADR-0014), and that is often a copy that
lost its EXIF: a full-resolution file re-saved by a messaging app or an editor, kept over a smaller copy that still
has the camera metadata. Such a keeper landed in `Undated/` or in the year of its modification time (2024/) although
its group knows the day the photo was taken (another member has DateTimeOriginal 2023-06-26). For a library meant
for Apple Photos, Google Photos or Lightroom that is the wrong folder for every such photo.

## Decision
The filing date of an exported file is resolved per keeper file over its whole duplicate group
(`ps_export::date::resolve`), in this order:
1. the file's own embedded date (DateTimeOriginal, CreateDate, container creation date);
2. another copy's embedded date: best provenance first, then the earliest, then the lowest file id;
3. a date in a file name (`IMG_20230626_143012.jpg`, `2023-06-26 14.30.12.jpg`), the keeper's first;
4. an XMP sidecar next to a copy (`exif:DateTimeOriginal`, `photoshop:DateCreated`, `xmp:CreateDate`), read
   read-only and only when nothing above gave a date, so planning stays cheap on NAS sources;
5. the keeper's modification time, under `Undated/` as before.

Every planned file carries its `DateOrigin` (source and the copy it came from). The plan's totals count files by
origin, the CLI dry run prints it per file, the Export view shows a one-line summary, and each manifest record gains
`date_source` (and `date_from`, the other copy's path) as optional fields; older manifest lines stay valid. Only the
export folder changes: the catalog's per-file dates, the gallery and grouping are untouched, and the copied bytes
are never rewritten (ADR-0022).

## Alternatives considered
- **Keep ADR-0022's per-file rule** - files real photos under the wrong year or `Undated/`.
- **Write the borrowed date into the copy's EXIF** - ADR-0022 forbids rewriting copies; a sidecar carries keywords
  only, and applications read the folder date for import anyway.
- **Prefer the file-name date over another copy's EXIF** - names are renamed and reused more often than EXIF
  dates are wrong; embedded dates win.

## Consequences
- Re-planning an existing library can place a keeper it has not exported yet in a different folder than an older
  plan would have; exported files stay where they are (the ledger keys by content).
- The file-name parser accepts compact dates only in 1990-2099 to avoid counters like `DSC_19871234`.

## Revisit when
The catalog stores a group-level capture date, or users ask for date precedence to be configurable.
