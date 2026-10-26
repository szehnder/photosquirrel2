# 0099. A cleanup run re-checks its files when the catalog changes under it, and edits wait while it runs

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0096](0096-cleanup-keeps-copies-better-than-a-chosen-keeper-and-rechecks-plans.md) (when a run
  derives the dispositions of its planned files), [0065](0065-one-file-changing-operation-at-a-time.md) (what the operation
  slot excludes)

## Context
ADR-0096 made a run derive the dispositions of its planned files once, when it starts. The final gate review of
`heic-system-decoders` found that this is not enough: group edits (`set_keeper`, `clear_keeper`, `separate`,
`merge`, `undo_merges`) and the Library's include/exclude decisions do not take the operation slot, and the app did
not block them during a run. A `QuarantineHooks::after_rename` hook that separates a covered resize from its group
after the run's first move made that resize its own photo with no copy in the library, and the run then
quarantined its only copy, against the owner's first priority.

## Decision
- **The catalog keeps a constraints generation.** The writer bumps an in-memory counter after every commit that
  can change what makes a file safe to move: keeper overrides, separations, merges, regroups (`apply_groups`),
  export-ledger rows, user decisions and every scan write of file rows. Cleanup-journal writes, review marks and
  the AI queue do not bump it. The catalog has a single writer process (ADR-0005), so an in-memory counter sees
  every such write; it is bumped after the commit, so a reader that sees the new value sees the write.
- **A run compares it before every move.** `quarantine::run` gets a `Recheck` (the generation and a way to derive
  holds for given planned files). The service reads the generation before deriving the run's first holds; when it
  has changed before a move, the holds of the files still to move are derived again (the ADR-0096 derivation) and
  a file no longer safe is skipped as "no longer safe". If that derivation fails, the file is skipped (never
  moved) and the next file tries again.
- **Edits wait while a run moves files.** Group and keeper edits, include/exclude decisions and AI suggestion
  decisions take an edit guard from the service (`ScanService::edit`): refused with `Busy::Cleanup` ("a cleanup is
  running ...") while a cleanup run holds the slot, and a cleanup run does not start while an edit is being saved
  (`Busy::Edit`). The app reads a lock-free flag (`ScanService::cleanup_running`) while rendering and does not
  offer those edits during a run (Duplicates view, inspector, AI review), saying why; they return when it ends.

## Alternatives considered
- **Edits take the operation slot** - they would also be refused during scans and exports, where they are safe
  today, and a slow regroup would hold the slot that the UI's status reads.
- **Only refuse edits** - writes that do not go through the service's edit path (a regroup, a future command)
  would still change safety under a run; the generation check covers them all.
- **Persist the generation** - unnecessary with one writer process, and a write per edit for nothing.
- **Re-derive before every move** - a full derivation per file is quadratic at a million files; the generation
  makes it once per change.

## Consequences
- Every file a run moves was safe by the dispositions committed before its move was decided.
- An edit made during a run is refused with a clear message instead of racing it; the user repeats it after.
- A change during a run costs one more derivation of dispositions per change.

## Revisit when
Cleanup runs in a process other than the catalog's writer, or edits need to proceed during long runs (then queue
them until the run ends).
