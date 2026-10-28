# Spec Delta

## ADDED Requirements

### Requirement: Scan summary counts damaged files
When a scan ends with damaged files on its sources (catalogued as could not be read), the scan command SHALL print, after the counts, a line with their number, saying they are copied as is and to keep the original until they are checked, and pointing to `report`; with `--json` the summary SHALL carry the count as `damaged`.

#### Scenario: Summary names a damaged RAW
- **WHEN** a folder with one cut CR2 and two intact JPEGs is scanned
- **THEN** the text summary says 1 damaged file and points to `report`, and the `--json` summary has `damaged` 1 and `errors` 1

#### Scenario: No damaged line for intact files
- **WHEN** the small corpus is scanned
- **THEN** the text summary has no damaged line and the `--json` summary has `damaged` 0
