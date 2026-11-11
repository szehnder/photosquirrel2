# Spec Delta

## ADDED Requirements

### Requirement: Telemetry settings
Settings SHALL store the health-report choice (undecided, on or off) and the crash-report choice (off, ask or always). A new settings file SHALL have them undecided and off. Upgrading a settings file from a version without them SHALL leave them undecided and off and change no other value. An unknown value SHALL be reported by validation and behave as off. Report queues and the sent log SHALL be stored outside the settings file.

#### Scenario: Defaults are undecided and off
- **WHEN** settings are read from an empty configuration directory
- **THEN** health reports are undecided and crash reports are off

#### Scenario: Older settings file upgraded without opting in
- **WHEN** a version-4 settings file with update mode automatic is loaded
- **THEN** health reports are undecided, crash reports are off, every other value is unchanged, and the saved file is version 5

#### Scenario: Unknown telemetry value behaves as off
- **WHEN** the settings file sets health reports to `yes`
- **THEN** validation reports the health-report key, and nothing is recorded or sent

#### Scenario: Choice persists across restart
- **WHEN** the user turns health reports on and restarts the application
- **THEN** health reports are on and the card is not shown

### Requirement: Privacy group in the settings window
The General section of the settings window SHALL include a Privacy group, separate from the Updates group. It SHALL show a "Share anonymous health reports" switch, a one-line summary of what is sent, when the last report was sent or "Nothing has been sent", "See exactly what is sent", and a separate crash-report choice. The settings search SHALL find it under "privacy", "telemetry" and "reports".

#### Scenario: Privacy group shows the state
- **WHEN** the settings window opens with health reports off and no report ever sent
- **THEN** the Privacy group shows the switch off, "Nothing has been sent", "See exactly what is sent" and crash reports "Off"

#### Scenario: Search finds the privacy settings
- **WHEN** the user types "telemetry" in the settings search
- **THEN** the health-report switch and the crash-report choice are shown
