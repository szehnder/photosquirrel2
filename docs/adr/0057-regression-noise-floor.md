# 0057. Regressions must also exceed 5% of the budget's bound

- Status: accepted
- Date: 2026-10-21
- Deciders: maintainers

## Context
ADR-0026 fails a check when a measurement is more than 15% worse than its baseline. On shared CI runners some
budgets vary by more than that from run to run while sitting far below their bound: `control-latency-p95` measured
84, 75, 53 and 66 ms on essentially the same code against a 250 ms bound, and run 39644751071 failed with 61.95 ms
against a base of 53.11 ms (17%) even after the best-of-three re-measure. A relative threshold alone treats a few
milliseconds of scheduler noise on a fast measurement as a regression.

## Decision
A measurement is a regression only when it is more than 15% worse than its baseline **and** worse by more than 5%
of the budget's bound (`NOISE_FLOOR` in `xtask/src/bench.rs`). This applies locally and in CI. The absolute bound
check is unchanged.

## Alternatives considered
- **A per-budget tolerance in `perf-budgets.toml`** - more knobs, each tuned by hand after a flake.
- **More re-measures** - the base and head already get best-of-three; the spread survives it and doubles CI time.
- **Drop control latency from CI** - it is the budget most likely to catch a blocked control path.

## Consequences
- A budget at a tenth of its bound can drift up to 50% before CI fails it; a budget near its bound is still held
  to 15%. Budgets near their bound are the ones a regression would break.
- Amends ADR-0026's regression rule.
