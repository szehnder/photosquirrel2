# 0033. Users can split members out of a duplicate group

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
ADR-0013 has no correction path for a false merge. A unique photo wrongly merged into a group becomes `covered` (ADR-0023) and would never be exported and could later be quarantined (ADR-0024). That is exactly the loss the product exists to prevent.

## Decision
The duplicate-review UI and CLI let the user **separate** any member from its group (and **merge** two groups). Separations are persisted as "never pair" constraints between content identities and survive regrouping; a separated member forms its own group (and is its own keeper). Merges are persisted as "always pair" constraints. Constraints win over the automatic rules.

## Alternatives considered
- **Only allow keeper override** - does not rescue a wrongly merged unique photo.

## Consequences
- Cleanup must respect the latest constraints: a file separated after export is `pending` again until its own copy is exported.

## Revisit when
Never.
