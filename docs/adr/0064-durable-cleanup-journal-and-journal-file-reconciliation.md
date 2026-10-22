# 0064. The catalog half of the cleanup journal is durable; reconciliation reads `journal.jsonl` (amends 0061)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0061 orders each move as catalog intent (committed), journal intent, rename, journal `done`, catalog
quarantined, and reconciles only moves left `moving` or `restoring`, deciding where a file is by its size. The
catalog writer runs WAL with `synchronous = NORMAL`, where a commit can be lost on power loss: a lost intent
or `done` leaves the move `planned` (or `skipped`) while the file sits in quarantine, and reconciliation never
looked. A same-size file at the other path could also be taken for the moved one.

## Decision
- Every write of the cleanup tables (`cleanup_create_run`, `cleanup_set_move`, `cleanup_set_run_state`) is
  committed in its own transaction with `PRAGMA synchronous = FULL` (`Writer::exec_durable`), after the batch
  in progress; the connection returns to `NORMAL` afterwards.
- Reconciliation (before every cleanup command and when the Cleanup history loads) also checks, for every run
  not undone that has a `journal.jsonl` of its own (same uid and run):
  - a move `planned` or `skipped` whose last journal record is `intent` or `done`: in quarantine only ->
    `quarantined`;
  - a move `quarantined` whose last journal record is `undo_intent` or `restored`: at its path only ->
    `restored`;
  - anything else is left as it is. Journal records match a move by file id and original path.
- "A file is at a path" means a regular file of the move's size whose content identity re-hashes to the
  move's (full BLAKE3 or the sampled identity), for every reconciliation decision, including marking a
  `restoring` move restored.
- A run whose source root is offline is not reconciled until it is back.
- The journal never appends onto a torn record: if the file does not end in a newline (a crash mid-append),
  the next record starts with one; readers skip only the bad line.

## Alternatives considered
- **`synchronous = FULL` for the whole writer** - costs every scan batch an extra fsync for a guarantee only
  cleanup needs.
- **Treat `journal.jsonl` as the only truth** - a read-only or full drive can lose it (ADR-0061 keeps both).

## Consequences
- Each cleanup state change costs a WAL fsync (a few ms; a move already re-hashes the file and fsyncs the
  JSON journal).
- Every cleanup command reads the journals of runs not undone; a few MB at most.

## Revisit when
The writer gains per-transaction durability control natively, or journals grow beyond what can be re-read on
every command.
