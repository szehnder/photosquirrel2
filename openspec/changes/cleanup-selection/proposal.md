# Proposal

## Why

The app's Cleanup panel can only plan and move a whole source, and can only undo a whole run, so a user who
wants to free one folder of a backup drive, or to keep a few files in place, must switch to the CLI. A plan
that the user decides not to run also stays in the history as "Planned" forever, with no way to drop it. These
are the follow-ups the owner asked for after `quarantine-cleanup`.

## What Changes

- **Narrow a plan before confirming.** In the Cleanup panel the plan's folders are a tree with check boxes;
  a folder can be expanded to its files. The user unticks folders or files, and the confirmation shows the
  narrowed counts and bytes. The narrowing is stored with the run (unselected moves are kept, marked, and can
  be ticked again until the run starts), so the CLI's `cleanup show` and the confirmation agree.
- **Partial undo by folder in the app.** A run's Undo opens the same tree over its quarantined files; the
  user puts back everything, chosen folders or chosen files. The service call already exists (the CLI uses
  it); the panel today passes "everything".
- **Discard a planned run** that never moved a file: `photosquirrel-cli cleanup discard <run>` and a Discard
  button on the plan review and in the history. It touches no file, keeps the run in the history as
  "Discarded", and is refused for any run that has moved a file (even one fully undone).
- **CLI:** `cleanup select <run> [--folder <rel>]... [--file <id>]... [--except-folder <rel>]...
  [--except-file <id>]... | --all` sets a stored plan's selection, `cleanup discard <run>`; both with `--json`.
  `cleanup show` and `cleanup list` report unselected files and discarded runs.
- Catalog migration 16: cleanup tables accept a `discarded` run state and an `unselected` move state
  (ADR-0068).

Note for the owner: the CLI already undoes by folder or file (`cleanup undo --folder/--file`), but `cleanup
plan` takes only one `--folder` and no files. File-level narrowing of a plan is new in the service, not a
reuse; this change adds it once in the service and uses it from both the app and the CLI.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `cleanup`: plan narrowing by folder and file (app and CLI), partial undo by folder in the app, discarding
  a planned run, and those commands in the operation slot and the CLI requirement.

## Impact

- ADRs relied on: 0004, 0019, 0023, 0024, 0027, 0028, 0041, 0061, 0062, 0063, 0064, 0065, 0066, 0067.
  New: ADR-0068 (narrowing marks moves `unselected`, discard is a run state, no cleanup row is deleted,
  migration 16; amends 0061).
- Crates: `ps-model` (`MoveState::Unselected`, `CleanupRunState::Discarded`), `ps-catalog` (migration 16,
  `cleanup_select_moves`, `cleanup_discard_run`), `ps-export::quarantine` (reconcile and counts know the new
  states; no new file operation), `ps-service::cleanup` (`Selection`, `select`, `discard`), `ui-workspace`
  (`views/cleanup.rs`, `scan_backend.rs`), `photosquirrel-cli` (`cleanup select|discard`), README.
- No new dependency, no new file-changing code path: discard and selection only write catalog rows.

## Out of scope

- Re-planning a run with new dispositions (plan again instead), merging or splitting runs, deleting runs or
  pruning the history.
- Choosing files across several sources in one plan; plans stay per source.
- Any change to how files are verified, moved or restored (ADR-0061..0067 stay as they are).
- Emptying the quarantine (never, ADR-0024).
