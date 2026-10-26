# completion-report Specification

## Purpose
Answers "is this drive fully accounted for?" by deriving, for every catalogued file, whether it is safely in the
library, covered by a better copy, deliberately excluded, still pending, unreadable or missing.

## Requirements

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

### Requirement: Ledger checks the library
Exported and covered dispositions SHALL depend on the library copy still existing with its recorded size; a verify option SHALL re-hash library copies, and a missing or mismatching copy SHALL turn the affected files back to pending.

#### Scenario: Deleted library copy becomes pending
- **WHEN** an exported file is deleted from the library and the report is refreshed
- **THEN** the keeper and its group members are pending

#### Scenario: Verify detects altered copy
- **WHEN** a library copy's bytes are altered and the report runs with verify
- **THEN** that keeper is pending and the report lists the mismatch

### Requirement: Report per source and folder
The completion report SHALL show, per source and per folder, the count and bytes of each disposition, and excluded files broken down by reason.

#### Scenario: Report per folder
- **WHEN** the report for a corpus drive is opened
- **THEN** each folder row shows counts and bytes per disposition that sum to the folder's totals

#### Scenario: Excluded reasons summarized
- **WHEN** the report for the small corpus is opened
- **THEN** excluded files are broken down by reason and the breakdown sums to the excluded count

### Requirement: Fully accounted badge
A source or folder with no pending and no unreadable files SHALL show "100% accounted for"; otherwise it SHALL show the pending and unreadable counts.

#### Scenario: Fully accounted badge
- **WHEN** every keeper of a corpus drive has been exported
- **THEN** that drive shows "100% accounted for"

#### Scenario: Pending blocks badge
- **WHEN** one keeper of that drive is not exported
- **THEN** the drive shows 1 pending instead of the badge

### Requirement: Drill down
Selecting a disposition count SHALL list the files behind it in the gallery with their reasons.

#### Scenario: Drill into pending files
- **WHEN** the user clicks the pending count of a folder
- **THEN** the gallery shows exactly the pending files of that folder

### Requirement: Disposition everywhere
The inspector SHALL show each file's disposition and its library path when exported or covered, and the gallery SHALL filter by disposition.

#### Scenario: Inspector shows disposition
- **WHEN** a covered duplicate is selected after export
- **THEN** the inspector shows covered and the library path of its keeper's copy

### Requirement: CLI report
The CLI report command SHALL print the same per-source and per-folder figures as the application, with `--json`, and exit 0 whether or not the source is fully accounted for.

#### Scenario: CLI report matches UI
- **WHEN** the CLI report runs with `--json` for a corpus drive
- **THEN** its counts and bytes per disposition equal those shown in the Reports view

### Requirement: Groups covered by an earlier copy
When no file of a group's keeper set has a valid library copy but another member of the group has one, the members with that copy SHALL be exported, the members ranked below the best-ranked of them SHALL be covered by it, and the keeper set and every member ranked above it SHALL be superseded: not accounted for and never eligible for cleanup. A keeper set with only some of its files in the library SHALL leave the group pending.

#### Scenario: Better copy not in the library is not accounted for
- **WHEN** a source's only group gets a new keeper after its JPEG was exported, and the report is refreshed
- **THEN** the JPEG is exported, a member ranked below it is covered with the JPEG's library path, the new keeper is superseded with the JPEG's library path, the source's badge reads "1 better copy not in the library yet" and is not fully accounted for, and after `export run --upgrade` copies the keeper it is exported and every other member covered

### Requirement: Better copies not in the library are reported
Reports (CLI text and `--json`, the Reports view and the inspector) SHALL show superseded files as their own line, "better copy not in the library yet", with a pointer to `export run --upgrade`; once the better copy is exported the group follows the usual rules.

#### Scenario: Reports name better copies not in the library
- **WHEN** a HEIC original decoded at last becomes the keeper of a group whose resized JPEG from another source is in the library, and `report` runs in text and with `--json`
- **THEN** the HEIC's source lists "1 better copy not in the library yet" with a pointer to `export run --upgrade`, its `counts.files.superseded` is 1 and `fully_accounted` is false, and the inspector's reason for the HEIC does not call the JPEG its best copy

#### Scenario: Partly exported keeper set stays pending
- **WHEN** the JPEG of a RAW+JPEG keeper set is in the library and the RAW is not
- **THEN** every member of the group is pending until the RAW is exported
