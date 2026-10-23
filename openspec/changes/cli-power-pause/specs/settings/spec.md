# Spec Delta

## MODIFIED Requirements

### Requirement: Pause on battery
When `general.pause_on_battery` is on, a running scan, whether started from the app or from a foreground CLI
scan command, SHALL pause when the machine switches to battery power. The app SHALL offer to resume when mains
power returns; the CLI SHALL resume a run it paused for battery by itself once mains power returns, and SHALL
never resume a pause the user requested. A CLI scan started with `--ignore-battery` SHALL not pause for power.

#### Scenario: Scan pauses on battery
- **WHEN** a scan is running with the setting on and the power source changes to battery
- **THEN** the scan pauses within a few seconds and the dashboard says why

#### Scenario: CLI scan pauses on battery
- **WHEN** a CLI scan is running with the setting on and the power source changes to battery
- **THEN** the run is paused within a few seconds, no further files are read while it stays on battery, and the command keeps running

#### Scenario: CLI scan resumes on mains power
- **WHEN** a CLI scan paused for battery sees mains power return
- **THEN** the same run resumes by itself and completes with status completed

#### Scenario: CLI scan started on battery pauses at once
- **WHEN** a CLI scan is started with the setting on while the machine is already on battery
- **THEN** the run is paused as soon as it starts, with `--json` the first output line is a `power` line with event `paused_on_battery`, and the run resumes and completes once mains power returns

#### Scenario: CLI ignore battery flag
- **WHEN** a CLI scan is started with `--ignore-battery` while the setting is on and the machine is on battery
- **THEN** the scan never pauses and completes, and stderr says that battery power is being ignored

#### Scenario: CLI setting off does not pause
- **WHEN** a CLI scan runs with the setting off and the power source changes to battery
- **THEN** the scan never pauses and no power line is printed

#### Scenario: CLI user pause not auto resumed
- **WHEN** a CLI scan is paused with `scan pause` from another terminal while on battery and mains power then returns
- **THEN** the run stays paused until `scan resume` is requested

#### Scenario: CLI resume on battery overrides
- **WHEN** `scan resume` is requested from another terminal for a CLI scan paused for battery while still on battery
- **THEN** the run continues and is not paused again for power until mains power has returned and been lost again
