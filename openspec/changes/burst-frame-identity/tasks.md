# Tasks

Ships in 0.3.0. Starts after `excluded-view-filters` is archived and before `preview-placeholders` (which moves to
migration 24).

## 1. Decisions

- [x] 1.1 Write the ADR "Recorded frame identities keep frames apart" (D1-D4; amends 0035) from the next free number, accepted, with an index row. Confirm migration 23 is free. Verify: the index lists it, and `ls crates/ps-catalog/migrations` has no 0023. Done: ADR-0105 (index row added); migrations end at 0022.

## 2. Corpus first

- [ ] 2.1 `ps-fixtures`: converted RAW burst sets (D6): CR2-style original, camera JPEG and converted DNG per frame (`OriginalRawFileName`, a Canon maker note with the file number inside `DNGPrivateData`, the first frame's sub-second dropped). Truth groups and must-not-merge sets. Verify: `scenario_test_corpus_converted_bursts_in_the_medium_preset`, and the generator stays deterministic for a seed.
- [ ] 2.2 Show the bug: with the new sets and today's grouping, `eval-corpus` reports must-not-merge violations. Verify: `scenario_test_corpus_the_corpus_catches_a_merged_converted_burst` (test hook that turns frame identity off), and record the violation count in design.md.

## 3. Metadata

- [ ] 3.1 `ps_model` `FrameId` and `fields.frame_id`; `ps-media` reads DNG 0xC68B, the Canon maker-note `FileNumber`, and maker notes inside `DNGPrivateData` (`has_maker_notes`). Verify: `scenario_media_metadata_converted_dng_records_its_frame`, `scenario_media_metadata_frame_identity_costs_no_extra_read` (byte-counting layer), fuzz-style truncation tests of the `DNGPrivateData` parser.

## 4. Grouping

- [ ] 4.1 `Contradiction::FrameId` and its explanation; RAW-format files count as camera originals in the two-originals guard (D2, D3). Verify: `scenario_duplicate_detection_converted_burst_frame_without_sub_second_not_merged`, `scenario_duplicate_detection_resized_copy_still_joins_its_original`, `scenario_duplicate_detection_two_converted_dngs_of_two_frames_not_merged`, and every existing `scenario_duplicate_detection_*` test still passes.

## 5. Catalog and rescan

- [ ] 5.1 Migration 23: `file.frame_id`, `file.meta_version`, non-RAW rows stamped to the current version without reading. Verify: a migration test on a catalog written by migration 22.
- [ ] 5.2 Rescan: re-read RAW metadata for rows below `META_LOGIC_VERSION` (header bytes only), bump the dedup stamp, report re-read files and split groups. Verify: `scenario_incremental_rescan_old_catalog_splits_merged_burst_frames_on_rescan`, `scenario_incremental_rescan_the_re_read_happens_once`, and every existing `scenario_incremental_rescan_*` test still passes.

## 6. Export, report and cleanup

- [ ] 6.1 A split frame is pending and the next export copies it; a quarantined file whose group changed is pending with the reason and the restore hint, in the CLI report and the app. Verify: `scenario_cleanup_quarantined_frame_of_a_split_burst_is_pending`, an export test that copies exactly the split frame's keeper.

## 7. Verification

- [ ] 7.1 Corpus gate on the medium preset (seed 42, drive-a/b/c): precision, recall and keeper accuracy 1.0000, 0 must-not-merge violations; record the new pairs, groups, files, reclaimable bytes and suggestions as the baseline in the gate docs and the memory of the release notes.
- [ ] 7.2 The owner's SD card (read-only mount, catalog under `target/`, never exported or cleaned up): rescan the existing scratch catalog with the new build; the five mixed groups (frames 0022, 0024, 0028, 0033, 0035) split, no group mixes two Canon file numbers, and the summary reports the re-read. Record the counts, not the file names, in design.md.
- [ ] 7.3 `cargo xtask ci` green in a clean worktree; `openspec validate --all --strict`; GitHub CI green on all five jobs.
- [ ] 7.4 Gate review by a fresh agent that did not write the code: every scenario, the corpus change, the re-read cost, and that no legitimate derivative was split (the full medium preset and the card).
