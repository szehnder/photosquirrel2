# 0007. Incremental rescans by stat fingerprint

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The user re-runs scans on the same drives. Re-reading terabytes to find a handful of new files is unacceptable.

## Decision
Each file row stores a **stat fingerprint**: size, modification time (ns) and, where available, inode/file-index. A rescan walks the tree (listing + stat only), diffs against the catalog and classifies every entry as *new*, *changed* (fingerprint differs -> re-analyze), *unchanged* (no work) or *missing* (marked missing, never deleted from history). Only new and changed files are read. A source's rescan report shows the diff counts before any analysis starts. Moved files inside a source are re-identified by content hash after analysis, keeping their AI results and dispositions.

## Alternatives considered
- **Directory mtime short-circuit** - skips unchanged directories but misses in-place edits on some filesystems; may be added later as an opt-in "quick" mode.
- **Filesystem change journals (USN, FSEvents)** - platform-specific and unavailable on NAS/exFAT.

## Consequences
- A no-change rescan of 100k files completes within the walk budget (ADR-0026).

## Revisit when
NAS listing cost dominates; then add the quick mode.
