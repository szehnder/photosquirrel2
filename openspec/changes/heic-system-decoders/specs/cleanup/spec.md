# Spec Delta

## MODIFIED Requirements

### Requirement: Only safe files eligible
Only files whose disposition is exported or covered SHALL be eligible for cleanup; pending, superseded, excluded, unreadable and missing files MUST NOT be moved. A file is covered only when an equal or better copy of it is in the library: a group member better than every copy of its group in the library (superseded, ADR-0093) is never eligible, until it is exported itself.

#### Scenario: Pending files never eligible
- **WHEN** a cleanup plan is made for a corpus drive with one unexported keeper
- **THEN** that keeper and its group members are not in the plan

#### Scenario: Excluded files not eligible
- **WHEN** a cleanup plan is made for a drive containing junk and graphics
- **THEN** none of the excluded files are in the plan

#### Scenario: Cleanup never moves a better copy that is not in the library
- **WHEN** a resized JPEG from one source was exported, the 3000x2000 HEIC original on a second source is then scanned with a HEIC decoder and becomes the group's keeper, and a cleanup plan is made for the second source
- **THEN** the HEIC is not in the plan, and after `export run --upgrade` copies it into the library a new plan may include it and the JPEG
