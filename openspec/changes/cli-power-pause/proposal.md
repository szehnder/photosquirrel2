# Proposal

## Why

`general.pause_on_battery` is a shared setting, but only the app honours it: the `settings-wiring` design said
"the CLI does not pause (it is an explicit, attended command)". In practice `photosquirrel-cli scan` is the
unattended path (overnight rescans of a NAS, scripts), so a laptop that is unplugged mid-scan drains its battery
reading files although the user asked for scans to pause. The CLI should obey the same setting through the same
power probe, with an explicit override, and say clearly on stderr and in `--json` output that it is paused and
why.

## What Changes

- A foreground CLI scan (`scan`, `scan start`, `scan --resume`, `scan resume` when no scan is live) watches the
  power source with the same probe and the same pause rule as the app (ADR-0053) while `general.pause_on_battery`
  is on.
- Unlike the app, which only offers to resume, the CLI resumes a run it paused for battery by itself when mains
  power returns: it has no Resume button and is often unattended (new ADR-0072, amends ADR-0053). A pause the user
  requested (`scan pause` from another terminal) is never auto-resumed.
- `--ignore-battery` on the scan commands turns the rule off for that invocation; `scan resume` from another
  terminal while on battery keeps the run going until mains power returns, as the app's Resume does.
- Output while paused for power: one stderr line when the pause starts and one when it ends (both modes, terminal
  or not); the progress line reads `paused on battery`; with `--json`, a `"type": "power"` NDJSON line per
  transition, and paused snapshots are throttled to one every 5 s. `scan status` in another terminal shows the
  reason.
- Ctrl-C while paused cancels as it does while running (exit 130, resumable run).
- A documented probe override (`PHOTOSQUIRREL_POWER_SUPPLY_DIR`, a Linux-style `power_supply` folder read on
  every platform) so the behaviour is tested end to end without a battery.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `settings`: "Pause on battery" now covers CLI scans, the CLI's automatic resume and the `--ignore-battery`
  override.
- `cli`: new requirement for how a scan paused for power is reported (stderr, `--json` power lines, throttled
  paused snapshots, status) and that an interrupt while paused still cancels.

## Impact

- ADRs relied on: 0008 (cooperative pause/cancel), 0019 (sampled progress), 0027 (CLI as verification surface),
  0041 (one owner process), 0049 (settings file ownership), 0053 (power probe). New: ADR-0072 "CLI scans pause on
  battery and resume on mains" (amends 0053).
- Code: `ps-service::power` (the watcher's pause rule extracted into a thread-free policy both the app watcher and
  the CLI call; the probe override), `photosquirrel-cli` (`cmd/scan.rs` foreground loop, `main.rs` flag,
  `control.rs` heartbeat payload), README CLI section.
- No new dependency, file format, crate boundary or performance budget.

## Out of scope

- `photosquirrel-cli demo` (a seeding helper that scans synchronously), `ai start`, `export` and `cleanup`: they do
  not pause on battery in the app either.
- Charge-level thresholds, low-power mode, or pausing the app's AI queue on battery.
- Changing the app's behaviour (it still only offers to resume).
