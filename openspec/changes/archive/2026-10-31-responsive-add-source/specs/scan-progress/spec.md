# Spec Delta

## MODIFIED Requirements

### Requirement: Status bar progress
While a run is active and the dashboard is not visible, the status bar SHALL show overall progress and ETA, and clicking it SHALL open the dashboard. This SHALL hold for a run started from any control, including adding a source and the queue, and while the run walks folders the status bar SHALL say so.

#### Scenario: Status bar shows progress
- **WHEN** a run is active and the Library view is shown
- **THEN** the status bar shows the run's progress and clicking it opens the dashboard

#### Scenario: Status bar follows a scan started by Add Source
- **WHEN** the Library view is shown and the user adds a folder with the Add Source command
- **THEN** the status bar shows "Walking folders" or "Scanning" for the run that add started, and the shell samples its activity for the Library

## ADDED Requirements

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
