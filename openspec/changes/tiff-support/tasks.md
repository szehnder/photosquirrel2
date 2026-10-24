# Tasks

## 1. Decisions

- [ ] 1.1 ADR-0084 (TIFF directories beyond the probe window), ADR-0085 (strip-wise TIFF decoding), ADR-0086 (one-pass analysis of TIFFs above 64 MB) and ADR-0087 (TIFF decode budgets) accepted and listed in `docs/adr/README.md` with their amends notes; roadmap row added; verify `openspec validate tiff-support --strict`

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

- [ ] 8.1 `perf-budgets.toml` entries `tiff-analysis-decode` and `tiff-stream-memory` with their measure tests; verify `scenario_performance_budgets_tiff_analysis_decode_within_budget`, `scenario_performance_budgets_streamed_tiff_decode_memory_within_budget` and `cargo xtask bench --check` (record the measurements here)
- [ ] 8.2 README formats section and `docs/performance.md` (TIFF read and memory behaviour); verify the README statements against the format-folder run

## 9. Verification

- [ ] 9.1 Format folder before and after (fresh data dirs, release CLI with `heic`): per-file stage, dimensions, perceptual hash and group recorded here
- [ ] 9.2 Corpus gate unchanged: medium corpus seed 42, precision/recall/keeper accuracy 1.0000, 0 must-not-merge violations, 1,843 keeper groups; RAW probe tests unchanged
- [ ] 9.3 `cargo xtask spec-coverage` maps every scenario of this change; `openspec validate tiff-support --strict`
- [ ] 9.4 `cargo xtask ci` green in a clean worktree; pushed; GitHub CI green on all four jobs
