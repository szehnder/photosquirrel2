# Design

## Context

See proposal.md for why. What exists today (read from the code, 2026-10-22):

- `ps-service/src/cleanup.rs`: `Cleanups::plan(source, folder: Option<RelPath>, library)` stores a `planned`
  run with one `cleanup_move` row per eligible file (`ps_export::quarantine::plan_moves`); `run_with_hooks`
  moves only rows in `MoveState::Planned` (`ps-export/src/quarantine/run.rs:135`); `undo(id, &UndoSelection
  { folder, files })` restores the picked quarantined moves; every command takes the operation slot
  (`crate::ops`, ADR-0065) and reconciles first (`reconcile_locked`).
- `photosquirrel-cli/src/cmd/cleanup.rs` + `main.rs` `CleanupCommand`: `plan --source [--folder]`,
  `run <id> --yes [--network-ok]`, `list`, `show <id>`, `undo <id> [--folder] [--file]... --yes`.
  So the CLI narrows a plan to one folder at plan time and undoes by folder or files; it cannot pick files of
  a plan, several folders, or discard.
- `ui-workspace/src/views/cleanup.rs`: `CleanupView::plan(source)` calls `ScanBackend::cleanup_plan(source)`
  (whole source, `scan_backend.rs:704` passes `None` for the folder); the review card lists the 5 largest
  folders and 3 sample moves; `undo()` calls `cleanup_undo(run)`, which passes `UndoSelection::all()`
  (`scan_backend.rs:746`).
- `ps-catalog/migrations/0014_cleanup.sql`: `cleanup_run.state CHECK IN ('planned','running','completed',
  'undone')`, `cleanup_move.state CHECK BETWEEN 0 AND 5`. Latest migration is 15.
- A run's quarantine folder and `journal.jsonl` are created lazily: `prepare_dir` for the first file's target
  folder, then `Journal::open` right before the first intent (`run.rs:279`, `run.rs:321`). A run that was
  never started has no folder on disk; a stopped one whose attempts were all skipped may have folders but no
  journal.

## Goals / Non-Goals

**Goals:**
- One selection type used by plan narrowing and undo, in the service, so the app and the CLI share it.
- Narrowing and discarding write catalog rows only; the set of code paths that rename files is unchanged.
- The confirmation, `cleanup show` and the run agree on exactly which files move.
- The panel stays responsive on a 1M-file plan (tree virtualized, files of a folder loaded on expand).

**Non-Goals:**
- Changing verification, journaling or move primitives (ADR-0061, 0062, 0064).
- Re-planning with fresh dispositions; widening a run beyond the files it was planned with.

## Decisions

1. **Selection type (`ps-service::cleanup::Selection`).** Replaces `UndoSelection` (renamed; its callers are
   `scan_backend.rs`, `cmd/cleanup.rs` and `ps-service/tests/cleanup.rs`):
   `Selection { folders: Vec<RelPath>, files: Vec<FileId>, except_folders: Vec<RelPath>, except_files:
   Vec<FileId> }`. `picks(m)` = (`folders` and `files` both empty, or `m.rel_path.is_within` any folder, or
   `m.file` in `files`) and not within any `except_folders` and not in `except_files`. Folder semantics are
   subtree (`RelPath::is_within`), the same as `cleanup plan --folder` and `undo --folder`. Inclusion lists
   serve the CLI; exclusion lists serve the app, which starts with everything ticked and records unticks,
   so the selection stays small however big the plan is.
2. **Narrowing marks moves `unselected` (ADR-0068).** `MoveState::Unselected` = code 6 in `ps-model`
   (`code`, `from_code`, `as_str`; not `is_quarantined`). `Cleanups::select(run, &Selection) ->
   Result<RunSummary>`: writable check; operation slot `Operation::CleanupSelect(run)`; `reconcile_locked`;
   refuse unless the run is `planned` (the state a stopped run returns to, `cleanup.rs:584`); compute the
   picked set over moves in `Planned | Unselected`; refuse when it is empty (`CleanupError::EmptySelection`,
   "discard the plan instead"); then `Writer::cleanup_select_moves(run, picked: Vec<FileId>)` in one
   `exec_durable` transaction: `UPDATE cleanup_move SET state = CASE WHEN file_id IN picked THEN 0 ELSE 6
   END WHERE run_id = ? AND state IN (0, 6)` (picked ids go through a temp table, not an `IN` list, so 1M
   ids are fine). No source access is needed, so selecting works with the drive disconnected. The run code
   needs no change: it already filters `Planned`.
3. **Discard is a run state (ADR-0068).** `CleanupRunState::Discarded` ("discarded"). `Cleanups::discard(run)
   -> Result<RunSummary>`: writable; slot `Operation::CleanupDiscard(run)`; `reconcile_locked` (this is what
   catches a journal the catalog does not know about: reconciliation adopts the journaled move and the
   refusal below then sees it); refuse when the run is not `planned`, when any move is `Moving |
   Quarantined | Restoring | Restored` (`CleanupError::NotDiscardable { id, why }`, naming up to 3 paths and
   a count), and, when the run's root is reachable, when `<root>/.photosquirrel-quarantine/<run>/journal.jsonl`
   has any `Intent` line (`quarantine::journal::read_journal`); then `Writer::cleanup_discard_run(run)`:
   in one durable transaction, `planned -> unselected` for its moves and `state = 'discarded', ended_at =
   now` for the run. Nothing on disk is touched; leftover empty folders from a stopped run stay.
   `run_with_hooks`, `select` and `undo` refuse a discarded run (`NotRunnable { why: "it was discarded" }`).
   `reconcile_locked` keeps including discarded runs that have a `journal.jsonl` (today it skips only
   `undone`); `reconcile_run` sets a discarded run to `completed` if reconciliation adopted any move, and logs
   it. `quarantine::reconcile` treats `Unselected` like `Planned` (suspect only when the journal says moved).
4. **Migration 16** (`ps-catalog/migrations/0016_cleanup_selection.sql`, `db.rs` `CLEANUP_SELECTION_MIGRATION`):
   rebuild `cleanup_run` and `cleanup_move` with the widened `CHECK`s. The runner (`db.rs` `migrate`) runs
   each migration's SQL in one `IMMEDIATE` transaction with `foreign_keys = ON` set at open, and that pragma
   cannot change inside a transaction, so the SQL starts with `PRAGMA defer_foreign_keys = ON` (allowed in a
   transaction; checked at commit) as a belt-and-braces measure, and then: drop both indexes; rename
   `cleanup_move` to `cleanup_move_old` and `cleanup_run` to `cleanup_run_old` (with `legacy_alter_table` off
   the old move table's reference follows the rename); create `cleanup_run` (same columns incl.
   `source_added_at`, `CHECK (state IN ('planned','running','completed','undone','discarded'))`), copy; create
   `cleanup_move ... REFERENCES cleanup_run(id) ... CHECK (state BETWEEN 0 AND 6)`, copy; drop
   `cleanup_move_old`, then `cleanup_run_old` (child first: no step ever leaves a move whose run is missing,
   so no foreign-key violation is ever counted); recreate `cleanup_move_by_state` and `cleanup_move_by_path`.
   (Implementation note: the first draft created `*_new` tables and dropped the old run table while the new
   move table still referenced the name `cleanup_run`; that relies on deferred-violation bookkeeping across a
   DROP and a RENAME, so the order was changed.) No change to the runner. `RUN_COLUMNS` is unchanged.
   Readers on an older schema (observer mode) keep working: they never see the new values. If another M7
   change lands a migration 16 first, renumber (ADR-0068 says the number is not the decision).
5. **Counts.** `MoveCounts` gains `unselected` and `unselected_bytes`; `CleanupRunRow` (UI) gains the same;
   `cleanup list/show` JSON gains them and the `discarded` state. `ps_export::quarantine::totals` is called
   on picked moves only for the review, and on all `Planned | Unselected` moves for the selection tree.
6. **Backend trait (`ui-workspace/src/scan_backend.rs`).** New and changed methods, all called from
   background threads by the view, like today:
   `cleanup_tree(run) -> CleanupTree` (every `Planned | Unselected` folder with files/bytes and how many are
   picked, for narrowing; or every quarantined folder, for undo — a `CleanupTreeKind` argument);
   `cleanup_folder_files(run, folder, kind) -> Vec<CleanupFileRow { file, name, size, picked }>` (direct
   files of one folder, loaded when it is expanded); `cleanup_select(run, Selection) -> CleanupReview`;
   `cleanup_discard(run) -> CleanupRunRow`; `cleanup_undo(run, Selection) -> UndoOutcome` (replaces the
   no-selection form). The in-memory fake backend used by UI tests gets the same methods.
7. **The panel (`views/cleanup.rs`).** A `SelectionTree` model (new file `views/cleanup_tree.rs`, pure, unit
   tested): folders from `CleanupTree` folded into a path tree with subtree totals; tri-state per node
   (all / some / none); toggling a node records it in `except_folders` (or removes it, or removes child
   exceptions when re-ticking a parent); toggling a file records `except_files`; it yields the `Selection`
   and the picked count/bytes (exact: folder totals are known, and a file can only be unticked after its
   folder's files were loaded, so its size is known). Rendering uses `uniform_list` over the visible
   (expanded) rows, as `library.rs:1064` does, so a 50k-folder plan scrolls without building 50k elements.
   - Review: the folder list becomes the tree (check box, chevron, path, files, size); the header stats show
     the picked numbers; "Move N files…" first calls `cleanup_select` on a background thread when the
     selection differs from the stored one, then shows the confirmation from the returned stored counts (the
     confirmation never shows numbers the catalog does not hold). The button is disabled with "Nothing
     selected" when the tree picks nothing.
   - Discard: a "Discard plan" ghost button on the review and a "Discard" button on history rows of runs
     with `planned + unselected > 0` and nothing moved; an inline confirmation ("Discard plan N? Nothing on
     the drive changes."), then `cleanup_discard`; refusal text is shown in the existing error note.
   - Undo: "Undo…" on a history row opens the tree in undo mode (quarantined moves, all ticked) with "Put
     back N files" (the existing undo confirmation text with the picked count); `cleanup_undo(run,
     selection)`.
   - States: `CleanupPhase` gains `Selecting` (applying a selection) and `Discarding`; `busy()` covers them;
     `state_label` gains "Discarded". All colours and sizes from `ui-theme` tokens (ADR-0028).
8. **CLI (`main.rs`, `cmd/cleanup.rs`).** `cleanup select <run> [--folder]... [--file]... [--except-folder]...
   [--except-file]... [--all]` (`--all` = empty selection = everything; at least one flag required),
   prints the new totals per folder like `plan`; `cleanup discard <run>` (no `--yes`: it moves nothing and is
   refused for anything that did; exit 1 with the reason on refusal). `undo` uses `Selection` and gains
   `--except-folder/--except-file` for free. `list` shows "discarded"; `show` lists unselected files.
9. **Safety review.** No new call to rename, link, unlink or `create_dir*`: `cargo xtask layering`'s
   forbidden-call allow-list is unchanged and its test must still pass. The confirmation (ADR-0004), the
   re-verification before each move, both journals, the operation slot and source identity (ADR-0063) are
   untouched because selection only changes which rows are `planned`.

## Effort

About 3-4 working days for one agent: catalog + model + migration with tests (0.5 d), service `select`/
`discard`/reconcile changes with tests (1 d), CLI (0.5 d), the panel's tree, discard and undo selection with
headless tests (1.5 d, the bulk: tri-state tree + virtualized list + three flows), README and screenshot
(0.25 d). Risk is in the UI, not in the file-changing code, which does not change.

## Risks / Trade-offs

- [A selection applied while the user is still looking at a stale tree] -> `cleanup_select` returns the stored
  review, which replaces the tree's counts before the confirmation is shown; the run moves exactly the stored
  `planned` rows.
- [Table rebuild in migration 16 on a big catalog] -> one-time, inside a transaction; at 1M moves a few
  hundred MB copied, seconds on SSD; tested by a migration-upgrade test with rows in every state.
- [Discarding offline misses a journal the catalog lost] -> reconciliation keeps watching discarded runs and
  revives one whose journal records a move (ADR-0068, scenario "Discarded run on an offline drive" plus a
  service test that plants such a journal after the discard).
- [Users expect "Discard" to free space] -> the label says plan, the confirmation says nothing on the drive
  changes, and only planned (never moved) runs offer it.

## Migration Plan

Catalog migration 16 runs on first open by this version (owner process only, ADR-0041). Rollback: an older
build refuses a version-16 catalog (existing newer-schema refusal); there is no down-migration, as for
earlier migrations.

## Verification

- `cargo test -p ps-catalog cleanup` (migration 16 upgrade with rows in all states; select/discard writes).
- `cargo test -p ps-export quarantine` (reconcile treats `unselected` like `planned`).
- `cargo test -p ps-service cleanup` (all new `scenario_cleanup_*` service scenarios).
- `cargo test -p photosquirrel-cli cleanup` (`scenario_cleanup_cli_select_and_discard`).
- `cargo test -p ui-workspace --test cleanup` (headless panel tests) and `cargo xtask screenshot -- --theme
  dark` for the review with a narrowed tree.
- `cargo xtask layering`, `cargo xtask tokens`, `cargo xtask ci`, `openspec validate cleanup-selection --strict`.
