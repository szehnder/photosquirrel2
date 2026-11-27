## MODIFIED Requirements

### Requirement: Derived dispositions
Every catalogued file SHALL have exactly one disposition derived from the catalog and the export ledger, never set by hand: exported (this file was copied out), covered (its group's keeper, or an earlier member of its group, was exported), excluded (by the classifier or the user, or a copy of an excluded keeper, with reason), not picked (left out by a best-of export), pending (a keeper or candidate not yet exported), unreadable (failed after retries) or missing (gone from the source).

#### Scenario: Dispositions derived after export
- **WHEN** the small corpus is scanned and fully exported
- **THEN** every keeper is exported, every other group member is covered, every expected junk file is excluded and none is pending

#### Scenario: Covered requires exported keeper
- **WHEN** a group's keeper has not been exported and no other member of the group has a library copy
- **THEN** every member of that group is pending, not covered

#### Scenario: Excluded keeper excludes its group
- **WHEN** the user excludes the keeper of a duplicate group and the rest is exported
- **THEN** every copy in that group is excluded by the user and none is pending

#### Scenario: Shots not picked after a best-of export
- **WHEN** the small corpus is exported with best of only
- **THEN** every member of a stacked group that is not the pick is not picked, none of them is pending, and the report counts them under not picked

## ADDED Requirements

### Requirement: Not picked disposition
A member of a stacked group that is not its stack's pick SHALL be not picked when the last export was best of only and no member of its group is in the library; it SHALL turn back to pending when an export of everything is planned, and to covered or exported once its group is in the library.

#### Scenario: Not picked turns pending for a full export
- **WHEN** the small corpus was exported with best of only and an export of everything is then planned
- **THEN** the shots that were not picked are pending in that plan's report
