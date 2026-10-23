# Tasks

## 1. Decision, model and catalog

- [x] 1.1 ADR-0068 set to accepted once 1.3 lands and listed in `docs/adr/README.md` (verify: the index row links the file; status line reads accepted)
- [x] 1.2 `ps-model`: `MoveState::Unselected` (code 6, `as_str` "unselected", not `is_quarantined`) and `CleanupRunState::Discarded` ("discarded"); verify unit tests round-trip every code and string, and that codes 0-5 are unchanged
- [x] 1.3 Catalog migration 16 `0016_cleanup_selection.sql` (rebuild with `defer_foreign_keys`, widened CHECKs, indexes recreated) and `CLEANUP_SELECTION_MIGRATION` in `db.rs`; verify `ps-catalog/tests/cleanup_selection_migration.rs`: a version-15 catalog with runs in every state and moves in every code upgrades with all rows, both indexes and the foreign key intact (`PRAGMA foreign_key_check` empty), and accepts state `discarded` and code 6
- [x] 1.4 `Writer::cleanup_select_moves(run, picked)` (temp table of ids, only rows in states 0/6 change) and `Writer::cleanup_discard_run(run)` (planned -> unselected, run -> discarded with `ended_at`), both `exec_durable`; verify `ps-catalog` unit tests: moves in other states are untouched, 100k picked ids in one call, and `synchronous = FULL` is used (same check as `durable_writes_use_synchronous_full`)

## 2. Service: selection

- [x] 2.1 `ps-service::cleanup::Selection` (folders, files, except_folders, except_files) replacing `UndoSelection`; `picks` unit tests for inclusion, exclusion, subtree semantics and the empty selection; verify `cargo test -p ps-service selection`
- [x] 2.2 `Cleanups::select(run, &Selection)` with `Operation::CleanupSelect`, reconcile first, refused unless `planned`, `EmptySelection` when nothing is picked; verify `scenario_cleanup_plan_narrowed_to_folders`, `scenario_cleanup_plan_narrowed_to_files`, `scenario_cleanup_narrowing_is_reversible_before_the_run`, `scenario_cleanup_empty_selection_refused`
- [x] 2.3 `MoveCounts` gains `unselected`/`unselected_bytes`; `RunSummary` and `cleanup list/show` JSON carry them; verify the narrowing scenarios assert the counts and the run's `skipped` stays 0
- [x] 2.4 `quarantine::reconcile` treats `Unselected` like `Planned` (suspect only when the JSON journal records a move); verify a `ps-export` test that an unselected move recorded as moved by `journal.jsonl` is adopted as quarantined

## 3. Service: discard

- [x] 3.1 `Cleanups::discard(run)` with `Operation::CleanupDiscard`, reconcile first, `NotDiscardable` when any move is or was moving/quarantined/restoring/restored or the run's `journal.jsonl` has an intent; run, select and undo refuse a discarded run; verify `scenario_cleanup_planned_run_discarded`, `scenario_cleanup_discard_refused_after_a_move`
- [x] 3.2 `reconcile_locked` keeps reconciling discarded runs that have a journal, and `reconcile_run` returns a discarded run that adopted a move to `completed` with a log line; verify `scenario_cleanup_discard_checks_the_journal_first` (journal planted with a move while the catalog says planned) and `scenario_cleanup_discarded_run_on_an_offline_drive` (source root renamed away for the discard, restored after; tree snapshot unchanged, run still discarded)
- [x] 3.3 Operation slot covers select and discard; verify `scenario_cleanup_selection_and_discard_wait_for_a_live_run` (hook pauses a live run; both refused naming it) and that `scenario_cleanup_operations_exclude_each_other` still passes
- [x] 3.4 Never-delete holds: extend `scenario_cleanup_cleanup_never_deletes` to narrow, run, partially undo and discard another plan of the same drive; verify the file count of tree plus quarantine is unchanged and every file has unchanged bytes

## 4. CLI

- [x] 4.1 `cleanup select <run>` (`--folder`, `--file`, `--except-folder`, `--except-file`, repeatable; `--all`), `cleanup discard <run>`, `undo` on `Selection` (gains `--except-*`), `list`/`show` print unselected files and the discarded state, all with `--json`; verify `scenario_cleanup_cli_select_and_discard` in `photosquirrel-cli/tests/cleanup.rs` and that `scenario_cleanup_cli_undo_round_trip` still passes
- [ ] 4.2 README cleanup section: narrowing, partial undo in the app, `cleanup select` and `cleanup discard` examples; verify each documented command line parses (`photosquirrel-cli cleanup select --help` and `discard --help` match the flags named)

## 5. App

- [x] 5.1 Backend trait: `cleanup_tree`, `cleanup_folder_files`, `cleanup_select`, `cleanup_discard`, `cleanup_undo(run, Selection)` on the real backend and the test fake; verify `cargo test -p ui-workspace scan_backend` (tree totals equal `MoveTotals` for a corpus plan)
- [x] 5.2 `views/cleanup_tree.rs` `SelectionTree`: path tree with subtree totals, tri-state nodes, untick/re-tick folders and files, `Selection` and picked count/bytes out; verify unit tests (untick a folder then re-tick its parent clears child exceptions; picked bytes equal the sum of picked files on a 3-level tree)
- [x] 5.3 Review card renders the tree with `uniform_list` (expanded rows only, files loaded on expand), stats show picked numbers, "Move N files…" applies the selection on a background thread before the confirmation, disabled with "Nothing selected" when empty; verify `scenario_cleanup_app_narrows_a_plan_before_confirming` (headless `#[gpui_kit::test]` in `ui-workspace/tests/cleanup.rs` on a tiny corpus: untick one folder, confirm, run; snapshot shows that folder unchanged)
- [x] 5.4 Discard: "Discard plan" on the review and "Discard" on history rows of never-moved runs, inline confirmation, refusal shown in the error note, "Discarded" label without Review/Undo; verify `scenario_cleanup_app_discards_a_plan`
- [x] 5.5 Undo selection: "Undo…" opens the tree over quarantined moves, "Put back N files" undoes the picked ones; verify `scenario_cleanup_app_undoes_one_folder` and that `cleanup_panel_undo_restores_a_run` (whole run) still passes
- [x] 5.6 Tokens and layout: no raw colours or sizes; a 20k-folder synthetic tree renders its first frame and scrolls without building off-screen rows; verify `cargo xtask tokens` and a headless test asserting at most (visible rows + overscan) row elements exist
- [ ] 5.7 Screenshot: recapture `docs/screenshots/cleanup-dark.png` with a narrowed plan (one folder unticked); verify `cargo xtask screenshot -- --theme dark` output reviewed and the README image updated

## 6. Verification

- [ ] 6.1 `cargo xtask layering` passes with the forbidden-call allow-list unchanged (no new rename/link/unlink/create_dir call site)
- [ ] 6.2 Round trip on the small corpus via the release CLI: plan a drive, `select --folder` one folder, `run --yes`, `undo --folder` part of it, `undo --yes` the rest, plan again and `discard`; tree snapshots before and after are equal and `report` shows 0 quarantined
- [ ] 6.3 `cargo xtask spec-coverage` reports every scenario of this change mapped to a test
- [ ] 6.4 `cargo xtask ci` is green
- [ ] 6.5 `openspec validate cleanup-selection --strict` passes
