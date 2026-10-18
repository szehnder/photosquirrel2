# Spec Delta

## Purpose

Stores the user's preferences durably and safely, shared by the desktop application and the CLI, with
local-first defaults and never any secrets on disk.

## ADDED Requirements

### Requirement: Persistent settings file
Settings SHALL be stored in a human-readable, versioned file in the per-user configuration directory, shared by the application and the CLI, and SHALL persist across restarts.

#### Scenario: Settings persist across restart
- **WHEN** the user changes the theme and the preview cache cap and restarts the application
- **THEN** both values are restored and the CLI reports the same values

### Requirement: Local-first defaults
Default settings SHALL select the local LM Studio provider at `http://localhost:1234/v1`, leave the AI queue stopped, disable hosted-provider consent, and use the ADR thresholds (200 px minimum side, 120,000 minimum pixels, 2 s minimum video, 4 GB preview cache).

#### Scenario: Defaults are local
- **WHEN** settings are read from an empty configuration directory
- **THEN** the provider is LM Studio at `http://localhost:1234/v1`, hosted consent is off and the thresholds equal the ADR defaults

### Requirement: No secrets in settings
The settings file MUST NOT contain API keys or other secrets.

#### Scenario: No secrets in settings file
- **WHEN** an API key is entered for a hosted provider and settings are saved
- **THEN** the settings file does not contain the key string

### Requirement: Validation
Settings changes SHALL be validated before saving; invalid values (for example a negative cache cap or a non-executable poster-frame tool path) SHALL be rejected with a message and MUST NOT be saved.

#### Scenario: Invalid value rejected
- **WHEN** the user enters a preview cache cap of -1 GB
- **THEN** the field shows an error and the saved settings are unchanged

### Requirement: Corrupt settings recovery
An unreadable or unparseable settings file SHALL be preserved as a backup copy, defaults SHALL be used, and the user SHALL be told once.

#### Scenario: Corrupt settings fall back
- **WHEN** the settings file contains invalid syntax at start-up
- **THEN** the application starts with defaults, a backup of the bad file exists beside it and a notice is shown

### Requirement: Classifier settings take effect
Changing classifier thresholds SHALL re-classify already probed files from stored probe data on the next rescan without reading their contents again.

#### Scenario: Threshold change reclassifies
- **WHEN** the minimum side is lowered to 100 px and an unchanged source is rescanned
- **THEN** files between 100 and 200 px that were junk are re-classified and no file content is read for the reclassification

### Requirement: Settings window
The application SHALL provide a settings window with sections (General, Appearance, Scanning, AI, Export) and a search field that filters settings by name.

#### Scenario: Search filters settings
- **WHEN** the user types "cache" in the settings search
- **THEN** only settings whose name contains "cache" are shown
