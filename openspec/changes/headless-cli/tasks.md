# Tasks

## 1. Conventions

- [ ] 1.1 Output envelope, human/JSON renderers and exit-code mapping; verify `scenario_cli_usage_error_exits_2` and `scenario_cli_failure_exits_1`
- [ ] 1.2 Global `--data-dir` / `PHOTOSQUIRREL_DATA_DIR`; verify `scenario_cli_data_dir_override`

## 2. Commands

- [ ] 2.1 `sources add|list|relink|remove` with `--yes` handling; verify `scenario_cli_non_interactive_needs_yes`
- [ ] 2.2 `scan` with live human progress and NDJSON; verify `scenario_cli_scan_streams_json_snapshots`
- [ ] 2.3 Interrupt handling and `scan --resume`; verify `scenario_cli_ctrl_c_cancels_scan` and `scenario_cli_resume_an_interrupted_run`
- [ ] 2.4 `status` (runs and active snapshot); verify `insta` snapshot of `status --json` on the small corpus
- [ ] 2.5 `groups list|show|set-keeper|separate|suggestions`; verify `scenario_cli_groups_explain_keeper` and `scenario_cli_set_keeper_from_cli`
- [ ] 2.6 `doctor`; verify `scenario_cli_doctor_lists_decoders`
- [ ] 2.7 `fixtures evaluate`; verify `scenario_cli_evaluate_fails_below_gate`

## 3. Cross-cutting

- [ ] 3.1 Help text for every subcommand; verify `scenario_cli_every_subcommand_has_help`
- [ ] 3.2 JSON for every subcommand plus `insta` snapshots; verify `scenario_cli_json_output_parses`

## 4. Verification

- [ ] 4.1 `cargo xtask ci` is green
- [ ] 4.2 Gate: a script runs fixtures generate, sources add, scan, groups list and fixtures evaluate with `--json` end to end and every step exits 0
- [ ] 4.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 4.4 `openspec validate headless-cli --strict` passes
