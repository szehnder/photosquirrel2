## MODIFIED Requirements

### Requirement: Only safe files eligible
Only exported or covered files SHALL be eligible for cleanup; pending, superseded, excluded, not picked, unreadable and missing files MUST NOT be moved. A group member better than every copy of its group in the library (superseded, ADR-0093) is never eligible until exported itself, nor is a member whose automatic rank is above a keeper the user chose (ADR-0096); plans list the latter as kept.

#### Scenario: Pending files never eligible
- **WHEN** a cleanup plan is made for a corpus drive with one unexported keeper
- **THEN** that keeper and its group members are not in the plan

#### Scenario: Excluded files not eligible
- **WHEN** a cleanup plan is made for a drive containing junk and graphics
- **THEN** none of the excluded files are in the plan

#### Scenario: Shots not picked are not eligible
- **WHEN** the small corpus is exported with best of only and a cleanup plan is made for a corpus drive
- **THEN** no file that is not picked is in the plan

#### Scenario: Cleanup never moves a better copy that is not in the library
- **WHEN** a resized JPEG from one source was exported, the 3000x2000 HEIC original on a second source is then scanned with a HEIC decoder and becomes the group's keeper, and a cleanup plan is made for the second source
- **THEN** the HEIC is not in the plan, and after `export run --upgrade` copies it into the library a new plan may include it and the JPEG

#### Scenario: Cleanup keeps a copy better than the keeper you chose
- **WHEN** a resized JPEG from one source was exported, its 3000x2000 HEIC original on a second source is the group's automatic keeper, the user makes the JPEG the keeper, and a cleanup plan is made for the second source
- **THEN** the HEIC counts as accounted for but is not in the plan, which lists it as kept, "better than the keeper you chose"

#### Scenario: App plan lists kept copies
- **WHEN** the user made a lesser copy of a group its keeper before exporting and plans a cleanup of the better original's source in the Cleanup panel
- **THEN** a closed "Kept" disclosure says how many files stay and why, and opening it lists the original with "better than the keeper you chose"; the original is not planned
