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
When a run ends the dashboard SHALL show its duration, files processed, photos and videos found, duplicate groups formed, skipped, failed and damaged counts, and any incomplete sources.

#### Scenario: Run summary on completion
- **WHEN** a scan of the small corpus completes
- **THEN** the summary shows the duration and counts equal to the catalog's totals for the run

### Requirement: Status bar progress
While a run is active and the dashboard is not visible, the status bar SHALL show overall progress and ETA, and clicking it SHALL open the dashboard. This SHALL hold for a run started from any control, including adding a source and the queue, and while the run walks folders the status bar SHALL say so.

#### Scenario: Status bar shows progress
- **WHEN** a run is active and the Library view is shown
- **THEN** the status bar shows the run's progress and clicking it opens the dashboard

#### Scenario: Status bar follows a scan started by Add Source
- **WHEN** the Library view is shown and the user adds a folder with the Add Source command
- **THEN** the status bar shows "Walking folders" or "Scanning" for the run that add started, and the shell samples its activity for the Library

### Requirement: Undecodable photos noticed in the run summary
When a run ends with photos waiting for a missing decoder, the run summary SHALL show a one-line notice per format with the count and a "How to enable HEIC previews" disclosure giving the reason and fix for this system. When the decoder is available and photos still wait, it SHALL offer a rescan of their sources. Counts SHALL come from the run's summary, not per-file events.

#### Scenario: Notice shown for HEIC without a decoder
- **WHEN** a run over a source holding 3 HEIC photos completes with no HEIC decoder
- **THEN** the summary shows "3 HEIC photos have no preview" and expanding "How to enable HEIC previews" shows the reason and the fix steps from the decoder report

#### Scenario: Rescan offered once the decoder is available
- **WHEN** the dashboard opens with a HEIC decoder available and 3 photos of one source still waiting
- **THEN** it offers a rescan, and choosing it starts a run of that source only

### Requirement: Decoder failures reported whatever the log filter
A file that a decoder could not read SHALL be reported by the run (its errors list or its placeholder reason, and the summary's counts) from PhotoSquirrel's own result for that file, never only through a log line, so hiding decoder warnings MUST NOT hide a failure.

#### Scenario: Damaged RAW file still reported under the default filter
- **WHEN** a folder with a truncated CR2 fixture is scanned with the default log filter
- **THEN** the run lists the file among its errors, its placeholder reason is "could not be read", and the summary counts 1 error and 1 damaged file, although no rawler warning is printed

### Requirement: Damaged files noticed in the run summary
The run summary SHALL count the present files of the run's sources catalogued as could not be read, from one catalog aggregate at the end of the run, so an unchanged rescan still shows them. When the count is above 0 the dashboard SHALL show a warning notice to keep the original until they are checked, with a way to show those files.

#### Scenario: Damaged files counted again on an unchanged rescan
- **WHEN** a source holding one cut CR2 is scanned and then rescanned unchanged
- **THEN** the first summary counts 1 error and 1 damaged file, and the second counts 0 errors and still 1 damaged file

#### Scenario: Dashboard warns about damaged files
- **WHEN** the dashboard shows a run whose summary counts 2 damaged files
- **THEN** it shows a damaged count of 2 and a warning notice, and choosing Show opens the Library filtered to those 2 files

### Requirement: Adding a source shows at once
In the first frame after the folder picker returns, every add path SHALL show "Adding <folder>…" in the status bar, and each chosen folder as an "Adding…" row in the source list where it is shown. Every add control, onboarding's included, SHALL be unavailable while the picker is open or an add is pending. The row SHALL then become the source's row or show its error inline.

#### Scenario: Pending row shown at once
- **WHEN** the user picks a folder in the Sources view's picker and adding it takes 300 ms
- **THEN** the next frame shows its "Adding…" row, the status bar says "Adding" with its name and the add control is unavailable, and once the add returns the row is the source's row

#### Scenario: Onboarding add unavailable while pending
- **WHEN** the user adds a folder from the onboarding view and adding it takes 300 ms
- **THEN** while it is pending the onboarding button reads "Adding…", is unavailable and a second click opens no picker

### Requirement: Source added during a run is queued
A source added while a run is active SHALL be queued once, its row saying "Queued, scans next" and the status bar naming how many more sources follow. When the run completes, a run of only the queued sources SHALL start by itself. A cancelled or paused run SHALL keep the queue and start nothing until the user acts. A queued source that is removed or goes offline SHALL leave the queue. An add whose start alone failed MUST NOT be reported as not added, and SHALL say why it was not scanned.

#### Scenario: Queued add starts after the active run
- **WHEN** a folder is added while a run of another source is active, and that run completes
- **THEN** the row says "Queued, scans next" and the status bar "then 1 more source" during the run, and afterwards a run of only the new source starts and the finished source is not rescanned

#### Scenario: Cancelled run leaves the queue visible
- **WHEN** a folder is added while a run is active, and the run is cancelled
- **THEN** no run starts, the row still says "Queued, scans next", and the primary action scans the queued source

#### Scenario: Removed queued source leaves the queue
- **WHEN** a folder is added while a run is active, and the queued source is removed from the catalog before the run ends
- **THEN** the status bar no longer says "then 1 more source", and no run of it starts when the run completes

#### Scenario: Offline queued source is dropped
- **WHEN** a folder is added while a run is active, goes offline, the run completes, and the folder comes back
- **THEN** no run of it starts, it is no longer queued, and its row says "Not scanned yet"

#### Scenario: Start refused by another operation says why
- **WHEN** a folder is added and its scan's start is refused because an export is running
- **THEN** the folder is added, its row says "Not scanned yet", and the dashboard says it was not scanned because an export is running

### Requirement: Never scanned sources scanned first
Whenever an online source has never finished a scan, the Sources view's primary action SHALL scan only those sources and its label SHALL name them or count them as new; scanning the selection SHALL be a secondary action whose label says how many of its sources are new, hidden when it would scan the same sources. A scan counts only if the source stayed online through it. Such rows SHALL show their state without progress or counts.

#### Scenario: Primary action scans only never scanned sources
- **WHEN** the Sources view lists one scanned source and one source never scanned
- **THEN** the primary action reads "Scan" with the new source's name, clicking it starts a run of only that source, the secondary action reads "Scan 2 sources (1 new)", and the new source's row shows "Not scanned yet" without a progress bar

#### Scenario: Source offline during a completed run stays never scanned
- **WHEN** a run of all sources completes while one of them is offline, and that source comes back
- **THEN** it reads "Not scanned yet" and the primary action scans only it
