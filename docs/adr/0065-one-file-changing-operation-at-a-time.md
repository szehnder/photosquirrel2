# 0065. One file-changing operation at a time per catalog

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
Cleanup checked for a running scan only when a run started; a scan or an export could start during a cleanup,
cleanup run, undo and reconciliation did not exclude each other, and reconciliation (called by plan, run and
the Cleanup history) treated a live `running` run as crashed and rewrote its moves.

## Decision
`ScanService` has one operation slot. An export, a cleanup plan, run or undo and cleanup reconciliation take it
(`ScanService::begin`), refused while a scan is active or another operation holds it. Starting or resuming a
scan and adding, re-linking or removing a source are refused while it is held, and run with the slot locked so
none can start meanwhile. Reconciliation that cannot take the slot is skipped (the next command does it), so it
never touches a run that is live in this process. Across processes the catalog's owner lock (ADR-0005,
ADR-0042) already admits a single writer, and every one of these operations needs it.

## Alternatives considered
- **A lock file per operation** - the owner lock already serialises writers across processes; a second
  mechanism would only add stale-lock handling.
- **Let exports run during scans** - nothing needed it, and a scan may re-key the files an export reads.

## Consequences
- An export or cleanup started during a scan is refused with a message naming what runs; the UI shows it.
- Dry runs, reports and listings take no slot and stay available.

## Revisit when
Exports are wanted during scans (they would need their own consistency argument).
