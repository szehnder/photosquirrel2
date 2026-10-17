# 0001. Record architecture decisions

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
PhotoSquirrel is built spec-first by humans and agents working in turns. Decisions made in one session must be visible, with their reasons, to the next.

## Decision
Every architecturally significant decision is recorded as a numbered Markdown file in `docs/adr/` using `template.md`. One decision per ADR. Accepted ADRs are never edited; a new ADR supersedes them. OpenSpec `design.md` files cite ADR numbers instead of restating them. New dependencies of consequence, file formats, crate boundaries and performance budgets require an ADR.

## Alternatives considered
- **Decisions only in design.md** - scattered and lost when changes are archived.

## Consequences
- `docs/adr/README.md` is the index; agents read it before changing architecture.

## Revisit when
Never.
