# Spec Delta

## MODIFIED Requirements

### Requirement: Pause, resume and cancel
The dashboard SHALL offer Pause, Resume and Cancel; each SHALL be reflected in the run status within 250 ms; Cancel SHALL state that the run can be resumed later. After a cancel request the dashboard SHALL show the cancelled status as soon as the engine reports the run stopped, without waiting for its next periodic progress sample.

#### Scenario: Pause from dashboard
- **WHEN** the user clicks Pause during a run
- **THEN** the run status shows paused within 250 ms and counts stop increasing

#### Scenario: Cancel from dashboard
- **WHEN** the user clicks Cancel during a run
- **THEN** the run status shows cancelled within 250 ms and a Resume action is offered

#### Scenario: Resume from dashboard
- **WHEN** the user clicks Resume on a cancelled or interrupted run
- **THEN** the same run continues and already finished files are not processed again

#### Scenario: Cancelled shown when the run stops
- **WHEN** the user clicks Cancel and the engine stops the run 30 ms later, with the next periodic sample 100 ms away
- **THEN** the dashboard shows cancelled without the clock advancing to that sample

### Requirement: Constant UI cost
The dashboard SHALL read at most one progress snapshot per 100 ms, and its frame time MUST NOT depend on how many files per second the scan processes. Frame time SHALL be measured as the processor time the rendering thread spends on the frame where the platform reports it per thread, so time the thread spends preempted by other processes is not counted.

#### Scenario: Snapshot sampled at most 10 Hz
- **WHEN** the dashboard is open for 5 s during an active run
- **THEN** at most 51 snapshots are taken

#### Scenario: UI cost independent of rate
- **WHEN** the dashboard renders a simulated run at 100 files/s and at 20,000 files/s, alternating frame by frame
- **THEN** the frame-time p95 differs by less than 1 ms and stays at or below 8.3 ms
