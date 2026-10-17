# Spec Delta

## Purpose

Provides the desktop application's main window, navigation, start-up behaviour and keyboard model that every
view of PhotoSquirrel lives in.

## ADDED Requirements

### Requirement: Main window layout
The main window SHALL show a navigator (Sources, Library, Duplicates, Excluded, AI Review, Export, Reports), a central view, a collapsible inspector and a status bar showing catalog totals and background activity.

#### Scenario: Window shows main areas
- **WHEN** the application opens with a scanned small-corpus catalog
- **THEN** the navigator lists all seven entries, the Library view is active and the status bar shows the photo count

### Requirement: Fast start
The application SHALL draw its first frame within 400 ms of a cold start and SHALL stay at or below 250 MB resident memory when idle with a 1,000,000-item catalog.

#### Scenario: Cold start within 400 ms
- **WHEN** the release application is started with its startup measurement flag
- **THEN** the reported time to first frame, recorded as a budget measurement, is at most 400 ms

#### Scenario: Idle memory within 250 MB
- **WHEN** the release application is started with its idle-memory measurement flag on a 1,000,000-item catalog
- **THEN** the reported resident memory, recorded as a budget measurement, is at most 250 MB

### Requirement: First-launch onboarding
With an empty catalog the application SHALL show an onboarding view that explains that sources are only read, never changed, and offers "Add a source".

#### Scenario: First launch shows onboarding
- **WHEN** the application starts with an empty data directory
- **THEN** the central view shows the onboarding message and an "Add a source" action

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
