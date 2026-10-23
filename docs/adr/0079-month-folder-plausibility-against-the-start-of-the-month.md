# 0079. A month folder's plausibility check compares against the start of the month (amends 0069)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0069](0069-capture-dates-from-folder-names.md)

## Context
ADR-0069 does not use a folder date that lies more than one day after the file's modification time (when that
time is on or after 1990-01-01): a photo cannot be changed before it was taken, so such a folder names a later
event. For a month folder it says "a month folder compares against the end of its month", while the
media-metadata spec of the same change (requirement "Folder date plausibility") says "for a month folder, before
the start of that month minus one day".

Taken literally, the end-of-month reading rejects most month folders: a photo taken and saved on 3 July in
`2019-07 Italy` has a modification time 28 days before the end of July and would fall back to its file time. The
rule exists to catch a folder that names a later event; a file modified at any time during the month, or after
it, is consistent with the folder.

## Decision
A month-precision folder date is checked like a day date whose day is the first of the month: it is not used
when the file's modification time (on or after 1990-01-01, read as the local wall clock of ADR-0070) lies more
than one day before the first day of that month at 00:00. `ps_media::date::resolve_capture_date` implements this
for analysis and re-dating alike; `scenario_media_metadata_backup_folder_date_ignored` covers a month folder
whose file was modified before the month.

## Alternatives considered
- **End of the month** (ADR-0069's wording) - rejects photos saved during their own month, the common case.
- **No check for month folders** - lets a `2019-07 Backup` folder of older photos date them all July 2019.

## Consequences
- The spec's wording is the rule; ADR-0069's sentence about the end of the month is superseded by this one.
- Month folders keep their dates for files modified during or after the month.

## Revisit when
Users report month folders that name a later event yet hold files modified during that month.
