# 0061. Cleanup journal, catalog migration 14 and quarantine moves

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0024 decides that cleanup moves already-safe files into `<source root>/.photosquirrel-quarantine/<run id>/`
with a journal in the catalog and a JSON journal inside the quarantine folder, and ADR-0034 adds the
`quarantined` disposition. Neither fixes the catalog tables, the journal file format, the move primitive or how a
crash is reconciled. Cleanup is the only feature that changes source trees, so these need one explicit record.
ADR-0038's no-overwrite rename for export falls back to link+unlink, which removes a name of the user's file: not
acceptable for source files, whose only allowed change is a rename.

## Decision
1. **Catalog migration 14** adds `cleanup_run` (source, source root at plan time, library, optional folder,
   state `planned | running | completed | undone`, times) and `cleanup_move` (one row per planned file: path,
   quarantine path, stat fingerprint, content identity, prior disposition, the covering library path, state,
   note). Move states, stable codes: 0 planned, 1 moving (intent), 2 quarantined, 3 skipped, 4 restoring (undo
   intent), 5 restored. Neither table has a foreign key to `file` or `source`: like the export ledger, the record
   of what was moved must outlive removing a source's catalog records. A file is quarantined while one of its
   moves is in state 2 or 4; dispositions derive from that (no stored disposition).
2. **The JSON journal** is `<source root>/.photosquirrel-quarantine/<run id>/journal.jsonl`: one JSON object per
   line, appended and fsynced, never rewritten. The first line is a header (`event: "run"`, `v: 1`, run id,
   a run `uid`, source root); then `intent`, `done`, `skipped`, `undo_intent`, `restored` and `conflict` lines,
   each with the file id, the original and quarantine paths (relative to the source root, plus hex bytes when not
   UTF-8), size and content hash. A run folder whose journal header carries another `uid` (another catalog's run
   with the same number) is never used: the run is refused.
3. **Order per move**: catalog intent (committed) -> journal `intent` (fsynced) -> rename -> journal `done` ->
   catalog quarantined. Undo mirrors it. **Reconciliation** (every cleanup command and the Cleanup view start with
   it) resolves each move left in state 1 or 4 from the file system: whichever of the two paths holds the file
   (same size, and the content hash when it can be read) is the truth; if both or neither do, the move is
   recorded as skipped with the reason and nothing is touched.
4. **The move primitive** (`ps_export::quarantine`): `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)`
   via `rustix`, after checking that the file and the quarantine folder are on the same device (`st_dev`); `EXDEV`
   is a refusal too. Where the flag is unsupported (Windows, some network file systems) the fallback is
   check-then-rename with `std::fs::rename`, which never copies across volumes (`EXDEV`/`ERROR_NOT_SAME_DEVICE`
   are refusals), followed by a re-stat that the file at the target is the moved one. There is **no link+unlink
   and no copy-then-delete** path for source files. Quarantine folders are created one level at a time and every
   existing component must be a real folder (a symlinked quarantine folder is refused). The only removal is
   `remove_dir` of empty folders PhotoSquirrel created inside the run's quarantine folder, after a full undo.

## Alternatives considered
- **Reuse `rename_no_replace` from export** - its link+unlink fallback unlinks a name of the user's file.
- **Journal only in the catalog** - a lost or replaced catalog would leave the user with an unexplained folder;
  the JSON file makes every move recoverable by hand.
- **Journal only on disk** - read-only or full drives would lose the record, and dispositions need SQL.

## Consequences
- Two new tables; the journal file is a stable format (version field `v`).
- Cleanup on file systems without an atomic no-replace rename has a small race window between the check and
  the rename, like export's last resort (ADR-0038); the post-rename re-stat reports it.

## Revisit when
A safe crate exposes `MoveFileExW` without replacement on Windows, or the journal format needs a new event.
