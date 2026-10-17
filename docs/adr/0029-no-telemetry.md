# 0029. No telemetry

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
This is a personal tool handling private family photos.

## Decision
PhotoSquirrel sends nothing anywhere except LLM requests the user configured and started (ADR-0021). No analytics, crash reporting or update pings.

## Alternatives considered
- **Opt-in crash reports** - maybe later, as a separate ADR.

## Consequences
- Diagnostics are local log files the user can share manually.

## Revisit when
Never without a new ADR.
