# scan-progress Specification

## Purpose
Shows the user, live and in detail, what a running scan is doing on every stage and every drive, and gives them
full control to start, pause, cancel and resume it.

## Requirements

### Requirement: Start a scan
The dashboard SHALL let the user choose one or more online sources and start a scan, and SHALL disable starting while a run is active.

#### Scenario: Start scan from dashboard
- **WHEN** the user selects two sources and clicks Start
- **THEN** a run starts for those two sources and the Start control becomes unavailable

### Requirement: Rescan diff shown first
For a rescan the dashboard SHALL show new, changed, unchanged and missing counts per source as soon as the walk completes and before analysis results arrive.

#### Scenario: Rescan diff shown
- **WHEN** a source with 3 added and 1 deleted files is rescanned
- **THEN** the dashboard shows new 3 and missing 1 for that source before any analysis count increases

### Requirement: Stage and source detail
The dashboard SHALL show counts and progress bars per stage (discovered, probed, analyzed, thumbed, skipped, failed) and a row per source with status, reader count, current folder of each reader, MB/s and files/s.

#### Scenario: Stage counts shown
- **WHEN** a snapshot with 1,000 discovered, 600 probed, 400 analyzed, 300 thumbed, 150 skipped and 5 failed is rendered
- **THEN** each stage shows exactly that count

#### Scenario: Per-source rows shown
- **WHEN** a run over two sources is active
- **THEN** each source has a row with its reader count, current folders and throughput

### Requirement: Throughput and ETA
The dashboard SHALL show overall files/s and MB/s averaged over the last 10 s, and an ETA from remaining bytes and smoothed throughput, showing "estimating" until 10 s of samples exist.

#### Scenario: ETA after warm-up
- **WHEN** a run has produced 10 s of throughput samples at a steady 50 MB/s with 3,000 MB remaining
- **THEN** the ETA shows 1 minute within 10%

#### Scenario: ETA estimating at start
- **WHEN** a run has been active for 3 s
- **THEN** the ETA shows "estimating"

### Requirement: Recent finds and errors
The dashboard SHALL show thumbnails of the most recent 64 photos found and the most recent 32 errors with path and reason; selecting a photo SHALL reveal it in the Library and the full error list SHALL be reachable.

#### Scenario: Recent finds show thumbnails
- **WHEN** a run has found 100 photos
- **THEN** the strip shows the 64 most recent with their thumbnails

#### Scenario: Errors listed with reason
- **WHEN** a run encounters an unreadable file
- **THEN** the errors list shows its path and the permission error

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

### Requirement: Run summary
When a run ends the dashboard SHALL show its duration, files processed, photos and videos found, duplicate groups formed, skipped and failed counts, and any incomplete sources.

#### Scenario: Run summary on completion
- **WHEN** a scan of the small corpus completes
- **THEN** the summary shows the duration and counts equal to the catalog's totals for the run

### Requirement: Status bar progress
While a run is active and the dashboard is not visible, the status bar SHALL show overall progress and ETA, and clicking it SHALL open the dashboard.

#### Scenario: Status bar shows progress
- **WHEN** a run is active and the Library view is shown
- **THEN** the status bar shows the run's progress and clicking it opens the dashboard
