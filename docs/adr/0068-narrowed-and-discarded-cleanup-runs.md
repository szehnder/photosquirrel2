# 0068. Cleanup plans are narrowed by marking moves unselected and discarded by state; no cleanup row is deleted (migration 16; amends 0061)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers

## Context
A cleanup plan (ADR-0024, ADR-0061) covers a whole source, or one folder of it from the CLI. The owner wants
the app to narrow a stored plan to chosen folders and files before confirming, and wants a way to drop a plan
that was never run (`cleanup discard <run>` and a button in the app). Today a planned run can only be run or
left in the history forever.

Two facts shape the choice. First, the catalog rows of a run are half of its write-ahead journal (ADR-0061,
ADR-0064): reconciliation compares them with `journal.jsonl` and with the file system, and it can only
recover a move whose catalog row still exists (the "lost catalog commit" case of ADR-0064 adopts a `planned`
row whose file the JSON journal records as moved). Second, run and file ids are plain `INTEGER PRIMARY KEY`s,
which SQLite hands out again after the highest row is deleted; the quarantine folder is named after the run
id (`.photosquirrel-quarantine/<run id>/`), and ADR-0063 already had to defend against reused ids.

Migration 14 constrains `cleanup_run.state` to four values and `cleanup_move.state` to codes 0-5 with `CHECK`
clauses, which SQLite cannot alter in place.

## Decision
- **Narrowing marks, never deletes.** A new move state `unselected` (code 6; codes are never reused) means
  "in the plan, left out by the user". Setting a run's selection makes every `planned` or `unselected` move
  of that run `planned` if the selection picks it and `unselected` otherwise, in one durable transaction.
  Moves in any other state are untouched. A run only ever moves `planned` moves, so an unselected file is
  never re-verified, journaled or renamed. Selection is allowed while the run is `planned` (also a partly
  run one that was stopped) and is reversible until each move is attempted. A selection that leaves no move
  planned is refused: the user discards the run instead.
- **Discard is a run state.** A new run state `discarded` ends a run that never moved a file. Discard is
  refused when any move of the run is or was `moving`, `quarantined`, `restoring` or `restored`, and when the
  run's quarantine folder holds a `journal.jsonl` with an intent (checked after reconciliation, which runs
  first as for every cleanup command). On a discard, the run's `planned` moves become `unselected`, the run
  becomes `discarded` with its end time, and nothing on disk is created, renamed or removed; directories a
  stopped run created under its quarantine folder before skipping its files are left as they are
  (ADR-0024: PhotoSquirrel never deletes).
- **Discard works offline, and reconciliation still watches discarded runs.** A source that is not
  connected cannot be checked for a journal, but the catalog half is durable before every rename
  (ADR-0064), so a run with only `planned`/`skipped`/`unselected` rows moved nothing. Reconciliation treats
  `discarded` runs like any run that is not `undone`: if their quarantine folder later shows a journal that
  records a move, the move is adopted as in ADR-0064 and the run returns to `completed`, with a log line and
  a note on the move. A discarded run cannot be run, selected or undone.
- **No cleanup row is ever deleted**, so run ids are never handed out again and reconciliation always has
  the row it needs.
- **Catalog migration 16** rebuilds `cleanup_run` and `cleanup_move` inside the migration transaction,
  keeping every row, index and the foreign key: the old tables are renamed aside, the new ones created and
  filled, and the old ones dropped child first, so no step leaves a move without its run to allow run
  state `discarded` and move code 6. Older builds refuse a catalog at version 16 as they refuse any newer
  catalog. If another change claims migration 16 first, this one takes the next free number; the decision
  does not depend on the number.

## Alternatives considered
- **Delete the unselected moves, or the whole discarded run** - simplest, but it destroys the catalog half of
  the journal that reconciliation needs, and frees the highest run id for reuse, so a later run could share
  a quarantine folder name and a history label with a discarded one.
- **Re-plan with the selection and discard the old run** - re-derives every disposition (seconds at 1M
  files), can silently change the plan the user was looking at, and multiplies runs in the history.
- **Keep the selection only in the UI and pass it to `run`** - the stored plan, `cleanup show` and the
  confirmation would disagree about what will move, and a stopped run would forget the selection.
- **An `excluded`/`discarded_at` column added with `ALTER TABLE ADD COLUMN`** - avoids the rebuild but makes
  "planned" mean two things; every reader of the state would have to check the extra column.

## Consequences
- The CLI gains `cleanup select <run>` and `cleanup discard <run>`; the app's Cleanup panel narrows plans,
  undoes by folder and discards plans through the same service calls.
- Run counts gain `unselected` (files and bytes); lists label discarded runs.
- Migration 16 rewrites both cleanup tables once; they hold at most one row per planned file (roughly 250-300 bytes
  each, so a few hundred MB for a 1M-file plan); copying them takes seconds on an SSD, once.

## Revisit when
Cleanup history needs pruning (for example a catalog with thousands of runs), or moves gain states that a
selection should also govern.
