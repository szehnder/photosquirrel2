# Spec Delta

## MODIFIED Requirements

### Requirement: Derived dispositions
Every catalogued file SHALL have exactly one disposition derived from the catalog and the export ledger, never set by hand: exported (this file was copied out), covered (its group's keeper, or an earlier member of its group, was exported), excluded (junk, graphic, cache without group, user-excluded, or a copy in a group whose keeper the user excluded, with reason), pending (a keeper or candidate not yet exported), unreadable (failed after retries) or missing (gone from the source).

#### Scenario: Dispositions derived after export
- **WHEN** the small corpus is scanned and fully exported
- **THEN** every keeper is exported, every other group member is covered, every expected junk file is excluded and none is pending

#### Scenario: Covered requires exported keeper
- **WHEN** a group's keeper has not been exported and no other member of the group has a library copy
- **THEN** every member of that group is pending, not covered

#### Scenario: Excluded keeper excludes its group
- **WHEN** the user excludes the keeper of a duplicate group and the rest is exported
- **THEN** every copy in that group is excluded by the user and none is pending

## ADDED Requirements

### Requirement: Groups covered by an earlier copy
When no file of a group's keeper set has a valid library copy but another member of the group has one, the members with that copy SHALL be exported and every other member, the keeper included, SHALL be covered by it with its library path, so the group counts as accounted for. A keeper set with only some of its files in the library SHALL leave the group pending.

#### Scenario: Group covered by an earlier copy is accounted for
- **WHEN** a source's only group gets a new keeper after its JPEG was exported, and the report is refreshed
- **THEN** the JPEG is exported, the new keeper and every other member are covered with the JPEG's library path, none is pending and the source can show the fully accounted badge

#### Scenario: Partly exported keeper set stays pending
- **WHEN** the JPEG of a RAW+JPEG keeper set is in the library and the RAW is not
- **THEN** every member of the group is pending until the RAW is exported
