# Design

## Context

ADR-0022 defines layout, copy protocol, sidecars, manifest and profiles; ADR-0023 the ledger and dispositions;
ADR-0004 makes `ps-export` the only crate allowed to mutate user paths, and only the library here. Full hashes
for sampled identities come from ADR-0011. AI keywords use the effective insight (ADR-0021: advisory, so AI never
changes *what* is exported, only sidecar content). Keeper sets come from ADR-0014.

## Goals / Non-Goals

**Goals:**
- Zero risk to sources; zero overwrite in the library; every byte verified; restartable at any instant.
- A trustworthy "100% accounted for".

**Non-Goals:**
- Move/cleanup (`quarantine-cleanup`), EXIF rewriting, import automation.

## Decisions

- **Crates/modules.** `ps-export::plan` (keepers minus ledger, target path resolution, collision planning,
  free-space via `statvfs`/`GetDiskFreeSpaceExW`), `ps-export::copy` (copier), `ps-export::xmp` (`quick-xml`
  writer: `dc:subject`, `dc:description`, `photosquirrel:` namespace with hash and sources), `ps-export::manifest`,
  `ps-export::profile`, `ps-export::pairs`. `ps-service::exports` and `ps-service::reports`. UI:
  `ui-workspace::views::{export, reports}`; inspector disposition row; gallery disposition filter.
- **Copy protocol.** Open source read-only; create `<target>.ps-partial` with `create_new(true)` (fails if it
  exists -> it is ours from a crashed run only if the ledger has an `in_progress` row for it, else suffix);
  stream 4 MB chunks into BLAKE3; `sync_all`; compare: full identity -> equal; sampled identity -> recompute
  sampled from the copy and compare, and record the streamed full hash; set mtime (`File::set_modified`); rename
  with no-replace semantics (`renameat2(RENAME_NOREPLACE)` on Linux via `rustix`, `renamex_np(RENAME_EXCL)` on
  macOS, `MoveFileExW` without `REPLACE_EXISTING` on Windows) so a race can never overwrite; fsync the directory
  on Unix. Ledger row written `in_progress` before and `done` after (same writer, ADR-0005).
- **Undated layout.** ADR-0022 says undated files go to `<library>/Undated/` "marked undated" without a
  sub-layout; we use `Undated/<YYYY>/<YYYY-MM-DD>/` by file mtime to avoid one huge folder. Recorded here;
  flagged in the report as an ADR clarification.
- **Live Photo pairing.** ADR-0022 requires keeping HEIC+MOV pairs together but does not define detection: we pair
  a still and a video in the same folder with the same stem whose Apple ContentIdentifier (MakerNote / QuickTime
  `com.apple.quicktime.content.identifier`) matches, falling back to same stem + capture time within 2 s.
  `media-metadata` gains ContentIdentifier extraction in this change.
- **Schema (migration 6).** `export_library(id, root_path, volume_uuid, created_at)`,
  `export_ledger(library_id, content_id, full_hash, file_id, library_rel_path, size, state, profile, at)`.
- **Dispositions.** A SQL view `file_disposition` computed from `file`, `dup_member`, `dup_group`,
  `user_inclusion` and `export_ledger`, plus a cheap library existence/size check cached per report refresh; verify
  mode re-hashes. Excluded includes `thumbnail_cache` files that are in no group (a cache copy in a group is
  covered once its keeper is exported).
- **CLI.** `export --library <dir> --profile <apple|google|lightroom> [--sidecars on|off] [--dry-run] [--yes]
  [--json]`, `report [--source <id>] [--folder <rel>] [--verify] [--json]`.
- **Fault injection.** `ps-export::copy` takes a `Sink` trait so tests inject ENOSPC and short writes.

## Risks / Trade-offs

- [No-replace rename unavailable on some NAS filesystems (SMB)] -> fall back to "check-then-rename" guarded by
  `create_new` partials and re-verify after rename; logged.
- [Disposition view cost on 1M files] -> materialise per-report into a temp table; budgeted by tests at 1M rows.
- [Free-space check races] -> disk-full handling is the backstop.

## Verification

- `cargo test -p ps-export -p ps-service exports reports` (all `scenario_export_*`, `scenario_completion_report_*`).
- `cargo test -p photosquirrel-cli export report`.
- End to end: `photosquirrel-cli fixtures generate --seed 42 --preset small --out /tmp/c`, add drives, scan,
  `photosquirrel-cli export --library /tmp/lib --profile lightroom --yes --json`, re-run (0 copied),
  `photosquirrel-cli report --json`.
- `cargo xtask screenshot --all-surfaces` (export plan, reports), `cargo xtask ci`,
  `openspec validate export --strict`.
