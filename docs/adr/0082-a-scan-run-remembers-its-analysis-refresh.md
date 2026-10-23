# 0082. A scan run remembers its analysis refresh (migration 18; amends 0071)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0071](0071-date-logic-version-and-existing-libraries.md)

## Context
ADR-0071 added an opt-in analysis refresh (`photosquirrel-cli scan --refresh-analysis`, the app's "Refresh
analysis" action): unchanged rows probed before class facts were stored are re-read up to the probe limit and
re-classified. The option lived only in the process that started the run. A refresh run that was paused in one
process, cancelled or interrupted, and then resumed (`scan resume`, `scan --resume`, the app's resume after a
restart) walked again as a plain scan: the remaining rows were counted, not refreshed, although the user had asked
for the refresh.

The gate review of the capture-dates change also found that a refresh file that could not be opened (permissions,
a share hiccup) went through the ordinary failure path, which sets the row to `failed`. An unchanged, fully
analysed keeper then left its duplicate group, and the next export planned a second, worse copy of a photo that
was already in the library.

## Decision
- **The run records the refresh.** Catalog migration 18 adds `scan_run.refresh_analysis INTEGER NOT NULL DEFAULT 0`
  (`0018_run_refresh.sql`). `Writer::start_run_with(sources, refresh)` writes it when a run starts with the refresh
  asked for (by the start call or by `ScanOptions::refresh_analysis`); `RunRecord::refresh_analysis` reads it, and
  `Scanner::resume_run` resumes a refresh run as a refresh run, whatever the resuming process's options say. Older
  runs read as plain scans.
- **A refresh never fails a row.** When an analysis-refresh file cannot be opened or read, its row keeps its stage,
  analysis, class and group; the run reports a recent error ("could not be read for the analysis refresh; it keeps
  its analysis") and the file stays counted as needing a refresh, so a later refresh tries again. A file that is
  gone is marked missing as before.
- **Summaries count what the refresh did.** `DiffCounts::refreshed` counts files the run refreshed; the CLI and the
  scan dashboard report "refreshed the analysis of N files" and as needing a refresh only
  `needs_refresh - refreshed`.

## Alternatives considered
- **Fold the column into migration 17** - migration 17 is unreleased, but catalogs at version 17 already exist on
  developer machines; `PRAGMA user_version` migrations never re-run, so those catalogs would lack the column.
- **Keep the flag in the run's `stats` JSON** - the summary overwrites that JSON at pause and at the end, and an
  interrupted run has none.
- **Let a refresh read error fail the row like a scan error** - punishes a file that was fine and is exported for
  a refresh the user asked for; ADR-0004's first priority is never to lose or duplicate a photo.

## Consequences
- One more column on `scan_run`; downgrade is not supported, as for every migration (ADR-0005).
- `photosquirrel-cli scan resume` and the app's resume finish a refresh the user started.
- An unreadable file keeps an old class until a later refresh can read it; the error says so.

## Revisit when
Runs grow more per-run options worth remembering (then store a run options record instead of one column).
