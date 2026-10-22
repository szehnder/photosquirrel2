# Proposal

## Why

"Extremely performant" only holds if it is measured and defended. Each engine and UI change added its own
benchmark; before release every ADR-0026 budget must be measured together on the reference machine, recorded as
a baseline, and guarded against regressions in CI, and every spec scenario must have its test.

## What Changes

- Audit `perf-budgets.toml` so every ADR-0026 budget (and the derived component budgets added by earlier changes)
  has exactly one entry with a measurable source.
- `cargo xtask bench --check` locally (absolute + baseline), `--ci` in CI (relative > 15% regressions only, no
  window measurements), `--update-baseline` writing `perf-baseline.json`.
- Reference-machine description and how to measure (`docs/performance.md`).
- Baseline recorded for v1; CI job running the relative check on every push to main.
- Spec-coverage enforcement: archived scenarios without a `scenario_*` test fail `cargo xtask ci`.

## Capabilities

### New Capabilities
- `performance-budgets`: measuring every performance budget, recording baselines and catching regressions.

### Modified Capabilities
- None.

## Impact

- ADRs: 0025, 0026.
- Crates: `xtask` (`bench`, `spec-coverage`), all crates with benches (no behaviour changes), `.github/workflows/ci.yml`,
  `perf-budgets.toml`, `perf-baseline.json`, `docs/performance.md`.

## Out of scope

- New optimisations (if a budget is missed, the owning change reopens with a fix); continuous benchmarking
  dashboards; per-OS absolute budgets.
