# Design

## Context

ADR-0024 defines quarantine cleanup; ADR-0023 restricts it to `exported`/`covered`; ADR-0004 confines source
mutation to `ps-export` and requires dry-run + confirmation; ADR-0006 gives source roots and volume identity;
ADR-0011/0022 give the hashes to re-verify. This is the only feature that changes source trees, and only by
reversible renames.

## Goals / Non-Goals

**Goals:**
- Provable round trip on the corpus: tree snapshot before cleanup == after undo.
- Every failure mode (changed file, missing library copy, crash, occupied path) leaves files in place and visible.

**Non-Goals:**
- Deletion, trash, emptying quarantine, cross-volume moves.

## Decisions

- **Module.** `ps-export::quarantine::{plan, run, undo, reconcile}`; the layering/forbidden-call check from
  `scanning` allows `fs::rename` only in `ps-export::copy` (library) and `ps-export::quarantine`, and
  `fs::remove_file` only for PhotoSquirrel's own `.ps-partial` and journal temp files.
- **Plan.** Query `file_disposition IN (exported, covered)` for a source or folder; group by folder; store as
  `cleanup_run(id, source_id, state=planned, created_at)` + `cleanup_move(run_id, file_id, rel_path,
  quarantine_rel_path, content_id, covering_library_path, state)`.
- **Run.** For each move: stat the source (fingerprint must match the catalog), re-hash (full BLAKE3; for sampled
  identities re-check sampled and compare size), confirm the covering library copy exists and its full hash
  equals the ledger's (cached per library file within a run); journal `intent` (catalog + append to
  `.photosquirrel-quarantine/<run>/journal.jsonl`, fsync); `create_dir_all` for the quarantine parent;
  no-replace rename (same primitives as export); journal `done`. Same-volume check: compare device ids
  (`st_dev` / volume serial) of the file and the quarantine root before renaming; rename errors `EXDEV` are also
  treated as refusal.
- **Reconcile.** On start-up, `intent` entries without `done` are resolved by checking which of the two paths
  exists (identity by size + content hash if both exist, which should not happen with no-replace rename).
- **Undo.** Reverse renames with no-replace semantics; conflicts reported, left in quarantine; empty quarantine
  directories are left (never deleted, ADR-0024), except PhotoSquirrel removes directories it created only if
  empty after a full undo - New ADR not needed (directories, not user files) but documented in the README.
- **Scanner and reports.** `ps-scan::walk` already skips the reserved folder name (from `scanning`). Gap:
  ADR-0023 has no `quarantined` disposition; files moved to quarantine would otherwise become `missing`.
  **New ADR needed: "Quarantined files in dispositions"** (proposed ADR-0034): a file with a `done` cleanup move
  keeps its prior disposition with a `quarantined` flag and its quarantine path; completion reports show
  quarantined counts and bytes.
- **CLI.** `cleanup plan --source <id> [--folder <rel>] [--json]`, `cleanup run <run-id> --yes`, `cleanup list`,
  `cleanup undo <run-id> [--folder <rel>] [--yes]`.
- **UI.** `ui-workspace::views::cleanup`: plan review with folder tree, bytes reclaimed, big explicit confirm,
  progress, history with Undo.

## Risks / Trade-offs

- [Rename on SMB/NAS shares lacks no-replace semantics] -> check-then-rename with post-rename verification;
  cleanup on network sources requires an extra confirmation.
- [Re-hashing makes cleanup as slow as a scan] -> acceptable; safety over speed, progress shown.
- [User edits files in quarantine] -> undo re-hashes before restoring and reports changed files (still restores,
  since restoring cannot lose data, but flags them).

## Verification

- `cargo test -p ps-export quarantine` and `cargo test -p ps-service cleanup` (all `scenario_cleanup_*`).
- `cargo test -p photosquirrel-cli cleanup` (CLI round trip on the small corpus).
- `cargo xtask layering` (forbidden-call allow-list), `cargo xtask ci`, `openspec validate quarantine-cleanup --strict`.
