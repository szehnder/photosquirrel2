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
Every add path SHALL show each chosen folder in the source list as an "Adding…" row, and the status bar SHALL say "Adding <folder>…", in the first frame after the folder picker returns. The add control SHALL be unavailable while the picker is open or an add is pending. The row SHALL then become the source's row or show its error inline.

#### Scenario: Pending row shown at once
- **WHEN** the user picks a folder in the Sources view's picker and adding it takes 300 ms
- **THEN** the next frame shows its "Adding…" row, the status bar says "Adding" with its name and the add control is unavailable, and once the add returns the row is the source's row

### Requirement: Source added during a run is queued
A source added while a run is active SHALL be queued once, its row saying "Queued, scans next" and the status bar naming how many more sources follow. When the run completes, a run of only the queued sources SHALL start by itself. A cancelled or paused run SHALL keep the queue visible and start nothing until the user acts. An add whose start alone failed MUST NOT be reported as not added.

#### Scenario: Queued add starts after the active run
- **WHEN** a folder is added while a run of another source is active, and that run completes
- **THEN** the row says "Queued, scans next" and the status bar "then 1 more source" during the run, and afterwards a run of only the new source starts and the finished source is not rescanned

#### Scenario: Cancelled run leaves the queue visible
- **WHEN** a folder is added while a run is active, and the run is cancelled
- **THEN** no run starts, the row still says "Queued, scans next", and the primary action scans the queued source

### Requirement: Never scanned sources scanned first
Whenever an online source has never finished a scan, the Sources view's primary action SHALL scan only those sources and its label SHALL name them or count them as new; rescanning other sources SHALL stay a secondary action whose label says how many of its sources are new.

#### Scenario: Primary action scans only never scanned sources
- **WHEN** the Sources view lists one scanned source and one source never scanned
- **THEN** the primary action reads "Scan" with the new source's name, clicking it starts a run of only that source, and the secondary action reads "Scan 2 sources (1 new)"
