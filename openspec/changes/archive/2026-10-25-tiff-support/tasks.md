# Tasks

## 1. Decisions

- [x] 1.1 ADR-0084 (TIFF directories beyond the probe window), ADR-0085 (strip-wise TIFF decoding), ADR-0086 (one-pass analysis of TIFFs above 64 MB) and ADR-0087 (TIFF decode budgets) accepted and listed in `docs/adr/README.md` with their amends notes; roadmap row added; verify `openspec validate tiff-support --strict`

## 2. Fixtures

- [x] 2.1 `ps-fixtures`: TIFF writer gains a data-first layout (directories after the pixel data) and several strips/tiles; `tiff_image::TiffSpec` covers layouts, sample formats, colour models, compression (none, LZW, Deflate via the `tiff` encoder, JPEG YCbCr), pages, reduced-resolution levels, orientation and EXIF; verify `scenario_test_corpus_end_directory_tiff_generated` and `scenario_test_corpus_corpus_presets_contain_no_tiff`

## 3. Probe

- [x] 3.1 `ps-media::tiff`: `View`/`Segment`, `walk_noting`, `missing_ranges`, `load_directories` (1 MiB, 64 reads, read-ahead, never twice), `metadata_block`; the existing walker unit tests unchanged; verify new unit tests for loops, chains of 100 IFDs and oversized values
- [x] 3.2 Probe entry points `probe_buffer`, `needs_directory_reads`, `probe_with_directory_reads`, `probe_seekable` (used by `probe_path`), `tiff_metadata_block`; `CountingReader` seekable; verify `scenario_media_probe_tiff_directory_after_the_pixel_data`, `scenario_media_probe_directory_reads_are_bounded`, `scenario_media_probe_camera_raw_probes_read_only_the_prefix`, `scenario_media_probe_whole_buffer_and_targeted_reads_agree`, `scenario_media_probe_multipage_tiff_reports_its_first_page`, `scenario_media_probe_pyramid_tiff_reports_its_full_resolution`, and `cargo test -p ps-media --test media_probe` unchanged otherwise

## 4. Decoder

- [x] 4.1 `ps-media::tiff_decode` (level choice, file-order chunks, 32 MB chunk cap, sample conversion, accumulator, colour models, alpha), `DecodeError::TooLarge`, `decode_tiff_reader`; `decode_scaled` uses it for TIFF with the `image` fallback; verify `scenario_thumbnails_tiff_decoded_at_a_reduced_scale`, `scenario_thumbnails_reduced_resolution_level_preferred`, `scenario_thumbnails_tiff_variants_decode_to_the_same_picture`, `scenario_thumbnails_jpeg_compressed_ycbcr_tiff_decodes`, `scenario_thumbnails_orientation_tag_applied_to_tiff`, `scenario_thumbnails_alpha_composited_over_white`, `scenario_thumbnails_decode_memory_bounded`, `scenario_thumbnails_oversized_strip_refused_when_not_in_memory`
- [x] 4.2 Metadata from the compact block; verify `scenario_media_metadata_exif_of_an_end_directory_tiff_extracted`

## 5. Identity

- [x] 5.1 `ps-hash::IdentityStream` (full and sampled, `next_needed`, `finish`); verify `scenario_content_identity_incremental_identity_equals_one_shot_identity`

## 6. Scanner

- [x] 6.1 Pipeline: plain TIFFs that need directory reads are read in full before classification (<= 64 MB) and re-probed from the buffer, or probed with targeted reads through the lane reader (> 64 MB); verify `scenario_scanning_end_directory_tiff_read_once` and the existing `scenario_scanning_junk_costs_only_a_probe` and `scenario_scanning_one_full_read_per_candidate`
- [x] 6.2 One-pass reader (`tiff_pass`) for TIFFs above 64 MB: identity and decode from one sequential read, 64 MB budget, decoded image and metadata block in the CPU job, placeholder on decode errors, I/O errors and cancels reported as read failures; verify `scenario_scanning_large_tiff_analysed_in_one_pass`, `scenario_scanning_undecodable_large_tiff_keeps_its_identity`, `scenario_media_metadata_exif_of_a_tiff_larger_than_64_mb_extracted`, and the existing control tests
- [x] 6.3 Diff: unchanged thumbed TIFF rows without dimensions analysed again (`DiffCounts::reanalyzed`, shown in the CLI summary); verify `scenario_scanning_tiff_catalogued_without_dimensions_analysed_again`
- [x] 6.4 Grouping across formats; verify `scenario_duplicate_detection_tiff_copies_group_with_their_jpeg`

## 7. Previews

- [x] 7.1 `ps-service` previews probe with `probe_buffer`; the large preview of a TIFF above 64 MB decodes from the file in bounded memory; verify a service test that a large end-directory TIFF's preview is upright with the right aspect ratio

## 8. Budgets and docs

- [x] 8.1 `perf-budgets.toml` entries `tiff-analysis-decode` and `tiff-stream-memory` with their measure tests; verify `scenario_performance_budgets_tiff_analysis_decode_within_budget`, `scenario_performance_budgets_streamed_tiff_decode_memory_within_budget` and `cargo xtask bench --check` (record the measurements here). Measured (reference machine, `cargo xtask bench --check --update-baseline --only tiff-analysis-decode --only tiff-stream-memory`): `tiff-analysis-decode` 319.56 ms (bound 1000 ms), `tiff-stream-memory` 0.94 MB (bound 4 MB); both recorded in `perf-baseline.json`
- [x] 8.2 README formats section and `docs/performance.md` (TIFF read and memory behaviour); verify the README statements against the format-folder run

## 9. Verification

- [x] 9.1 Format folder before and after (fresh data dirs, release CLI with `heic`): per-file stage, dimensions, perceptual hash and group recorded here. Before (release 0.2.0 + heic): 22 files in 1.8 s, 2 groups (6 HEIC/AVIF/JPEG files; t05 and t14, byte-identical, as exact copies only), 1 error; t01, t03, t04, t06, t07, t09, t11, t12, t16 thumbed without dimensions (first IFD beyond the probe window); t08, t10, t17 (above 64 MB) without dimensions and perceptual hash; t13 (JPEG YCbCr pyramid) failed "Unhandled TIFF color type YCbCr(8)"; only t02 (IFD0 at offset 8) had dimensions. After: 22 files in 9.4 s (1.6 GB read), 0 errors, every file thumbed with its dimensions and perceptual hashes, one group of 20 (j00, the five HEIC/AVIF files and t01, t03-t14, t16). Not grouped, rightly: t02 (orientation 6: it shows the picture rotated, 2000x3000) and t17 (979 MB 14000x10000 scan of a different picture, aspect 1.4). The 979 MB t17 alone scanned in 8.0 s with a 25 MB peak resident size
- [x] 9.2 Corpus gate unchanged: medium corpus seed 42, precision/recall/keeper accuracy 1.0000, 0 must-not-merge violations, 1,843 keeper groups; RAW probe tests unchanged. After: 9,933 files scanned in 16.6 s, precision 1.0000, recall 1.0000, keeper accuracy 1.0000, 0 must-not-merge violations, 19,381 true pairs = result pairs = correct, 1,843 keeper groups; `groups list --json` totals 1,843 groups, 8,594 files, 4,048,886,433 reclaimable bytes, 5,570 suggestions (identical to before); the corpus has no TIFF (`scenario_test_corpus_corpus_presets_contain_no_tiff`), and the RAW probe tests and `scenario_media_probe_camera_raw_probes_read_only_the_prefix` pass
- [x] 9.3 `cargo xtask spec-coverage` maps every scenario of this change; `openspec validate tiff-support --strict`
- [x] 9.4 `cargo xtask ci` green in a clean worktree; pushed; GitHub CI green on all four jobs. Clean-worktree `cargo xtask ci` at f7c3f96 passed (31 specs valid, spec coverage 484/484); GitHub CI run 40124110743 for f7c3f96: Linux full CI, macos-14, windows-2022 and Performance budgets all green

## 10. Gate review fixes

- [x] 10.1 Decode work cap and stop check (ADR-0088, amends 0085/0086): 4 GiB of decoded bytes and 4 GiB of claimed compressed bytes per directory (shared blocks counted per chunk), checked from the header before any chunk; the largest reduced-resolution level within the cap stands in for a first image above it; `decode_with`/`decode_tiff_reader_with`/`decode_scaled_with`/`analyze_still_with` call a stop check before every strip or tile and return `DecodeError::Cancelled` (no `image` fallback); verify `scenario_thumbnails_tiff_decode_work_capped` (the reviewer's 81920x81920 shared-tile bomb: 0 chunks, < 64 KB read), `scenario_thumbnails_reduced_resolution_level_stands_in_for_a_huge_first_image`, `scenario_thumbnails_tiff_decode_stops_when_cancelled`
- [x] 10.2 Scanner wiring: the CPU worker passes `Busy::checkpoint` to `analyze_still_with` (a cancelled job is dropped, the file stays `probed`), the one-pass lane decode passes it to `decode_tiff_reader_with` (a cancel is a cancelled read); verify the timing-pass test `cancel_reaches_a_long_tiff_decode_within_250_ms` (a 16 MB file of 4 GiB shared tiles, exactly the cap) and that it fails without the check
- [x] 10.3 Overflow-free arithmetic: box factor at most 4104, per-axis box along an edge shorter than the factor, checked tile counts, `u64` averaging; verify `extreme_aspect_tiled_image_decodes` (33,554,432x1 in shared 1,048,576x16 tiles), `huge_box_factor_keeps_the_mean`, `tile_count_overflow_refused`
- [x] 10.4 Cross-byte bit reader for 1-7-bit samples, other non-byte depths refused as unsupported; verify `odd_bit_depths_decode_or_are_refused` (every depth 1-7 decodes exactly)
- [x] 10.5 Accumulator too large: the box factor grows until it fits; verify `scenario_thumbnails_large_preview_of_a_five_channel_tiff_decodes` (4095x4095 CMYK + alpha, 2048 px preview at factor 2, 2047x2047)
- [x] 10.6 `Tiff::metadata_block`: values above `wanted_value` (64 KB) skipped, one entry per tag, the block at most 1 MiB; verify `scenario_media_metadata_metadata_of_a_large_tiff_gathered_in_bounded_memory` (the 200 MB block of 1,000 shared 200 KB descriptions reproduced first)
- [x] 10.7 TIFF dimensions: `Tiff::uints` reads SBYTE/SSHORT/SLONG (non-negative) and LONG8/SLONG8/IFD8 (within u32); a TIFF whose probe still lacks dimensions gets the decoder's (`TiffDecodeStats::image_dims`, `tiff_decode::dimensions`), and one whose dimensions neither reads fails instead of being thumbed; verify `scenario_scanning_tiff_with_unusual_dimension_types_analysed_once` (LONG8 ImageWidth/ImageLength: reproduced thumbed without dimensions first; now 1600x1200 and a rescan reads 0 bytes) and the unit test `uints_read_every_integer_type`
- [x] 10.8 `IdentityStream::new(0)` needs no bytes; verify `scenario_content_identity_incremental_identity_of_an_empty_file`; `docs/performance.md` says `tiff-stream-memory` is the decoder's accounting, not RSS
- [x] 10.9 One-pass reader tests over sparse files in TMPDIR: verify `tiff_pass::tests::sampled_identity_of_a_sparse_300_mb_file`, `full_and_sampled_identity_at_the_256_mb_boundary`, `a_file_that_grows_or_shrinks_during_the_pass_fails`
- [x] 10.10 Delta specs (thumbnails, scanning, media-metadata, content-identity), ADR index, README limits, `heic-system-decoders` ADR numbering from 0089; verify `openspec validate tiff-support --strict` and `cargo xtask spec-coverage` (every tiff-support scenario mapped)
- [x] 10.11 Re-verification: format folder, corpus gate and the two TIFF budgets, results recorded here. Format folder (fresh data dir, release CLI with `heic`): 22 files in 9.4 s (1.6 GB read), 0 errors, every file thumbed with its dimensions and perceptual hashes, one group of 20 (j00, the five HEIC/AVIF files and t01, t03-t14, t16); t02 (orientation 6) and t17 (different picture) not grouped, as before. Corpus gate (medium, seed 42): 9,933 files in 16.5 s, precision 1.0000, recall 1.0000, keeper accuracy 1.0000, 0 must-not-merge violations, 19,381 true pairs = result pairs = correct, 1,843 keeper groups. `cargo xtask bench --check --only tiff-analysis-decode --only tiff-stream-memory`: 317.11 ms (bound 1000 ms, baseline 319.56 ms) and 0.94 MB (bound 4 MB), baseline unchanged
- [x] 10.12 `cargo xtask ci` green in a clean worktree; pushed; GitHub CI green on all four jobs (run 40208485377 on 5824890)
