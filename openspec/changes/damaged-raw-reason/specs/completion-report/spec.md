# Spec Delta

## ADDED Requirements

### Requirement: Damaged files flagged as at risk
Reports (CLI text and `--json`, the Reports view) SHALL count, per source and folder, the files catalogued as could not be read, without changing their disposition. The badge SHALL add the count ("100% accounted for, 1 damaged"), and the source SHALL say the photo may be incomplete and to keep the original until it is checked, listing the files.

#### Scenario: Exported damaged RAW is still flagged
- **WHEN** a source with one cut CR2 and two intact JPEGs is fully exported and `report` runs in text and with `--json`
- **THEN** the cut CR2 is exported, the badge reads "100% accounted for, 1 damaged", the text names its path with the keep-the-original hint, and `counts.damaged` is 1 with the path in `damaged_files`

#### Scenario: Reports view drills into damaged files
- **WHEN** the Reports view shows that source and the user chooses its damaged count
- **THEN** the gallery shows exactly the cut CR2
