# 0080. Folders named for a range of two years are not month dates (amends 0069)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0069](0069-capture-dates-from-folder-names.md)

## Context
ADR-0069 reads a month from a folder name that starts with `YYYY-MM`, `YYYY_MM` or `YYYY.MM`. School years, sport
seasons and academic terms are named the same way: `2011-12 School Year`, `2008_09 Season`, `2009-10 Soccer`.
Under ADR-0069 such a folder dates every EXIF-less photo in it as December 2011, September 2008 or October 2009,
although it says only that the photos were taken during two years. The gate review of the capture-dates change
found this; years 2000-2011 (and 1900-1911) are the ones whose following year's last two digits are a valid month,
and they are the years of many scanned and messaging-app photos.

A range written with both years in full (`2011-2012`) already gives no date (`20` is not a month), but nothing said
so.

## Decision
- A folder name that starts with `YYYY` + separator + `YY`, where `YY` equals the last two digits of `YYYY + 1`
  (`2011-12`, `2008_09`, `1999.00`), is read as a range of two years and gives **no** folder date. The rule applies
  to the month patterns only: `2011-12-25 Christmas` is still 25 December 2011, and a dated tree `2011/12` is still
  December 2011 (a chain of whole components is a calendar, not a range).
- A name starting with `YYYY-YYYY` (`2011-2012`) gives no folder date either (stated in the spec, unchanged code).
- Files in such folders fall back to their file time (undated), as for any folder without a date; an embedded date
  of any copy still wins at export (ADR-0043, ADR-0069).
- `DATE_LOGIC_VERSION` stays 2: the folder rules of version 2 have not been released, so no catalog outside
  development builds has rows dated by the earlier reading.

## Alternatives considered
- **Keep the month and let the plausibility check catch it** - the check compares against the file time, which
  for a photo taken in December of a school year passes; it is not meant to detect ranges.
- **Read the range as its first year with year precision** - year-only dates are excluded by ADR-0069 (too coarse
  to file by), and a school year spans two calendar years anyway.
- **Require the folder text to mention "school", "season" or similar** - language-dependent and incomplete.

## Consequences
- `2011-12 Christmas` (December 2011, written as a month folder) is no longer a month date: an accepted false
  negative, since the files still get their file time and a day folder (`2011-12-24 Christmas`) keeps working.
- One more scenario in the media-metadata spec ("School-year folder is not a month").

## Revisit when
Users report month folders of the years 2000-2011 that lose their dates, or per-source folder naming settings are
added (ADR-0069).
