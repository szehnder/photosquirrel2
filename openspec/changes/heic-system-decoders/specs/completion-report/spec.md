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
When no file of a group's keeper set has a valid library copy but another member of the group has one, the members with that copy SHALL be exported, the members ranked below the best-ranked of them SHALL be covered by it with its library path, and the keeper set and every member ranked above it SHALL be superseded: better than anything of the group in the library, with that copy's path, not accounted for and never eligible for cleanup. Reports (CLI text and `--json`, the Reports view and the inspector) SHALL show superseded files as their own line, "better copy not in the library yet", with a pointer to `export run --upgrade`; once the better copy is exported the group follows the usual rules. A keeper set with only some of its files in the library SHALL leave the group pending.

#### Scenario: Better copy not in the library is not accounted for
- **WHEN** a source's only group gets a new keeper after its JPEG was exported, and the report is refreshed
- **THEN** the JPEG is exported, a member ranked below it is covered with the JPEG's library path, the new keeper is superseded with the JPEG's library path, the source's badge reads "1 better copy not in the library yet" and is not fully accounted for, and after `export run --upgrade` copies the keeper it is exported and every other member covered

#### Scenario: Reports name better copies not in the library
- **WHEN** a HEIC original decoded at last becomes the keeper of a group whose resized JPEG from another source is in the library, and `report` runs in text and with `--json`
- **THEN** the HEIC's source lists "1 better copy not in the library yet" with a pointer to `export run --upgrade`, its `counts.files.superseded` is 1 and `fully_accounted` is false, and the inspector's reason for the HEIC does not call the JPEG its best copy

#### Scenario: Partly exported keeper set stays pending
- **WHEN** the JPEG of a RAW+JPEG keeper set is in the library and the RAW is not
- **THEN** every member of the group is pending until the RAW is exported
