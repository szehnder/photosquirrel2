# 0078. CI compares control latency by the median of five runs, not the best of three (amends 0057)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0057](0057-regression-noise-floor.md)

## Context
GitHub run 39965164278 (head be70eeb, OpenSpec documents only) failed the relative performance check:
`control-latency-p95: 68.08 ms is 27% worse than the baseline 53.80 ms`. The base had measured 61.96, 53.80 and
63.82 ms (best-of-three: 53.80), the head 72.23, 69.19 and 68.08 ms (best after two re-measures: 68.08). Run
39644751071 and the cleanup work failed the same way. ADR-0057's 5%-of-bound floor (12.5 ms) did not help: the gap
was 14.3 ms.

Why this budget is so noisy:
- **The measurement is a high order statistic of a bimodal sample.** The test alternates 10 cancels and 10 pauses.
  On 4 cores (`taskset -c 0-3`, 200 trials on the reference machine) a pause took 15-68 ms (median 20.5 ms): the
  workers finish the file in hand. A cancel took 37-450 ms (median 69 ms): the same, plus the run's teardown (the
  20 ms receive-poll of the lanes and workers, two durable catalog flushes with `fsync`, the run-status commit and the
  thread joins). The p95 of 20 trials is the second largest, which is in practice the ninth of the ten cancels: the
  upper tail of the cancel teardown, whose `fsync` and scheduling on a shared VM vary most. Bootstrapped from those
  trials, one measurement has a coefficient of variation of 27% (5th-95th percentile 73-103 ms around 86 ms).
- **Best-of-three compares two minimums.** A single low base run (53.80 among 61.96 and 63.82) sets the bar, and a
  head whose three runs are all ordinary fails against it.
- Runners also differ from one another (median control latency per run 60-137 ms over 25 CI runs), but base and head
  share a runner, so that part cancels out; within a run the values spread by 22% (standard deviation of
  value/run-median over 107 CI measurements of unchanged scanner code), with single runs up to 2.3x the run median.

Two independent models agree on the false-failure rate of the current rule (every pause and cancel 10, 15, 20 or 30
ms slower is a real regression; the 15% and 12.5 ms thresholds are unchanged throughout):

| Rule (base vs head) | CI model: no change | +10 ms | +15 ms | +20 ms | +30 ms |
|---|---|---|---|---|---|
| Best of 3 vs best of up to 3 (today) | 5.4% | 35% | 61% | 82% | 96% |
| Median of 3 vs median of 3 | 2.2% | 29% | 65% | 87% | 98% |
| Median of 5 vs median of 5 | 0.6% | 23% | 69% | 91% | 99% |
| Median of 5, then 5 more head runs | **0.2%** | 15% | 65% | **90%** | **99%** |

The "CI model" resamples the 107 CI measurements (each run's values divided by its median, times a random run's
level). The trial model (bootstrapped from the 200 local trials, 3,000 simulated checks per cell) gives 4.5% for
today's rule and 0.7% for medians of five, with the same ordering of power. Observed: 2 failures of unchanged code
in 28 runs (7%).

## Decision
- A budget may set `ci_runs = n` in `perf-budgets.toml`. In `--ci`, such a budget is measured `n` times (at least
  `--best-of`) for the base and for the head, and the **medians** are compared. The base file records every run
  (`samples` in `perf-baseline-ci.json`) beside the median. A suspected regression measures the head `n` more times,
  and only the median of all its runs counts. The thresholds stay as ADR-0026 and ADR-0057 set them (15% and more
  than 5% of the bound).
- `control-latency-p95` sets `ci_runs = 5`. The other budgets keep best-of-three and re-measures. Their best runs are
  stable on runners: in run 39965164278 every other budget's three base runs were within 4% of each other
  (except near-dup-lookup-p95, whose 0.01 ms against a 1 ms bound sits far below the floor).
- A base without `samples` was measured by an older xtask, which took the best of three. The head is then compared
  with it by its best run, as before, with a warning. This happens only on the first push after this change.
- The absolute bound is unchanged: 250 ms is checked against the measurement in local mode
  (`cargo xtask bench --check`), and the test asserts it in release builds. ADR-0076's timing pass still asserts the
  250 ms pause and cancel scenarios on every CI run.

Checked on the reference machine, pinned to 4 cores like a runner (`taskset -c 0-3`). A base of five runs (97.4,
88.0, 81.9, 85.2 and 102.0 ms, median 88.0 ms) checked against the same code passed with a median of 88.6 ms. With a
20 ms sleep injected into `Scanner::pause` and `Scanner::cancel` (a scratch build, not committed), the head
measured a median of 103.9 ms over five runs and 111.2 ms over ten, so the check failed: "26% worse than the
baseline 88.02 ms".

## Alternatives considered
- **Interleave head and base, or re-measure both on a failure.** This is the soundest design, because drift
  during the job cancels out. But the base's test binaries do not survive the head build: both commits build into one
  target directory, and cargo hashes path crates relative to the workspace root, so the binaries collide. Keeping
  them means building and running copies of the test binaries outside cargo, or giving the base a second target
  directory (about 20 minutes more of compilation). The CI data shows no head-versus-base drift to cancel: the head's
  first run was below the base median in 13 of 24 runs.
- **More trials per measurement, with a pooled p95.** Pooling 100 trials (5 x 20) gave 1.5% false failures with
  similar power. It needs the test to print its raw trials and the xtask to pool them. The median of five gets the
  same result from the one `measure` line every source already prints.
- **A larger floor for this budget only.** About 25 ms (10% of the bound) would have passed this run. But it lowers
  the power at +20 ms and moves the noise elsewhere instead of removing its cause: the minimum statistic.
- **Medians of five for every budget.** Their best-of-three is already stable, and it would add about 50 minutes of
  measurement to the job.

## Consequences
- The perf job measures control latency 10 times instead of 4 (about 43 s each on a runner), so it takes about
  4 minutes longer, plus another 3.6 minutes when a regression is suspected.
- The false-failure rate on unchanged code drops from about 5% to about 0.2% per run. A 20 ms regression in the pause
  or cancel path is caught 90% of the time instead of 82%.
- `perf-baseline-ci.json` gains a `samples` map. The committed `perf-baseline.json`, which is written in local mode,
  does not.

## Revisit when
Another budget flakes on identical code (give it `ci_runs`), or the perf job gets a target directory of its own for
the base, so that interleaving costs no extra build.
