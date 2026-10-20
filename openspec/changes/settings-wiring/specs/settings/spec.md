# settings (delta)

## ADDED Requirements

### Requirement: Scanning settings reach every scanner
The app and the CLI SHALL build their scanner options from the same settings: classifier thresholds, extra cache
patterns, hidden files, the preview cache size and the poster-frame tool. A change SHALL apply to the next scan
started, never to a running one.

#### Scenario: App rescan uses changed thresholds
- **WHEN** the minimum side is lowered to 100 px in the app's settings and an unchanged source is rescanned from the app
- **THEN** files between 100 and 200 px that were junk are re-classified, as they are when the CLI rescans, and no file content is read for the reclassification

#### Scenario: Hidden files setting honoured
- **WHEN** `scanning.include_hidden` is off and a source contains a hidden folder of photos
- **THEN** the scan does not list that folder, and turning the setting on and rescanning adds its photos

### Requirement: Pause on battery
When `general.pause_on_battery` is on, a running scan SHALL pause when the machine switches to battery power and
SHALL offer to resume when mains power returns.

#### Scenario: Scan pauses on battery
- **WHEN** a scan is running with the setting on and the power source changes to battery
- **THEN** the scan pauses within a few seconds and the dashboard says why
