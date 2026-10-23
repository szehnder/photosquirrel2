# 0081. A keyed QuickTime creation date is a wall clock, also at +00:00 (amends 0070)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0070](0070-day-of-a-time-instant-in-the-local-zone.md)

## Context
ADR-0070 treats a container creation date whose recorded offset is exactly +00:00 as a UTC instant and moves it to
the machine's local zone, because the movie header's `mvhd` time is UTC by definition and the video parser hands
both kinds of date over the same way. It argued that a keyed Apple creation date (`mdta` key
`com.apple.quicktime.creationdate`, an ISO 8601 local time with the device's offset) at +00:00 only occurs in UTC
zones, where the conversion changes nothing *for a machine in that zone*. The gate review of the capture-dates
change pointed out the other case: a video recorded in London in winter (or Lisbon, Reykjavik, Accra) at 00:30 and
scanned on a machine in California moves to 17:30 the previous day, although the phone recorded the local day.

`nom-exif` prefers the keyed date and falls back to `mvhd`, so `ps_media::video` can tell the two apart by reading
the key itself.

## Decision
- `ps_media::video::movie_info` marks the creation date as keyed when the `moov/meta` key
  `com.apple.quicktime.creationdate` holds the same wall clock the parser returned (`VideoInfo::creation_keyed`).
  `EmbeddedDates::container_keyed` carries it to `resolve_capture_date`: a keyed date keeps its wall clock and
  offset, whatever the offset; only an `mvhd`-only date at +00:00 is an instant made local (ADR-0070).
- The metadata JSON records the source as the QuickTime tag `CreationDateSource` (`keyed` or `mvhd`), so
  re-dating from stored facts (`EmbeddedDates::from_stored`, ADR-0071) decides alike. Rows analysed before the tag
  existed read as `mvhd`, as ADR-0070 treated them.
- `DATE_LOGIC_VERSION` stays 2: version 2 has not been released.

## Alternatives considered
- **Keep ADR-0070's rule** - wrong by up to a day for keyed +00:00 videos viewed from other zones.
- **Re-read videos without the tag on rescan** - re-dating must not read files (ADR-0071); an analysis refresh
  or a content change re-analyses them anyway.
- **Compare the keyed instant with `mvhd`** - cameras write both from one clock, so equal instants say nothing
  about which one the parser used.

## Consequences
- Keyed QuickTime dates at +00:00 stay on the recorded day on every machine.
- One more tag in each video's tag list; older lines of metadata JSON stay valid.
- A catalog row analysed before this change and dated from a keyed +00:00 date is re-dated as an instant until the
  file is analysed again.

## Revisit when
Another container format records local dates with an offset that the parser reports the same way as `mvhd`.
