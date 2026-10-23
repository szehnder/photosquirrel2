# Tasks

## 1. Decision

- [x] 1.1 ADR-0072 "CLI scans pause on battery and resume on mains power" moved from proposed to accepted (verify: status line reads accepted and the `docs/adr/README.md` row links the file)

## 2. Shared pause rule (`ps-service::power`)

- [x] 2.1 Extract `PowerPolicy` (`ResumeOnMains::{Offer, Auto}`, `tick`, `user_paused`, `note`, `PowerEvent`) from `PowerWatch::tick`; `PowerWatch` delegates with `Offer` (verify: `scenario_settings_scan_pauses_on_battery` passes unchanged, and the existing `power.rs` unit tests pass)
- [x] 2.2 `probe_from_env()` with `PHOTOSQUIRREL_POWER_SUPPLY_DIR` over `SysfsPower` (verify: unit test in `power.rs` that a temp folder with `BAT0` discharging and no online mains reads battery, an empty variable falls back to `SystemPower`)
- [x] 2.3 `Auto` resumes only runs it paused, on a slow `ScanService` (`max_readers: 1`, `read_delay`) with a fake source in `crates/ps-service/tests/pause_on_battery.rs` (verify: `scenario_settings_cli_scan_pauses_on_battery`, `scenario_settings_cli_scan_resumes_on_mains_power`, `scenario_settings_cli_setting_off_does_not_pause`)
- [x] 2.4 `user_paused` and the resume-on-battery override with `Auto` (verify: `scenario_settings_cli_user_pause_not_auto_resumed`, `scenario_settings_cli_resume_on_battery_overrides`)

## 3. CLI wiring (`photosquirrel-cli`)

- [x] 3.1 `--ignore-battery` on `ScanStartArgs` and `scan resume` in `main.rs`; `start()` builds an `Option<PowerGuard>` from the setting and the flag (verify: `scenario_cli_every_subcommand_has_help` still passes and `scan --help` lists the flag)
- [x] 3.2 `foreground()` ticks the guard at start and every second, never after a cancel; `pause` requests mark the pause as the user's (verify: new binary test file `crates/photosquirrel-cli/tests/power.rs`, using `PHOTOSQUIRREL_POWER_SUPPLY_DIR`: `scenario_settings_cli_scan_started_on_battery_pauses_at_once`, `scenario_settings_cli_ignore_battery_flag`)
- [x] 3.3 Power output: stderr transition lines, `paused on battery` in `progress_line`, `power` NDJSON lines (`PowerLine`), paused snapshots and plain lines at most every 5 s (verify: `scenario_cli_power_pause_json_lines`, `scenario_cli_paused_snapshots_throttled`, `scenario_cli_power_pause_on_stderr`, and `scenario_cli_scan_streams_json_snapshots` unchanged)
- [x] 3.4 Heartbeat `LiveScan` with `power_pause`; `scan status` text and JSON show it; `control_live` still parses (verify: `scenario_cli_status_shows_power_pause`; `scan pause`/`scan resume` from a second process in `tests/power.rs` still report the new status)
- [x] 3.5 Interrupt while paused on battery (unix, in `tests/power.rs`, serialized like `tests/interrupt.rs`) (verify: `scenario_cli_ctrl_c_while_paused_on_battery`)

## 4. Documentation

- [x] 4.1 README CLI section: pause on battery applies to CLI scans, auto-resume, `--ignore-battery`, the `power` NDJSON line and `PHOTOSQUIRREL_POWER_SUPPLY_DIR` (verify: README diff reviewed; `cargo xtask ci` passes)

## 5. Verification

- [x] 5.1 `cargo test -p ps-service --test pause_on_battery` and `cargo test -p photosquirrel-cli --test power` pass (with `CARGO_BUILD_JOBS=4`, `--test-threads=4`, TMPDIR under `target/`)
- [x] 5.2 `cargo xtask spec-coverage` maps every scenario of this change
- [x] 5.3 `cargo xtask ci` is green
- [x] 5.4 `openspec validate cli-power-pause --strict` passes

## 6. Gate review fixes

- [x] 6.1 A `pause` request queued before mains power returns wins over the automatic resume: the foreground loop reads the probe, then takes the pending request, then applies the reading (verify: `pause_request_queued_before_mains_wins` and `scenario_cli_status_shows_power_pause` in `tests/power.rs`, three runs in a row)
- [x] 6.2 `scan resume --ignore-battery` against a live scan prints a stderr note that the flag has no effect; README says when the flag applies (verify: `resume_request_on_battery_keeps_the_scan_running` checks the note; README diff reviewed)
- [x] 6.3 `scenario_settings_cli_setting_off_does_not_pause` is the binary test (setting off, unplugged mid-scan: no power line, no paused snapshot, completed); the ps-service gate test is renamed `cli_policy_off_without_the_setting_or_with_the_flag` (verify: both pass; `cargo xtask spec-coverage` still maps every scenario of this change)
- [x] 6.4 The sysfs reader skips non-regular files and caps each read at 64 bytes; the override logs a warning, and another when its folder is missing; `doctor` shows the power probe (verify: `sysfs_skips_non_regular_files_and_caps_reads`, `sysfs_does_not_block_on_a_fifo` (unix), `override_is_warned_about_and_shown_by_doctor`)
- [ ] 6.5 `cargo xtask ci` is green in a clean worktree, and GitHub CI is green on all four jobs

## Workflow follow-up

- Gate review by a fresh agent, then `/opsx:archive cli-power-pause`.
