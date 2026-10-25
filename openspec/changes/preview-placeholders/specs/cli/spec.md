# Spec Delta

## ADDED Requirements

### Requirement: Preview status and fill in
`photosquirrel-cli previews status` SHALL report the placeholder count by format and reason, and how many can be filled in now. `previews fill [--format F]` SHALL run the Fill in previews job with progress on stderr, Ctrl-C handling like `scan`, and a summary. Both SHALL support `--json`.

#### Scenario: Status counts by reason
- **WHEN** `previews status --json` runs on a catalog with 4 HEIC files waiting for a missing decoder and 1 damaged JPEG
- **THEN** the JSON lists heic/decoder_missing 4 with fillable 0, and jpeg/unreadable 1 with fillable 0

#### Scenario: Fill one format
- **WHEN** `previews fill --format heic --json` runs after a HEIC decoder became available
- **THEN** only HEIC placeholders are re-decoded, and the summary reports filled, skipped offline and failed counts
