# 0008. Resumable staged pipeline with cooperative cancellation

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Scans take hours. They are interrupted by sleep, unplugged drives, crashes and the user pressing Cancel. Resuming must not repeat finished work.

## Decision
Each file advances through persisted **stages**: `discovered -> probed -> analyzed -> thumbed` (plus `skipped`, `failed`). A stage is recorded in the catalog in the same batch as its results, so the catalog is always a consistent checkpoint. A **scan run** row records status (`running`, `paused`, `cancelled`, `completed`, `interrupted`). Resume = walk again (cheap, ADR-0007), then process every file whose stage is incomplete. On start-up, a run left `running` is shown as `interrupted` with a one-click Resume. Cancellation and pause are cooperative: a shared token is checked between files and between read chunks; in-flight batches are flushed before the run stops. Failed files record their error and are retried at most twice on later runs.

## Alternatives considered
- **Persisted walk cursor** - more complex and unnecessary given cheap stat diffs.
- **In-memory queue only** - loses all progress on crash.

## Consequences
- Pause/cancel respond within 250 ms; no partial results are ever visible as complete.

## Revisit when
Never; stages may be added (e.g. `classified` for AI, which has its own queue, ADR-0020).
