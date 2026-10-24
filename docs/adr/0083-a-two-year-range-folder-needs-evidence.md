# 0083. A folder is read as a range of two years only with text after it, outside its year (amends 0080)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0080](0080-two-year-range-folders-are-not-month-dates.md)

## Context
ADR-0080 reads every folder name starting with `YYYY-YY`, `YYYY_YY` or `YYYY.YY`, where `YY` is the last two
digits of `YYYY + 1`, as a range of two years with no date. It names the cost as `2011-12 Christmas` losing its
month, but understates it: the common month layout `YYYY/YYYY-MM/` (`2010/2010-11/`, as many importers, phones and
people write it) loses its date for one month of every year 2000-2011 (and 1900-1911): November 2010, December
2011, June 2005 and so on. Those are bare month folders, or month folders sitting in a folder of their own year;
neither reads as a school year or season to a person.

A school year or a season is almost always written with a word after it (`2011-12 School Year`, `2008_09 Season`,
`2009-10 Soccer`), and is not filed inside a folder of its first year (that folder would hold only half of it).

## Decision
- `YYYY-YY`, `YYYY_YY` or `YYYY.YY` with `YY` = the last two digits of `YYYY + 1` is a range of two years (no
  folder date) only when **both** hold:
  - more text follows it in the folder name (`2011-12 School Year`; a bare `2011-12` is not a range), and
  - the folder directly holding it is not a folder of the year `YYYY` (a whole component `YYYY`, optionally
    followed by a space and text, as the dated-tree rule of ADR-0069 reads a year: `2010`, `2010 Family`).
- Otherwise the name is a month, as ADR-0069 reads it: `2010-11`, `2005_06`, `2010/2010-11`,
  `2010/2010-11 Thanksgiving` and `2011 Family/2011-12 Christmas` are November 2010, June 2005, November 2010,
  November 2010 and December 2011. A folder of another year does not help (`2012/2011-12 School Year` stays a
  range).
- Unchanged: `YYYY-YYYY` (`2011-2012`) is never a date; `2011-12-25` is a day; a dated tree `2011/12` is a month;
  only the immediate parent is looked at, and only folders inside the source count.
- `DATE_LOGIC_VERSION` stays 2: the folder rules of version 2 (ADR-0080 included) have not been released, so no
  catalog outside development builds holds dates from the ADR-0080 reading.

## Alternatives considered
- **Keep ADR-0080 as it is** - loses one real month per year in the most common month layout, for the benefit of
  ranges that nearly always carry a word anyway.
- **Only the parent-folder test** - a bare top-level `2010-11` (a month folder in a source of month folders) would
  still lose its month.
- **Only the trailing-text test** - `2010/2010-11 Thanksgiving` would lose its month although its year folder
  says it is a month.
- **A word list ("school", "season", "year")** - language-dependent and incomplete (ADR-0080).

## Consequences
- A bare `2011-12` folder used for a school year dates its EXIF-less photos to December 2011: an accepted false
  positive, limited by the plausibility check (ADR-0069, ADR-0079) and never deciding duplicates or keepers.
- `2011-12 Christmas` at the top of a source is still a range (ADR-0080's accepted false negative), but inside a
  `2011` folder it is December 2011 again.
- Two more scenarios in the media-metadata spec ("Bare month folder next to the school-year pattern is a month",
  "Month folder inside its year folder is a month").

## Revisit when
Users report school-year folders filed inside their first year's folder, or per-source folder naming settings
are added (ADR-0069).
