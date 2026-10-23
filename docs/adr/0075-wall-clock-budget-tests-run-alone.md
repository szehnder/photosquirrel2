# 0075. Wall-clock budget tests run alone, in a timing pass after the other tests

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers

## Context
A few headless UI tests assert a wall-clock or frame-time bound taken from a spec: the dashboard's 250 ms
pause/cancel, its rate-independent 8.3 ms frame p95, the shell's frame p95 while a query runs, the gallery's
100 ms jump and its headless frame cost. In `cargo xtask ci` and the Linux CI job they ran inside
`cargo test --workspace`, beside every other test binary and three sibling test threads. Diagnosing the two
dashboard flakes (change `release-hygiene`, task 4.1) showed that what they measure under that load is the
machine, not the app: the engine's own stop after a cancel went past 250 ms and the render thread's CPU time per
frame rose from ~4 ms to 10 ms, while the dashboard's own contribution stayed at a few milliseconds. The bounds are
the specs' and must not be widened.

## Decision
Tests that assert a wall-clock or frame-time budget in the UI crates are marked `#[ignore = "timing"]`, so the
main test pass skips them. `cargo xtask timing` runs exactly those tests (listed in `xtask/src/timing.rs`, which a
unit test keeps equal to the marked tests), with `--ignored --exact --test-threads=1`, and `cargo xtask ci` runs it
once right after `cargo test`, when nothing else of CI is running. Every test still runs once per CI run; spec
coverage counts them as mapped as before.

## Alternatives considered
- **Widen the bounds or `ci_slack`** - hides real regressions; the bounds are acceptance criteria (ADR-0026).
- **A cargo feature or a separate test target** - a feature would change what the main pass compiles and needs
  `cfg_attr` on every test; a separate target would split each test file's fixtures. `ignore` with a fixed reason
  is one attribute and lets `cargo test -- --ignored` run a single one by hand.
- **Serialise only within a binary (a global lock)** - does not stop the other test binaries running at the same
  time, which is where the load came from.

## Consequences
- CI takes slightly longer (the timing tests no longer overlap the rest; about a minute).
- A new budget test must be marked `#[ignore = "timing"]` and added to `xtask/src/timing.rs`, or the list check
  fails.
- Budget tests can still fail when the machine itself is loaded by something outside CI; the measured headroom is
  recorded in the change's task notes.

## Revisit when
CI moves to runners whose timing is not stable even alone, or the number of timing tests makes the serial pass slow.
