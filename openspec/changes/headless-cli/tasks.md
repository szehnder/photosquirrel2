# Tasks

## 1. Conventions

- [x] 1.1 Output envelope, human/JSON renderers and exit-code mapping; verify `scenario_cli_usage_error_exits_2` and `scenario_cli_failure_exits_1` (`out.rs` envelope, exit codes 0/1/2/130 in `main.rs`)
- [x] 1.2 Global `--data-dir` / `PHOTOSQUIRREL_DATA_DIR`; verify `scenario_cli_data_dir_override`

## 2. Commands

- [x] 2.1 `sources add|list|relink|remove` with `--yes` handling; verify `scenario_cli_non_interactive_needs_yes` (`sources add|list|relink|remove`; removal refuses without `--yes` off a terminal)
- [x] 2.2 `scan` with live human progress and NDJSON; verify `scenario_cli_scan_streams_json_snapshots` (NDJSON snapshots >= 200 ms apart plus a summary line; live progress line on a terminal, every 5 s otherwise)
- [x] 2.3 Interrupt handling and `scan --resume`; verify `scenario_cli_ctrl_c_cancels_scan` and `scenario_cli_resume_an_interrupted_run` (`ctrlc` crate, workspace dependency; cancel within 1 s on the medium corpus, `scan --resume` continues the same run id; plus `scan pause|resume|cancel` from another terminal via `<data-dir>/cli/`)
- [x] 2.4 `status` (runs and active snapshot); verify `insta` snapshot of `status --json` on the small corpus (shape snapshot `tests/snapshots/cli__status_json_shape.snap` (keys and types; values vary per run))
- [x] 2.5 `groups list|show|set-keeper|separate|suggestions`; verify `scenario_cli_groups_explain_keeper` and `scenario_cli_set_keeper_from_cli` (`groups list|show|set-keeper|clear-keeper|separate|merge|suggestions|explain|regroup`)
- [x] 2.6 `doctor`; verify `scenario_cli_doctor_lists_decoders`
- [x] 2.7 `fixtures evaluate`; verify `scenario_cli_evaluate_fails_below_gate` (also as top-level `eval-corpus`)

## 3. Cross-cutting

- [x] 3.1 Help text for every subcommand; verify `scenario_cli_every_subcommand_has_help`
- [x] 3.2 JSON for every subcommand plus `insta` snapshots; verify `scenario_cli_json_output_parses`

## 4. Verification

- [ ] 4.1 `cargo xtask ci` is green
- [x] 4.2 Gate: a script runs fixtures generate, sources add, scan, groups list and fixtures evaluate with `--json` end to end and every step exits 0 (`gate_script_runs_end_to_end` in `photosquirrel-cli/tests/cli.rs`)
- [x] 4.3 `cargo xtask spec-coverage` reports every scenario of this change mapped (every `cli` scenario mapped)
- [x] 4.4 `openspec validate headless-cli --strict` passes
