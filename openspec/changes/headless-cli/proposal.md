# Proposal

## Why

Agents, CI and power users cannot click through a GUI. Every engine feature must be scriptable and verifiable
from a terminal with stable machine-readable output before the UI is built on the same facade (ADR-0027). It is
also how the user will run long scans of a NAS overnight.

## What Changes

- `photosquirrel-cli` subcommands over `ps-service`: `sources` (add, list, relink, remove), `scan` (start, resume,
  live progress), `status` (runs, active snapshot), `groups` (list, show/explain, set-keeper, separate,
  suggestions), `doctor`, `fixtures` (generate from `ground-truth-corpus`, plus `evaluate`).
- Human output by default; `--json` everywhere with a versioned envelope; NDJSON progress for `scan --json`.
- Shared data directory with the app, overridable with `--data-dir` / `PHOTOSQUIRREL_DATA_DIR`.
- Exit-code contract; Ctrl-C cancels cooperatively and leaves a resumable run.
- Non-interactive safety: confirmations require `--yes` when stdin is not a terminal.
- `report`, `export` and `ai` subcommands are added by `export` and `ai-insights` on the same conventions.

## Capabilities

### New Capabilities
- `cli`: the headless command-line interface to every engine feature, with human and JSON output.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0008, 0017, 0019, 0025, 0027, 0029, 0031.
- Crates: `photosquirrel-cli` (clap commands, renderers), `ps-service` (any facade gaps found while wiring).

## Out of scope

- Export, report and AI subcommands (their own changes); a TUI; shell completions beyond clap's generator.
