# 0072. CLI scans pause on battery and resume on mains power (amends 0053)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0053 gives the app a power probe and a watcher that pauses a running scan when the machine switches to
battery while `general.pause_on_battery` is on; the dashboard then offers to resume once mains power is back.
The `settings-wiring` design left the CLI out ("an explicit, attended command"). In practice the CLI is the
unattended path: overnight rescans of a NAS, cron jobs, scripts. An unplugged laptop keeps reading files at full
speed although the user's setting says not to, and a CLI scan has no Resume button: a pause that waits for a
person would hang a script until someone notices.

## Decision
- A foreground CLI scan (`scan`, `scan start`, `scan --resume`, and `scan resume` when no scan is live) obeys
  `general.pause_on_battery` with the same probe and the same pause rule as the app. The rule (pause a running
  run on battery; a run the user resumed on battery is left running until mains power returns) lives in one
  thread-free policy in the service layer that the app's watcher thread and the CLI's foreground loop both call.
- The CLI **resumes by itself** a run it paused for battery once mains power returns. It never resumes a pause it
  did not make: a `scan pause` request from another terminal, before or during a power pause, makes the pause the
  user's. The app keeps offering instead of resuming (a person is in front of it and may have paused for another
  reason since).
- `--ignore-battery` turns the rule off for one invocation; it does not change the setting. The CLI polls the
  probe every second (the app every three); every probe is one cheap system call or a few small sysfs reads.
- `PHOTOSQUIRREL_POWER_SUPPLY_DIR` replaces the platform probe with the Linux `power_supply` reader over the
  given folder, on every platform. It exists so the behaviour is tested end to end without a battery, is
  documented in the CLI reference, and can only cause or prevent a pause; it reads nothing else and writes
  nothing.

## Alternatives considered
- **Keep the CLI exempt** - the setting is shared (ADR-0049) and its name promises scans pause; the CLI is where
  long unattended scans happen.
- **Pause and wait for `scan resume`, as the app does** - a script or cron job would block indefinitely after a
  short unplug; the user did not ask for the scan to stop, only not to drain the battery.
- **Exit with a resumable run instead of pausing** - forces every script to loop on `scan --resume`, and the
  exit status would mean neither success nor failure.
- **A hidden test-only cargo feature for the probe** - the shipped binary would then be untested; an environment
  variable keeps one binary.

## Consequences
- The app's watcher and the CLI share one rule, so a fix to it reaches both.
- CLI JSON consumers see a new NDJSON line type, `power`, and fewer snapshot lines while a run is paused; the
  envelope's `schema_version` stays 1 because both are additions a strict consumer can skip by `type`.

## Revisit when
The app gets an "auto-resume on mains" preference, or charge level or low-power mode should matter.
