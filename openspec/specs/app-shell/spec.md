# app-shell Specification

## Purpose
Provides the desktop application's main window, navigation, start-up behaviour and keyboard model that every
view of PhotoSquirrel lives in.

## Requirements

### Requirement: Main window layout
The main window SHALL show a navigator (Sources, Library, Duplicates, Excluded, AI Review, Export, Reports), a central view, a collapsible inspector and a status bar showing catalog totals and background activity.

#### Scenario: Window shows main areas
- **WHEN** the application opens with a scanned small-corpus catalog
- **THEN** the navigator lists all seven entries, the Library view is active and the status bar shows the photo count

### Requirement: First-launch onboarding
With an empty catalog the application SHALL show an onboarding view that explains that sources are only read, never changed, and offers "Add a source". When a source added from onboarding is not scanned at once, the application SHALL show the Sources view with that source's scan as the primary action.

#### Scenario: First launch shows onboarding
- **WHEN** the application starts with an empty data directory
- **THEN** the central view shows the onboarding message and an "Add a source" action

#### Scenario: Onboarding add without scanning leads to Sources
- **WHEN** "Scan new sources right away" is off and the user adds a folder from the onboarding view
- **THEN** the Sources view is shown with the new source's row reading "Not scanned yet" and no run is active

### Requirement: Interrupted run offered for resume
When the catalog has an interrupted scan run at start-up, the application SHALL show a banner naming the run with a one-click Resume.

#### Scenario: Interrupted run offers resume
- **WHEN** the application starts on a catalog whose last run was killed mid-scan
- **THEN** a banner shows the interrupted run and clicking Resume starts it

### Requirement: Layout persistence
Window size, position, panel sizes, collapsed panels and the active view SHALL be restored on the next start.

#### Scenario: Window layout restored
- **WHEN** the user resizes the inspector, switches to Duplicates and restarts the application
- **THEN** the inspector width and the Duplicates view are restored

### Requirement: Keyboard model
The application SHALL provide keyboard shortcuts for switching views (G grid, E loupe, D duplicates), toggling the inspector (I), opening settings (platform settings shortcut) and a command palette listing every command.

#### Scenario: Shortcuts switch views
- **WHEN** the user presses D and then G
- **THEN** the Duplicates view and then the Library grid become active

#### Scenario: Palette finds commands
- **WHEN** the user opens the command palette and types "rescan"
- **THEN** the palette lists the rescan command with its shortcut

### Requirement: Responsive interface
Catalog and engine work SHALL run off the UI thread; the UI MUST keep drawing frames while a slow query or operation is in progress.

#### Scenario: Slow query keeps UI responsive
- **WHEN** a catalog query is artificially delayed by 2 s while the user opens a view
- **THEN** the view shows a loading state and frames keep being drawn at least every 16.7 ms

### Requirement: Quiet start
The application MUST NOT open any network connection at start-up or while idle.

#### Scenario: No network at startup
- **WHEN** the application starts and idles for 10 s with outbound connections recorded
- **THEN** no network connection was attempted

### Requirement: Start-up time and idle memory
The application SHALL draw its first frame within 400 ms of a cold start and SHALL stay at or below 320 MB resident memory when idle with a 1,000,000-item catalog.

#### Scenario: Cold start within 400 ms
- **WHEN** the release application is started with its startup measurement flag
- **THEN** the reported time to first frame, recorded as a budget measurement, is at most 400 ms

#### Scenario: Idle memory within 320 MB
- **WHEN** the release application is started with its idle-memory measurement flag on a 1,000,000-item catalog
- **THEN** the reported resident memory, recorded as a budget measurement, is at most 320 MB

### Requirement: Duplicates count shows groups left to review
The navigator's Duplicates item SHALL show the number of duplicate groups not yet reviewed, and its tooltip SHALL give both that number and the group total. When groups exist and none is left, the count SHALL be replaced by a check mark. The count SHALL change when a group is marked reviewed or unreviewed, and after a regroup, without the user reopening a view.

#### Scenario: Count goes down as groups are reviewed
- **WHEN** the user marks 3 groups of the scanned small corpus reviewed
- **THEN** the navigator's Duplicates count is the group total minus 3, and its tooltip reads that number "left to review of" the group total "groups"

#### Scenario: Check mark when every group is reviewed
- **WHEN** the user marks every group reviewed
- **THEN** the Duplicates item shows a check mark instead of a count and stays in the navigator

#### Scenario: Count rises again when a reviewed group changes
- **WHEN** every group is reviewed and a rescan adds another copy of a reviewed group's keeper
- **THEN** the Duplicates count shows 1 again, for the changed group
