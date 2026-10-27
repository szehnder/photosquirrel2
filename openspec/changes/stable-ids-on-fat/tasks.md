# Tasks

Ships in 0.3.x (ROADMAP row 30), after `quiet-decoder-logs`. Takes **ADR-0112**. **No migration** (D2, D5); migration
26 goes to `adobe-rgb-color`.

## 1. Decision

- [ ] 1.1 Write ADR-0112 "File ids are used only on filesystems that keep them" (D1-D3, D5; amends 0007), accepted, with an index row. Verify: the index lists it.

## 2. Model and probe

- [ ] 2.1 `ps_model::fs_ids` (`IdStability`, `stable_for_magic`, `stable_for_name`, `is_fat_time`) and `StatFingerprint::unchanged_with(.., FatTime)`. Verify: table unit tests; the existing `fingerprint_ignores_missing_file_index` still passes.
- [ ] 2.2 The Linux and macOS probe in `ps-scan/src/fs.rs` (cached by `st_dev` per walk; FUSE through `source::imp::mount_of`), `fingerprint` writing `None` on `Unstable`, `WalkEntry.fat_time`. Verify: unit test on this machine's target dir reports `Stable`; `cargo xtask layering` green.

## 3. Diff and catalog

- [ ] 3.1 Test seams on `Instrumentation` (`force_ids`, `force_fat_time`, `remap_file_index`, `shift_mtime`) (D4). Verify: `scenario_incremental_rescan_new_inode_numbers_still_mean_changed_where_ids_are_stable` (shows the seam bites).
- [ ] 3.2 Diff and `UPSERT_SQL` with the FAT hour rule as one rule. Verify: `scenario_incremental_rescan_remounted_fat_card_reads_nothing`, `scenario_incremental_rescan_first_rescan_after_upgrading_reads_nothing`, `scenario_incremental_rescan_renamed_file_on_fat_is_detected`, `scenario_incremental_rescan_replaced_file_on_fat_is_detected`, `scenario_incremental_rescan_fat_time_zone_shift_is_not_a_change`, `scenario_incremental_rescan_other_shifts_are_changes`, and every existing `scenario_incremental_rescan_*` test.

## 4. Sources and cleanup

- [ ] 4.1 `VolumeInfo.ids`, the hint's `ids=`, `SourceInfo.stable_file_ids`, `sources list --json`. Verify: `scenario_sources_exfat_source_shows_unstable_file_ids` (probe forced).
- [ ] 4.2 ps-export pre-move check skips the file-id comparison on `Unstable` (`rustix::fs::statfs`). Verify: `scenario_sources_quarantine_after_a_remount_is_not_refused_for_a_new_inode`, and the existing quarantine tests (a changed file is still refused by size, time or content).

## 5. Docs

- [ ] 5.1 README: remove the FAT/exFAT "Known limitations" entry; add a sentence that FAT cards rescan quickly after a remount. The 0.3.x release-notes line. Verify: `grep -n 'stable file ids' README.md` finds nothing.

## 6. Verification

- [ ] 6.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [ ] 6.2 Corpus gate unchanged (no grouping change): medium preset seed 42, precision, recall and keeper accuracy 1.0000, 0 violations, same pair and group counts as `preview-placeholders`.
- [ ] 6.3 Owner check (read-only, owner-run, no screenshots without approval): scan the Canon card, unmount, remount, rescan: 0 changed, no file read (`--json` diff counts); `sources list --json` shows `stable_file_ids: false`.
- [ ] 6.4 Budgets: `cargo xtask bench --check`, `rescan-nochange-100k` within budget (one `statfs` per device).
- [ ] 6.5 Pushed; GitHub CI green on all five jobs.
- [ ] 6.6 Gate review by a fresh agent that did not write the code.
