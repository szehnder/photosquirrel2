# Tasks

## 0. Carried over from scanning
- [ ] 0.1 Bring `scan-analysis-12mp` within 6.67 ms/file (7.42 ms measured at M1 re-review; profile: 1/4-scale JPEG decode ~37 ms CPU per 12 MP file). Options: SIMD decoder with DCT scaling (libjpeg-turbo; new ADR), buffer reuse to cut sys time, workers sized to physical cores. Verify `scenario_scanning_analysis_throughput_within_budget` and `cargo xtask bench --check --only scan-analysis-12mp`

## 1. Budgets file

- [ ] 1.1 Audit `perf-budgets.toml` against ADR-0026 and add any missing entry; verify `scenario_performance_budgets_all_adr_budgets_defined`
- [ ] 1.2 `bench --list` prints owner change; verify `scenario_performance_budgets_list_shows_every_budget`
- [ ] 1.3 Replace the `App` source's `connection` field with `seeded`; verify `cargo test -p xtask` compiles and passes

## 2. Checks

- [ ] 2.1 Absolute and missing-measurement failures; verify `scenario_performance_budgets_absolute_budget_exceeded_fails` and `scenario_performance_budgets_missing_measurement_fails`
- [ ] 2.2 Regression detection; verify `scenario_performance_budgets_regression_detected`
- [ ] 2.3 CI mode; verify `scenario_performance_budgets_ci_mode_checks_regressions_only`
- [ ] 2.4 Baseline merge on update; verify `scenario_performance_budgets_baseline_update_writes_values`
- [ ] 2.5 Coverage enforcement; verify `scenario_performance_budgets_missing_scenario_test`

## 3. Measurement and baseline

- [ ] 3.1 Write `docs/performance.md` (reference machine, warm-cache procedure, how to read results); verify the file lists every budget name from `cargo xtask bench --list`
- [ ] 3.2 Run the full check on the reference machine, fix or reopen owning changes for misses; verify `scenario_performance_budgets_all_budgets_within_limits` (recorded output of `cargo xtask bench --check`)
- [ ] 3.3 Record and commit `perf-baseline.json`; verify `cargo xtask bench --check --ci` passes against it
- [ ] 3.4 Add the `bench-regressions` job to `.github/workflows/ci.yml`; verify a green CI run on main

## 4. Verification

- [ ] 4.1 `cargo xtask ci` is green
- [ ] 4.2 Gate: all budgets measured on the reference machine and the baseline committed
- [ ] 4.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 4.4 `openspec validate performance-budgets --strict` passes
