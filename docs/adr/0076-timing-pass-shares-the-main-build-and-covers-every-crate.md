# 0076. The timing pass shares the main pass's build and covers every crate (amends 0075)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0075](0075-wall-clock-budget-tests-run-alone.md)

## Context
ADR-0075 moved the UI crates' wall-clock budget tests into a timing pass and expected it to add "about a minute".
The pass ran `cargo test -p ui-workspace ...` and `-p ui-gallery ...`, while the main pass runs
`cargo test --workspace`. Cargo resolves features for the packages a command selects, so `-p` gave a different
feature set: on GitHub run 39918678832 the timing pass compiled 378 crates and took 22 minutes of the Linux job
(07:59:06 to 08:21:20), left a second, differently featured build in the target directory, and ran the timing tests
against features the main pass never tested.

The pass also covered the UI crates only. ps-scan's 250 ms pause and cancel scenarios and the CLI's "exits within
1 s of Ctrl-C" scenario assert a spec's wall-clock bound in debug builds and still ran inside the parallel main
pass, where the release-hygiene diagnosis (task 4.1) measured the engine taking 252-516 ms to stop. The macOS and
Windows jobs run the engine crates' tests and no timing pass at all. And the list check found markers only as the
exact line `#[ignore = "timing"]` in top-level `crates/*/tests/*.rs` files, so a marker elsewhere, or spelt another
way, would hide a test from both passes.

## Decision
- The timing pass selects packages exactly as the main pass does: `cargo test --workspace --test <target>... --
  --ignored --exact --test-threads=1 <names>`, one invocation. Same selection, same features, same build: after the
  main pass nothing compiles but, at most, the test binaries. A unit test fails if the pass uses `-p`.
- The rule covers every crate: a test that asserts a spec's wall-clock or frame-time bound in debug builds is a
  timing test. Added: `scenario_scan_control_cancel_within_250_ms` and `scenario_scan_control_pause_within_250_ms`
  (ps-scan) and `scenario_cli_ctrl_c_cancels_scan` (photosquirrel-cli; the resume scenario that shares its
  interrupted scan no longer asserts the bound). Not timing tests: release-only budget tests (the `budgets` tests of
  ps-catalog, ps-hash, ps-export and ps-scan, ps-dedup's regroup budget and the small-corpus preset assert only when
  `debug_assertions` is off) and guards that only catch a hang, with margins of several times the work they wait for
  (catalog lock retry, start-up grace, fuzzed probe per case, wait-until timeouts).
- The macOS and Windows jobs run their engine crates' timing tests in a step after their main tests, with the same
  `-p` list (so again the same features), `PHOTOSQUIRREL_TEST_SCALE`, `--ignored --exact --test-threads=1`. A unit
  test keeps that step equal to the engine entries of the list.
- The list check reads every `.rs` file under every crate (`src/`, `tests/` and nested directories) and fails on
  any `ignore = "timing"` text (whitespace ignored, `cfg_attr` included) that is not the exact, unindented marker
  line on a test of a top-level integration test file, the only place `--test <target> -- --exact <name>` reaches.

## Alternatives considered
- **Keep `-p` and pass the main pass's features explicitly** - the features cargo unifies for the workspace are not
  written anywhere; reproducing them by hand drifts.
- **Leave the engine's 250 ms tests in the main pass** - they measure the loaded machine, as ADR-0075 found for the
  dashboard; the bound is the spec's and is not widened.
- **A full `cargo xtask timing` on macOS and Windows** - it selects the whole workspace, which those jobs do not build
  or test (the UI crates are tested on Linux).

## Consequences
- The timing pass costs its own run time (about a minute and a half on the reference machine) instead of a second
  build; the Linux job is back to roughly its length before ADR-0075.
- The timing pass and the main pass only share a build when cargo runs with the same environment: `cargo xtask ci`
  runs both. (Build scripts that track `CARGO_*` variables rebuild when a plain shell `cargo test` follows a
  `cargo xtask` run, and the other way round.)
- A new timing test anywhere must carry the exact marker in a top-level integration test file and be listed in
  `xtask/src/timing.rs`; an engine one also goes into the macOS/Windows timing step.

## Revisit when
The workspace's package selection changes (a crate left out of `--workspace`), or the platform jobs start testing
the UI crates.
