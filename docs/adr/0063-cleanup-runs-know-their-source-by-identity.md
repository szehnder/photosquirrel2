# 0063. Cleanup runs know their source by identity; quarantined files match by path (migration 15)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0061 stores a cleanup run's `source_id` and its moves' `file_id`, and the service used the source's
*current* root for a run's undo and matched quarantined files to catalog rows by `file_id` only. Neither
`source.id` nor `file.id` is AUTOINCREMENT, so SQLite hands a removed row's id to the next insert. The gate
review reproduced it: clean drive-c (run 3), remove source 3, add another folder; the new folder got id 3,
`cleanup list` showed run 3 against it, and `undo 3` looked for every file in the wrong folder. A reused file id
would likewise mark an unrelated file as quarantined.

## Decision
- **A source with files in quarantine is never removed or folded** into a new outer source. Both are refused
  with a message that says to undo its runs first (moves in states moving, quarantined or restoring count).
- **Source ids a cleanup run refers to are never handed out again**: a new source gets
  `1 + max(max(source.id), max(cleanup_run.source_id))`.
- **Catalog migration 15** adds `cleanup_run.source_added_at`: a run's source is the source with its id *and*
  that `added_at`. Existing runs take the `added_at` of their source when it was added before the run was
  planned, else stay NULL (no current source matches). When the run's source is gone the run acts on the
  root stored with it (`root_path`); `cleanup run` refuses a run whose source was removed, and undo refuses a
  run folder whose `journal.jsonl` header carries another run's uid.
- **Quarantined files are matched by source, relative path and the move's stat fingerprint** (size and
  modification time), never by `file_id` alone: in dispositions, grouping inputs, the export input and the
  rescan diff. Migration 15 adds `cleanup_move_by_path (rel_path, state)` for that lookup.

## Alternatives considered
- **AUTOINCREMENT on `source` and `file`** - SQLite cannot add it in place; rebuilding `file` (up to a million
  rows, a dozen covering indexes, foreign keys from other tables) in a migration is slow and risky, and file ids
  would still need the path match for rows reused for another file at the same path.
- **Undo always on the stored root** - a re-linked source (same drive, new mount point) keeps its id and
  `added_at`; its current root is right.

## Consequences
- `cleanup_run` gains a column; the stored plan root is now load-bearing.
- Removing or folding a drive needs its quarantine undone first.

## Revisit when
The catalog moves to stable UUID identities for sources or files.
