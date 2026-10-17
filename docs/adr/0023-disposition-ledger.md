# 0023. Disposition ledger and completion reports

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
After several runs the user must be able to answer: "Is this drive done? Is everything worth keeping already in my library?" Cleanup (ADR-0024) depends on a trustworthy answer.

## Decision
Every catalogued file has a derived **disposition**:
`exported` (this exact file was copied out), `covered` (its group's keeper was exported, i.e. a duplicate or lesser copy of something safe), `excluded` (junk / graphic / user-excluded, with reason), `pending` (a keeper or candidate not yet exported), `unreadable` (failed after retries), `missing` (gone from the source). Dispositions are computed from the catalog and the export ledger, never stored by hand. A **completion report** per source and per folder shows counts and bytes per disposition and a "100% accounted for" badge when nothing is pending or unreadable. The CLI prints the same report.

## Alternatives considered
- **Manual "done" flags on folders** - drift from reality.

## Consequences
- The cleanup feature can only act on `exported`/`covered` files.

## Revisit when
Never; dispositions may be refined.
