# Spec Delta

## MODIFIED Requirements

### Requirement: Scan with live progress
The scan command SHALL render the progress snapshot at most 10 times per second; with `--json` it SHALL print one JSON snapshot per line and a final summary line. With `--json`, lines of type `power` MAY appear between snapshots when the run pauses or resumes for power; no other line types SHALL appear.

#### Scenario: Scan streams JSON snapshots
- **WHEN** the small corpus is scanned with `--json`
- **THEN** every output line parses as JSON, lines are at least 100 ms apart, and the last line is a summary with status completed

## ADDED Requirements

### Requirement: Power pause is reported
When a foreground scan pauses or resumes for power, the CLI SHALL print one line on standard error saying so and
how to override it, SHALL mark its progress line as paused on battery and, with `--json`, SHALL print one NDJSON
line of type `power` per transition with the event, the run id and the time. While a run is paused, JSON
snapshots SHALL be printed at most once every 5 seconds.

#### Scenario: Power pause JSON lines
- **WHEN** a `--json` scan pauses for battery and later resumes on mains power
- **THEN** every line parses as JSON, a `power` line with event `paused_on_battery` and the run id precedes the paused snapshots, a `power` line with event `resumed_on_mains` follows them, and the last line is a summary with status completed

#### Scenario: Paused snapshots throttled
- **WHEN** a `--json` scan stays paused for battery for 12 seconds
- **THEN** at most four snapshot lines are printed during the pause and each has status paused

#### Scenario: Power pause on stderr
- **WHEN** a scan without `--json` and without a terminal pauses for battery
- **THEN** standard error has a line saying the scan is paused because the computer is on battery power and naming `--ignore-battery`, printed within a second of the pause and not repeated while the pause lasts

#### Scenario: Status shows power pause
- **WHEN** `scan status` is run in another terminal while a CLI scan is paused for battery
- **THEN** its live entry reports the run as paused on battery power

### Requirement: Interrupt while paused
An interrupt signal during a scan that is paused, for power or by request, SHALL cancel it exactly as for a running scan: exit status 130 and a resumable run.

#### Scenario: Ctrl-C while paused on battery
- **WHEN** an interrupt is sent to a CLI scan paused for battery
- **THEN** the CLI exits with status 130 within 1 s, the run is cancelled, and `scan --resume` completes it
