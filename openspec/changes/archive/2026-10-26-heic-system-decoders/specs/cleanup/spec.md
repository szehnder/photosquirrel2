# Spec Delta

## MODIFIED Requirements

### Requirement: Only safe files eligible
Only exported or covered files SHALL be eligible for cleanup; pending, superseded, excluded, unreadable and missing files MUST NOT be moved. A group member better than every copy of its group in the library (superseded, ADR-0093) is never eligible until exported itself, nor is a member whose automatic rank is above a keeper the user chose (ADR-0096); plans list the latter as kept.

#### Scenario: Pending files never eligible
- **WHEN** a cleanup plan is made for a corpus drive with one unexported keeper
- **THEN** that keeper and its group members are not in the plan

#### Scenario: Excluded files not eligible
- **WHEN** a cleanup plan is made for a drive containing junk and graphics
- **THEN** none of the excluded files are in the plan

#### Scenario: Cleanup never moves a better copy that is not in the library
- **WHEN** a resized JPEG from one source was exported, the 3000x2000 HEIC original on a second source is then scanned with a HEIC decoder and becomes the group's keeper, and a cleanup plan is made for the second source
- **THEN** the HEIC is not in the plan, and after `export run --upgrade` copies it into the library a new plan may include it and the JPEG

#### Scenario: Cleanup keeps a copy better than the keeper you chose
- **WHEN** a resized JPEG from one source was exported, its 3000x2000 HEIC original on a second source is the group's automatic keeper, the user makes the JPEG the keeper, and a cleanup plan is made for the second source
- **THEN** the HEIC counts as accounted for but is not in the plan, which lists it as kept, "better than the keeper you chose"

#### Scenario: App plan lists kept copies
- **WHEN** the user made a lesser copy of a group its keeper before exporting and plans a cleanup of the better original's source in the Cleanup panel
- **THEN** a closed "Kept" disclosure says how many files stay and why, and opening it lists the original with "better than the keeper you chose"; the original is not planned

### Requirement: Re-verification before each move
When a run starts, and whenever the catalog's constraints generation changes during it, the dispositions of its remaining planned files SHALL be derived again and each file no longer eligible or no longer made safe by its planned library copy SHALL be skipped as "no longer safe" (ADR-0096, ADR-0099). Immediately before a move, the file's hash SHALL be recomputed and compared with the catalog, and its library copy confirmed by hash; on any mismatch the file SHALL be skipped and reported.

#### Scenario: Changed source skipped
- **WHEN** a planned file's bytes are changed after planning and the cleanup runs
- **THEN** that file is not moved and is reported as changed since scanning

#### Scenario: Missing library copy blocks move
- **WHEN** the library copy covering a planned file is deleted after planning and the cleanup runs
- **THEN** that file and the other files covered by that copy are not moved and are reported

#### Scenario: Cleanup run skips a file that became unsafe after planning
- **WHEN** a stored plan names a HEIC original as covered by an exported JPEG, the plan is narrowed and widened again with `select`, the keeper override that made the JPEG the keeper is cleared, and the run starts
- **THEN** the HEIC is skipped with a "no longer safe" reason in the run report and its JSON, stays at its path, and the other planned file moves

#### Scenario: Group edit during a cleanup run is respected
- **WHEN** a cleanup run has moved its first file and a covered resize it still plans is then separated from its group, with no copy of it in the library
- **THEN** the service refused the edit with "a cleanup is running", and the same edit written to the catalog directly makes the run check its remaining files again and skip the resize as no longer safe, leaving it at its path

### Requirement: Selection in the app
The Cleanup panel SHALL show a plan's folders as a tree the user can tick, expand to files and untick before confirming, SHALL show the narrowed file count and bytes in the confirmation, SHALL list the files a run left in place with why, and SHALL let the user undo a run whole or by chosen folders and files.

#### Scenario: App narrows a plan before confirming
- **WHEN** the user unticks one folder of a plan in the Cleanup panel and confirms
- **THEN** the confirmation names the narrowed count and bytes, and after the run that folder's files are still at their original paths while the rest are quarantined

#### Scenario: App undoes one folder
- **WHEN** the user undoes only one folder of a completed run from the Cleanup panel
- **THEN** only that folder's files are back at their original paths and the history shows the rest still quarantined

#### Scenario: App run skips a file that became unsafe after planning
- **WHEN** the user plans a cleanup in the Cleanup panel, makes one planned covered copy its group's keeper before confirming, and confirms the run
- **THEN** that copy stays at its path, the run summary lists it with a "no longer safe" reason, and every other planned file is quarantined

## ADDED Requirements

### Requirement: Edits wait during a cleanup run
While a cleanup run is moving files, the service SHALL refuse keeper, group, merge, include/exclude and AI suggestion decisions with a message saying a cleanup is running, and the app SHALL not offer those edits until the run ends (ADR-0099).

#### Scenario: App refuses edits during a cleanup run
- **WHEN** a cleanup run holds the operation slot while the Duplicates view shows a group
- **THEN** the keeper, separate and merge actions are not offered, a note says a cleanup is running, the keeper shortcut changes nothing, the service refuses the same edit, and the actions return when the run ends
