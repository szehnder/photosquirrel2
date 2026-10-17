# 0019. Progress as coalesced snapshots, not per-file events

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Rich progress (per stage, per source, throughput, ETA, recent discoveries) is required, but sending one UI event per file at thousands of files per second would swamp the UI thread.

## Decision
The engine maintains progress in shared atomic counters plus a small mutex-guarded "recent" ring (last 64 photos found, last 32 errors, current directory per lane, throughput samples). The UI samples a `ProgressSnapshot` at most every 100 ms (and on demand) while a run is active. The CLI renders the same snapshot. Durable facts go to the catalog; the snapshot is ephemeral.

## Alternatives considered
- **Event stream per file** - O(files) UI work and allocations.

## Consequences
- Progress cost is constant regardless of scan rate.

## Revisit when
Never.
