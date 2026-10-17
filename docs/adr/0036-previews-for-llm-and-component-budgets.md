# 0036. LLM previews and component budgets

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
ADR-0020 asks for a 1024 px preview "from the thumbnail pipeline", but ADR-0016 defines only 384 px stored thumbnails and 2048 px on-demand previews. ADR-0026 states throughput minimums, while `cargo xtask bench` checks lower-is-better values.

## Decision
LLM requests use a 1024 px JPEG downscaled from the 2048 px preview (generated from the source on demand; photos whose source is offline are deferred, not failed). All budgets in `perf-budgets.toml` are lower-is-better: throughputs are expressed as time per unit (e.g. 20 us per catalog row = 50k rows/s; 6.67 ms per 12 MP JPEG = 150 files/s). Component budgets derived inside OpenSpec designs (e.g. single-core decode time) may be added to `perf-budgets.toml` without a new ADR as long as they serve an ADR-0026 budget.

## Alternatives considered
- **Store a 1024 px tier** - more disk for little gain.

## Consequences
- `bench evaluate` stays simple.

## Revisit when
Never.
