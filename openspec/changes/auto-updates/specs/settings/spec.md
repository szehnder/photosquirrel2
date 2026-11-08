# Spec Delta

## MODIFIED Requirements

### Requirement: Local-first defaults
Default settings SHALL select the local LM Studio provider at `http://localhost:1234/v1`, leave the AI queue stopped, disable hosted-provider consent, leave the update mode undecided, and use the ADR thresholds (200 px minimum side, 120,000 minimum pixels, 2 s minimum video, 4 GB preview cache).

#### Scenario: Defaults are local
- **WHEN** settings are read from an empty configuration directory
- **THEN** the provider is LM Studio at `http://localhost:1234/v1`, hosted consent is off, the update mode is undecided and the thresholds equal the ADR defaults

### Requirement: Settings window
The application SHALL provide a settings window with sections (General, Appearance, Scanning, AI, Export) and a search field that filters settings by name. General SHALL include an Updates group showing the installed version, the update mode, the installation kind and whether it can update itself, the last check and its result, "Check for updates" and, when a previous version is kept, "Go back to <version>".

#### Scenario: Search filters settings
- **WHEN** the user types "cache" in the settings search
- **THEN** only settings whose name contains "cache" are shown

#### Scenario: Updates group shows the update state
- **WHEN** the settings window opens on a per-user installation with mode automatic, a completed check and a kept previous version
- **THEN** the Updates group shows the installed version, "Automatic", the installation kind, the time and result of the last check, "Check for updates" and "Go back to <previous>"

## ADDED Requirements

### Requirement: Update settings
Settings SHALL store the update mode (undecided, automatic, notify or off), shared by the application and the CLI. Upgrading a settings file from a version without it SHALL leave it undecided. An unknown mode SHALL be rejected by validation. The state the updater keeps (last check, the newest accepted version, ready and kept versions) SHALL be stored outside the settings file.

#### Scenario: Update mode persists and is shared
- **WHEN** the user sets the update mode to notify and restarts the application
- **THEN** the mode is notify and the CLI's update status reports notify

#### Scenario: Older settings file upgraded to undecided
- **WHEN** a version-3 settings file is loaded
- **THEN** the update mode is undecided, every other value is unchanged, and the saved file is version 4

#### Scenario: Unknown update mode rejected
- **WHEN** the settings file sets the update mode to `always`
- **THEN** validation reports the update mode key and the mode used is undecided
