# Design

## Context

ADR-0026 lists the budgets and makes them acceptance criteria; benchmarks are deliverables of the changes that
introduced the measured code, so by this point every source exists. ADR-0025 makes spec coverage part of
`cargo xtask ci`. `xtask/src/bench.rs` and `xtask/src/spec_coverage.rs` already implement most mechanics
(scaffolded by `bootstrap-workspace`); this change completes, audits and records.

## Goals / Non-Goals

**Goals:**
- One command proves every budget on the reference machine; CI blocks regressions.

**Non-Goals:**
- Making a missed budget pass (that is a fix in the owning crate, tracked as a task here if needed).

## Decisions

- **Budget entries.** `perf-budgets.toml` `[[budget]]` with `name, metric, max, unit, change, source`. All
  budgets are upper bounds; throughput budgets are stored as time per unit (catalog insert: 20 µs/row; analysis:
  6.67 ms/file wall time) so `evaluate` stays a single comparison. Derived component budgets
  (`analysis-decode-thumb`, `near-dup-index-build`, `corpus-small-generate`) are tracked for regressions with
  `change` naming their owner; they are not ADR budgets and do not count toward the eleven.
- **Sources.** `criterion` for pure benches (hashing, lookup, catalog), `test` for integration measurements
  printing `measure <name>=<v> <unit>` (rescan, analysis, control latency), `app` for window measurements
  (`--measure-startup`, `--measure-idle-memory`, `--render-bench`). The `App` source's legacy `connection` field
  (inherited from the bootstrap template) is replaced by `seeded: bool` (generate a synthetic 1M catalog in a
  temp data dir before launching), matching the existing unit tests.
- **Reference machine.** Recorded in `docs/performance.md`: CPU (16 cores), RAM, NVMe SSD, GPU, OS, power
  profile; warm-cache procedure (`cat` corpus to `/dev/null` twice). Absolute checks run only there (`Mode::Local`).
- **CI.** `ci.yml` job `bench-regressions` runs `cargo xtask bench --check --ci` on Linux after tests; noise is
  absorbed by the 15% threshold and criterion medians.
- **Coverage.** `cargo xtask spec-coverage --enforce` fails on archived unmapped scenarios (active changes only
  reported), as implemented; `cargo xtask ci` calls it.

## Risks / Trade-offs

- [CI runner noise exceeds 15%] -> medians over more samples; if still flaky, mark specific budgets `ci = false`.
- [Reference machine drift] -> baseline re-recorded only with a commit message stating the reason.

## Verification

- `cargo test -p xtask` (all `scenario_performance_budgets_*`).
- `cargo xtask bench --list`, `cargo xtask bench --check` (reference machine), `cargo xtask bench --check --ci`.
- `cargo xtask spec-coverage --enforce`, `cargo xtask ci`, `openspec validate performance-budgets --strict`.
