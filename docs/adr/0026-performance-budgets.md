# 0026. Performance budgets as acceptance criteria

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
"Extremely performant" must be measurable or it regresses silently.

## Decision
`perf-budgets.toml` defines budgets checked by `cargo xtask bench --check` (absolute on the reference machine, relative >15% regressions in CI):
- Gallery: p95 frame <= 8.3 ms while scrolling 1M items with warm textures; first gallery frame after start-up with a 1M catalog <= 600 ms.
- Cold start to first frame <= 400 ms; idle RSS <= 250 MB.
- Catalog: >= 50k file rows/s inserted; ID-list query for 1M items <= 300 ms.
- No-change rescan of 100k files (warm cache) <= 4 s.
- Analysis of 12 MP JPEGs: >= 150 files/s on 16 cores (warm cache).
- pHash+dHash per image (from 512 px decode) <= 0.3 ms; near-dup lookup over 1M hashes <= 1 ms p95.
- Pause/cancel latency <= 250 ms.

## Alternatives considered
- **Ad hoc profiling** - no regression protection.

## Consequences
- Benchmarks are deliverables of the changes that introduce the code they measure.

## Revisit when
Budgets are consistently beaten by 2x - tighten them.
