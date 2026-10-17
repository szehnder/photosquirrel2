# 0034. A `quarantined` disposition

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
ADR-0023 lists no disposition for files moved by cleanup (ADR-0024); they would appear `missing`, which reads as data loss.

## Decision
Add `quarantined`: the file was moved by a cleanup run into the source's quarantine folder and the journal records it. Completion reports count it as accounted for. Undo returns it to its previous disposition.

## Alternatives considered
- **Reuse `missing`** - alarming and ambiguous.

## Consequences
- `Disposition` gains a variant; reports gain a column.

## Revisit when
Never.
