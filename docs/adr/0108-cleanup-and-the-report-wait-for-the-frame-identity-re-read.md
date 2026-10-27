# 0108. Cleanup and the report wait for the frame-identity re-read

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0105](0105-recorded-frame-identities-keep-frames-apart.md) and
  [0106](0106-frame-identity-re-read-covers-same-second-canon-files-and-quarantine.md) (what the one-time re-read
  reads, and what is trusted before it ran), [0024](0024-reversible-quarantine-cleanup.md)
  (a precondition of cleanup plan and run)

## Context
Migration 23 marks the rows whose frame can matter for a re-read on the next rescan. Until that rescan, the groups
are the ones built without frame identity. The gate review showed that `cleanup plan` and `cleanup run` then still
quarantined frame N+1's CR2 as covered by frame N's library copy, and the report said 100% accounted for. Cleanup
re-derives coverage from the current groups before each move, but the current groups are the wrong ones.

The review also found that the re-read read the probe window only: a `DNGPrivateData` or maker-note value beyond the
first 256 KB was taken for "no frame identity" and the row stamped current for good; and that a quarantined file
removed from the quarantine folder by hand was reported unreadable on every rescan.

## Decision
- **Stale rows bear on a scope** when they are analysed rows below `META_LOGIC_VERSION` that are in the scope, or in
  a duplicate group with a file in it. While any does, `cleanup plan` (the source or folder) and `cleanup run` (the
  planned files) are refused with "rescan first", and the completion report flags each such source as not confirmed
  (the CLI badge, its JSON `rescan_needed`, and the app's report badges, beside "Free up space"). Once every row is
  current this costs one indexed count.
- **The re-read is complete or not stamped.** For a TIFF-based file it fetches directories beyond the window with the
  ADR-0084 bounds (1 MiB, 64 reads), then the frame values the window lacks (`OriginalRawFileName`,
  `DNGPrivateData`, the Exif maker note), each at most 1 MiB. When a bound stops it, or a value lies outside the
  file, the row is not stamped and stays below the version (and cleanup keeps waiting for it): a later analysis of
  the file reads it.
- **A quarantined file gone from the quarantine folder** (`NotFound` there, the source online) is stamped current
  without new facts and without an error: nothing is left to read.
- The re-read writes the frame tags into the metadata's tag list the same way a fresh analysis does.

## Alternatives considered
- **Block cleanup while any row of the catalog is stale** - an offline source would block every other source's
  cleanup for no reason.
- **Leave a vanished quarantine file unstamped** - cleanup would wait forever for a file that cannot come back.
- **Read the whole file when the window is not enough** - unbounded on large RAWs; the bounded reads cover the
  layouts converters write.

## Consequences
- After upgrading, the first cleanup needs a rescan of the sources involved; the message says so.
- A file whose frame values lie beyond the bounds keeps cleanup of its scope waiting until it is analysed again
  (a change or a refresh). None is known.

## Revisit when
A real file is reported whose frame values lie beyond the bounds, or a source cannot be rescanned and blocks a
cleanup the owner needs.
