# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Cleanup from the CLI
The CLI SHALL plan, run (with `--yes`), list, show, select (narrow a plan by folder and file), discard and undo cleanup runs with `--json` output.

#### Scenario: CLI undo round trip
- **WHEN** the CLI plans and runs a cleanup of a fully exported corpus drive and then undoes it
- **THEN** the drive's tree snapshot equals the one taken before the cleanup

#### Scenario: CLI select and discard
- **WHEN** the CLI plans a cleanup, narrows it with `cleanup select --folder`, shows it, and discards it, all with `--json`
- **THEN** `show` reports only that folder's files as planned and the rest as unselected, `discard` exits 0 and reports the run discarded, a second `discard` and a `run --yes` of it exit with status 1, and the drive's tree is unchanged

### Requirement: One operation at a time
Scans, exports, cleanup plans, selections, discards, runs, undo and reconciliation SHALL exclude each other; reconciliation MUST NOT touch a run that is live.

#### Scenario: Operations exclude each other
- **WHEN** a scan, an export, a source removal, a cleanup plan and a reconciliation are attempted while a cleanup run is moving files
- **THEN** the first four are refused naming the running cleanup, reconciliation skips, and the live run's in-flight move is untouched

#### Scenario: Selection and discard wait for a live run
- **WHEN** a selection and a discard of a planned run are attempted while another cleanup run is moving files
- **THEN** both are refused naming the running cleanup and the planned run is unchanged
