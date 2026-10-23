# Spec Delta

## ADDED Requirements

### Requirement: Capture precision shown
The inspector SHALL show a capture date with month precision as a month and year only and a date with day precision without a time, each with its provenance.

#### Scenario: Month-precision date shown as a month
- **WHEN** an EXIF-less photo dated by the folder `2019-07 Italy` is selected
- **THEN** the Capture section shows "July 2019" with provenance folder name and no day or time
