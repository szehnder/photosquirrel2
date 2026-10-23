# cleanup Specification

## Purpose
Lets the user reclaim space on backup drives by moving files that are provably safe in the library into a
quarantine folder on the same drive, re-verified at the moment of the move and fully reversible, without ever
deleting anything.

## Requirements

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

### Requirement: Moves never replace a path
Moves and undo MUST use an operation that fails rather than replace an existing target; where the file system offers none, the move MUST be refused, never done by a plain rename.

#### Scenario: Target appearing mid-move not replaced
- **WHEN** a file appears at the target path right before a move on a file system without a no-replace rename
- **THEN** that file is unchanged, the moved file stays at its source path and the move is reported as occupied

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

#### Scenario: Lost catalog commit recovered
- **WHEN** the catalog lost the record of a move (it says planned or skipped) while the file is in quarantine and the JSON journal records the move
- **THEN** reconciliation finds the file by its content hash and marks it quarantined, and undo restores it

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
The CLI SHALL plan, run (with `--yes`), list, show, select (narrow a plan by folder and file), discard and undo cleanup runs with `--json` output.

#### Scenario: CLI undo round trip
- **WHEN** the CLI plans and runs a cleanup of a fully exported corpus drive and then undoes it
- **THEN** the drive's tree snapshot equals the one taken before the cleanup

#### Scenario: CLI select and discard
- **WHEN** the CLI plans a cleanup, narrows it with `cleanup select --folder`, shows it, and discards it, all with `--json`
- **THEN** `show` reports only that folder's files as planned and the rest as unselected, `discard` exits 0 and reports the run discarded, a second `discard` and a `run --yes` of it exit with status 1, and the drive's tree is unchanged

### Requirement: Quarantined files stay accounted for only while their library copy is valid
A quarantined file SHALL count as accounted for only while the library copy that makes it safe exists and matches, whatever its exclusion state; otherwise it SHALL be reported as pending, with its quarantine path and the reason, and export SHALL copy it from the quarantine folder (read-only) without moving it.

#### Scenario: Quarantined file without library copy not accounted
- **WHEN** the library copy covering a quarantined file is deleted and the completion report is made, with or without verification
- **THEN** the drive is not reported 100% accounted for, and the file is pending with its quarantine path and a reason that says its only copy is in the quarantine folder

#### Scenario: Excluded quarantined file without library copy not accounted
- **WHEN** a quarantined file becomes excluded (by the user or a reclassification) and the library copy covering it is then deleted
- **THEN** the file is pending with its quarantine path, not excluded, and the drive is not reported 100% accounted for

#### Scenario: Export reads quarantined keeper
- **WHEN** an export runs after the library copy of a quarantined keeper was deleted
- **THEN** the keeper is copied into the library again from its quarantine path, the quarantined file is unchanged and still in quarantine, and the drive is accounted for again

### Requirement: Runs stay bound to their source
A cleanup run SHALL act only on the source it was planned on (by identity, not only by id) or, when that source is gone, on the root stored with the run; a source with files in quarantine MUST NOT be removed or folded into another source.

#### Scenario: Source with quarantined files not removed
- **WHEN** the user removes a source, or adds a folder containing it, while it has files in quarantine
- **THEN** it is refused with a message to undo first; after a full undo the source can be removed and a folder added later never takes over its runs

#### Scenario: Run of removed source refused
- **WHEN** a planned run's source is removed and the run is started
- **THEN** it is refused and nothing on disk changes

### Requirement: One operation at a time
Scans, exports, cleanup plans, selections, discards, runs, undo and reconciliation SHALL exclude each other; reconciliation MUST NOT touch a run that is live.

#### Scenario: Operations exclude each other
- **WHEN** a scan, an export, a source removal, a cleanup plan and a reconciliation are attempted while a cleanup run is moving files
- **THEN** the first four are refused naming the running cleanup, reconciliation skips, and the live run's in-flight move is untouched

#### Scenario: Selection and discard wait for a live run
- **WHEN** a selection and a discard of a planned run are attempted while another cleanup run is moving files
- **THEN** both are refused naming the running cleanup and the planned run is unchanged

### Requirement: Narrowing a plan
The user SHALL be able to narrow a stored plan that has not finished to chosen folders and files before confirming it; files left out SHALL stay in the plan as unselected, MUST NOT be verified, journaled or moved by the run, and SHALL be selectable again until the run moves them. A selection that leaves no file planned MUST be refused.

#### Scenario: Plan narrowed to folders
- **WHEN** a plan of a fully exported corpus drive is narrowed to two of its folders and the run is confirmed
- **THEN** only files under those two folders are moved, every other planned file is unchanged at its original path, and the run lists those files as unselected, not skipped

#### Scenario: Plan narrowed to files
- **WHEN** a plan is narrowed to its whole tree except three files of one folder and the run is confirmed
- **THEN** those three files are unchanged at their original paths and absent from both journals, and every other file of the plan is quarantined

#### Scenario: Narrowing is reversible before the run
- **WHEN** a plan is narrowed to one folder and then selected whole again
- **THEN** the plan lists every file it listed when it was made, with the same quarantine paths, and the drive's tree is unchanged

#### Scenario: Empty selection refused
- **WHEN** a selection that picks no planned file is applied to a plan
- **THEN** it is refused with a message to discard the plan instead, and the plan is unchanged

### Requirement: Selection in the app
The Cleanup panel SHALL show a plan's folders as a tree the user can tick, expand to files and untick before confirming, SHALL show the narrowed file count and bytes in the confirmation, and SHALL let the user undo a run whole or by chosen folders and files.

#### Scenario: App narrows a plan before confirming
- **WHEN** the user unticks one folder of a plan in the Cleanup panel and confirms
- **THEN** the confirmation names the narrowed count and bytes, and after the run that folder's files are still at their original paths while the rest are quarantined

#### Scenario: App undoes one folder
- **WHEN** the user undoes only one folder of a completed run from the Cleanup panel
- **THEN** only that folder's files are back at their original paths and the history shows the rest still quarantined

### Requirement: Discarding a planned run
The user SHALL be able to discard a run that has not moved any file, from the CLI and the app; discarding MUST NOT create, rename or delete any file or folder, SHALL keep the run in the history as discarded, and MUST be refused for a run that has moved a file, even one since restored.

#### Scenario: Planned run discarded
- **WHEN** a planned run is discarded
- **THEN** the drive's tree and quarantine folder are unchanged, the run is listed as discarded with no planned files, and running, selecting or undoing it is refused

#### Scenario: Discard refused after a move
- **WHEN** discard is asked for a run that quarantined one file, and for a run whose files were all restored by undo
- **THEN** both are refused naming the moved files, and nothing changes in the catalog or on disk

#### Scenario: Discard checks the journal first
- **WHEN** the catalog says a run is only planned but its quarantine folder holds a journal recording a move of one of its files, and discard is asked
- **THEN** reconciliation first finds that file quarantined, the discard is refused, and undo can restore the file

#### Scenario: Discarded run on an offline drive
- **WHEN** a planned run of a disconnected drive is discarded and the drive is connected again
- **THEN** the discard succeeds without touching the drive, and the run stays discarded after reconciliation

#### Scenario: App discards a plan
- **WHEN** the user discards the plan shown in the Cleanup panel and confirms
- **THEN** the review closes, the history shows the run as discarded without Review or Undo, and the drive's tree is unchanged
