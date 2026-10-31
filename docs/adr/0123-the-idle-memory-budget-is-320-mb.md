# 0123. The idle-memory budget is 320 MB

- Status: accepted
- Date: 2026-10-31
- Deciders: maintainers
- Supersedes: the idle-RSS bound of [0026](0026-performance-budgets.md) ("idle RSS <= 250 MB"); the rest of 0026
  stands

## Context
ADR-0026 bounds the app's resident memory after 5 s idle on the 1M-item synthetic catalog (`idle-rss`) at 250 MB.
The baseline of 209.6 MB was recorded on 2026-10-21. Measured now:
- 267-276 MB in both 0.3.0 and the current HEAD;
- 290.6 MB in one run on the owner's desktop.

The growth came across earlier releases (decoders, the colour pipeline, larger caches), not in 0.3.1. Holding
250 MB would mean trimming features that the owner wants. The owner chose to raise the budget.

## Decision
`idle-rss` is bounded at **320 MB**. That is 10% above the worst observation (290.6 MB) and about 17% above the
typical one. It applies to:
- `perf-budgets.toml`;
- the app-shell requirement "Start-up time and idle memory" and its scenario "Idle memory within 320 MB";
- the performance-budgets requirement "Every budget defined".

The measurement is unchanged: the release app, `--synthetic-catalog 1000000 --measure-idle-memory 5`. The
reference value in `perf-baseline.json` stays 209.6 MB until it is measured again with
`cargo xtask bench --update-baseline` on the reference machine with an unlocked desktop.

## Alternatives considered
- **300 MB** - only 3% of headroom over a measurement whose runs spread by about 9%. It would fail at random.
- **350 MB** - a 25% growth would pass unnoticed.
- **Keep 250 MB and cut memory** - no single large consumer has been identified. The reader pools are already
  unmapped (ADR-0026's reader note: mapped pages once made the idle app 692 MB).

## Consequences
- The budget fails again on growth of about 30 MB over today's typical value.
- Release notes do not mention it. It changes no behaviour.

## Revisit when
Idle RSS is consistently under 250 MB again (tighten it), or a measurement exceeds 300 MB (find the consumer
before raising the budget again).
