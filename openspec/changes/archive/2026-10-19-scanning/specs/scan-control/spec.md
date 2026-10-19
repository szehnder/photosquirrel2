# Spec Delta

## Purpose

Lets the user start, pause, cancel and resume hours-long scans at any time, and recovers from crashes and
unplugged drives without losing or repeating finished work.

## ADDED Requirements

### Requirement: Scan runs with status
Each scan SHALL be a run over one or more sources with a persisted status of running, paused, cancelled, completed or interrupted, and only one run SHALL be active per catalog at a time.

#### Scenario: Completed run recorded
- **WHEN** a scan of the small corpus finishes
- **THEN** a run with status completed, start and end times and per-stage totals is recorded

#### Scenario: Second scan rejected while running
- **WHEN** a scan is started while another run is active
- **THEN** the second start is refused with the active run's id

### Requirement: Cooperative pause and cancel
Pause and cancel SHALL take effect within 250 ms, flush in-flight results first, and MUST NOT leave any file showing a stage whose results are incomplete.

#### Scenario: Cancel within 250 ms
- **WHEN** cancel is requested during a scan of the medium corpus
- **THEN** all reader and worker threads stop within 250 ms and the run status is cancelled

#### Scenario: Pause within 250 ms
- **WHEN** pause is requested during a scan
- **THEN** no new file is started after 250 ms and the run status is paused

#### Scenario: No partial results visible
- **WHEN** a scan is cancelled while large files are mid-read
- **THEN** every file marked analyzed has an identity, metadata and hashes, and none is marked analyzed without them

### Requirement: Resume without repeating work
Resuming a paused, cancelled or interrupted run SHALL re-walk the sources and process only files whose stage is incomplete.

#### Scenario: Resume skips finished files
- **WHEN** a scan is cancelled after 40% of files are thumbed and then resumed
- **THEN** no already thumbed file is read again and the final catalog equals that of an uninterrupted scan

### Requirement: Crash recovery
On start-up, a run left in running status SHALL be marked interrupted and offered for resume.

#### Scenario: Killed scan becomes interrupted
- **WHEN** the scanning process is killed mid-scan and the catalog is reopened
- **THEN** the run is interrupted and resuming it produces the same final catalog as an uninterrupted scan

### Requirement: Source lost mid-scan
If a source disappears during a run, its lane SHALL stop, its unfinished files SHALL keep their last completed stage, the other sources SHALL continue, and the run summary SHALL name the source as incomplete.

#### Scenario: Source vanishes mid-scan
- **WHEN** one of two corpus drives is renamed away during a scan
- **THEN** the other drive completes, the lost source is offline and listed as incomplete, and a later resume finishes it once re-linked

### Requirement: Pause and cancel latency budget
The measured latency from a pause or cancel request to all threads idle SHALL be at most 250 ms at p95 over repeated trials.

#### Scenario: Control latency within budget
- **WHEN** 20 cancel requests are issued at random moments during scans on the reference machine
- **THEN** the p95 latency is reported as a budget measurement and is at most 250 ms
