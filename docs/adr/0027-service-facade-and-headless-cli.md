# 0027. Service facade and a headless CLI as verification surface

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Agents and CI cannot click through a GUI reliably. The UI and any automation need the same engine API.

## Decision
`ps-service` is the only API the UI and the CLI use: sources, scans (start/pause/resume/cancel, snapshots), library queries, groups, AI queue, exports, reports and settings. `photosquirrel-cli` exposes it as subcommands (`scan`, `status`, `report`, `groups`, `export --dry-run`, `ai`, `doctor`, `fixtures`) with human and `--json` output. Behaviour is verified through the CLI and tests first; the GUI is verified with headless UI tests and screenshots.

## Alternatives considered
- **UI calls engine crates directly** - duplicated orchestration, untestable.

## Consequences
- Every engine feature is scriptable.

## Revisit when
Never.
