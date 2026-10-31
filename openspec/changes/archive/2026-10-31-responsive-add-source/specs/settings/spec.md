# Spec Delta

## ADDED Requirements

### Requirement: Scan new sources right away
The setting `scanning.scan_new_sources` ("Scan new sources right away", on by default) SHALL decide whether adding a source starts its scan. When it is off, an added source SHALL show "Not scanned yet", no run SHALL start, and the Sources view SHALL offer to scan just that source. The setting SHALL persist and the CLI's `doctor` SHALL report it.

#### Scenario: New source not scanned when the setting is off
- **WHEN** the setting is off and the user adds a folder in the Sources view
- **THEN** no run is active, the row shows "Not scanned yet" and the primary action scans only that source

#### Scenario: Scan new sources setting persists
- **WHEN** the setting is read from an empty configuration directory, turned off, saved and read again
- **THEN** it is on at first, off after the restart, and `doctor --json` reports it off
