# Spec Delta

## Purpose

Lets the user reclaim space on backup drives by moving files that are provably safe in the library into a
quarantine folder on the same drive, re-verified at the moment of the move and fully reversible, without ever
deleting anything.

## ADDED Requirements

### Requirement: Only safe files eligible
Only files whose disposition is exported or covered SHALL be eligible for cleanup; pending, excluded, unreadable and missing files MUST NOT be moved.

#### Scenario: Pending files never eligible
- **WHEN** a cleanup plan is made for a corpus drive with one unexported keeper
- **THEN** that keeper and its group members are not in the plan

#### Scenario: Excluded files not eligible
- **WHEN** a cleanup plan is made for a drive containing junk and graphics
- **THEN** none of the excluded files are in the plan

### Requirement: Plan and confirmation
Cleanup SHALL first show a plan with the files, their quarantine paths, counts and bytes per folder, and SHALL move nothing until the user confirms that plan.

#### Scenario: Plan before move
- **WHEN** a cleanup plan is made for a fully exported corpus drive
- **THEN** it lists every exported and covered file with its quarantine path and the drive's tree is unchanged

#### Scenario: No move without confirmation
- **WHEN** the CLI cleanup run is started non-interactively without `--yes`
- **THEN** nothing is moved and the command exits with status 1

### Requirement: Re-verification before each move
Immediately before moving a file, its current content hash SHALL be recomputed and compared with the catalog, and the library copy that makes it safe SHALL be confirmed to exist with a matching hash; on any mismatch the file SHALL be skipped and reported.

#### Scenario: Changed source skipped
- **WHEN** a planned file's bytes are changed after planning and the cleanup runs
- **THEN** that file is not moved and is reported as changed since scanning

#### Scenario: Missing library copy blocks move
- **WHEN** the library copy covering a planned file is deleted after planning and the cleanup runs
- **THEN** that file and the other files covered by that copy are not moved and are reported

### Requirement: Same-volume rename into quarantine
Each eligible file SHALL be moved by a rename to `<source root>/.photosquirrel-quarantine/<run id>/<relative path>` on the same volume, without copying data; a move that would cross volumes MUST be refused.

#### Scenario: Move is a same-volume rename
- **WHEN** a planned file is quarantined
- **THEN** it exists at its quarantine path with the same file identity (inode or file index) and unchanged bytes, and no longer exists at its original path

#### Scenario: Cross-volume move refused
- **WHEN** a planned file's quarantine folder would be on a different volume than the file
- **THEN** the file is not moved and the reason is reported

### Requirement: Never delete
Cleanup and undo MUST NOT delete any user file; the total number of user files in the source tree plus its quarantine folder SHALL be unchanged by any cleanup or undo.

#### Scenario: Cleanup never deletes
- **WHEN** a cleanup run and a partial undo are performed on a corpus drive
- **THEN** every original file exists either at its original path or at its quarantine path with unchanged bytes

### Requirement: Write-ahead journal
Every move SHALL be journaled in the catalog and in a JSON journal inside the run's quarantine folder before it is performed and marked done after; after a crash the journal SHALL be reconciled with the file system on next start.

#### Scenario: Journal records every move
- **WHEN** a cleanup run moves 50 files
- **THEN** both journals list 50 done moves with original and quarantine paths and content hashes

#### Scenario: Crash mid-run is recoverable
- **WHEN** the process is killed during a cleanup run and restarted
- **THEN** every journaled move is reconciled as done or not done from the file system and undo restores all moved files

### Requirement: Undo
The user SHALL be able to undo a whole cleanup run or a selection of its moves (by folder or file), restoring each file to its original path; a file whose original path is now occupied MUST NOT overwrite it and SHALL be reported.

#### Scenario: Undo restores run
- **WHEN** a cleanup run is undone
- **THEN** every moved file is back at its original path with unchanged bytes and the run is marked undone

#### Scenario: Partial undo by folder
- **WHEN** the user undoes only the moves of one folder
- **THEN** only that folder's files are restored and the run shows the rest as still quarantined

#### Scenario: Occupied path not overwritten
- **WHEN** a new file has been created at a quarantined file's original path and the move is undone
- **THEN** the new file is unchanged, the quarantined file stays in quarantine and the conflict is reported

### Requirement: Quarantine ignored by scans
Scans SHALL skip `.photosquirrel-quarantine` folders, and quarantined files SHALL be reported as quarantined (with their prior disposition) rather than missing.

#### Scenario: Quarantine ignored by scanner
- **WHEN** a drive with a cleanup run is rescanned
- **THEN** no file under the quarantine folder is catalogued as new and the moved files are reported as quarantined, not missing

### Requirement: Cleanup from the CLI
The CLI SHALL plan, run (with `--yes`), list and undo cleanup runs with `--json` output.

#### Scenario: CLI undo round trip
- **WHEN** the CLI plans and runs a cleanup of a fully exported corpus drive and then undoes it
- **THEN** the drive's tree snapshot equals the one taken before the cleanup
