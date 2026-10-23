# 0070. The day of a time instant is taken in the recorded offset, else the local zone (amends 0022)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
PhotoSquirrel keeps capture times as camera wall-clock values ("seconds as if the wall clock were UTC",
`CaptureTime`), and files a photo on the day its wall clock shows. That is right for EXIF dates: a camera records
local time, with or without `OffsetTimeOriginal`, and the photo belongs on that local day wherever the library is
built.

Two kinds of date are not wall clocks but **instants**: a file's modification time, and a QuickTime/MP4 `mvhd`
creation time (UTC by the format's definition). Today both are turned into a "wall clock" by reading the instant
as if it were UTC (`capture_time_from_unix`, the `mvhd` path in video parsing, and the export layout's `mtime_ymd`
for files without any date). A photo modified at 20:30 in California on 4 July lands in `2019-07-05`; a phone
video without a keyed local creation date files under the UTC day. Roughly a third of evening or night captures
west of Greenwich, and of morning captures east of it, get the next or previous day.

## Decision
- An instant becomes a wall-clock capture time by adding a UTC offset, chosen in this order:
  1. the offset the file itself records for that time (EXIF `OffsetTime`/`OffsetTimeOriginal`/`OffsetTimeDigitized`
     without a matching date, or a video's keyed creation date offset);
  2. the machine's local time-zone rules **at that instant** (daylight saving applied for the date of the file,
     not for today).
  The chosen offset is stored with the date (`capture_offset_min`), so the day is reproducible from the catalog.
- A container creation date whose recorded offset is exactly +00:00 is treated as a UTC instant (the `mvhd` case;
  a keyed Apple creation date with +00:00 only occurs in UTC zones, where the conversion changes nothing for a
  machine in that zone). Any other recorded offset is a wall clock and is used as recorded.
- EXIF dates are never shifted: with or without an offset, the recorded wall clock is the capture time.
- The local zone is read from the operating system once per scan run (and once per export plan for files that have
  no catalog date at all) through a small zone interface; tests inject fixed or daylight-saving rules, so results
  never depend on the test machine's zone. The OS rules come through `chrono`'s `Local` (already in the build via
  the EXIF parser), behind a cargo feature of `ps-media` that only the I/O crates enable; no time-zone database
  is bundled.
- `Undated/` keeps its meaning (only the file time is known); only the day computed for it changes.

## Alternatives considered
- **Keep UTC** - deterministic across machines but wrong for most users most of the time.
- **Bundle a time-zone database (`chrono-tz`) and a "library time zone" setting** - a large dependency and a
  setting nobody wants to think about; the OS rules are what the user's file manager shows.
- **Use the zone at export time instead of scan time** - the catalog's date (gallery, inspector, sort) and the
  library folder would disagree; storing the offset with the date keeps them one value.
- **Use another group copy's EXIF offset for a stripped file** - copies of one photo may come from devices in
  different zones; a borrowed EXIF date already wins over the file time (ADR-0043), which makes this moot.

## Consequences
- Files dated by their file time, and videos with only a UTC creation time, move to their local day in the
  gallery and in new exports.
- Two machines in different zones can file the same undated file on different days; the catalog stores the offset
  it used, and the export ledger keeps already exported copies where they are (ADR-0071).
- Changing the machine's zone does not re-date existing rows; only the date-logic version does (ADR-0071).

## Revisit when
Users run one catalog from machines in different zones and ask for a fixed "library zone", or the OS zone lookup
proves unreliable on a supported platform.
