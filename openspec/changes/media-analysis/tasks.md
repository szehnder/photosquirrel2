# Tasks

## 1. Model types and fixtures

- [x] 1.1 Add media types (`MediaFormat`, `MediaKind`, `Dimensions`, `Orientation`, `ProbeRecord`, `Metadata`, `CaptureDate`, `PhotoClass`, `ClassReason`, `ClassifierSettings` with ADR-0015 defaults) to `ps-model`; verify `cargo test -p ps-model` (serde round-trip unit tests)
- [x] 1.2 Add `ps-fixtures::media` generators for orientation JPEGs, a DNG with embedded preview, MOV/MP4 with QuickTime metadata, palette PNG, ICO, truncated/corrupt files and quality-60/92 encodes; verify a unit test that every generator output is non-empty and deterministic for a seed

## 2. Probe

- [x] 2.1 Implement magic sniffing and header dimensions in `ps-media::probe`; verify `scenario_media_probe_jpeg_with_wrong_extension` and `scenario_media_probe_unknown_bytes_are_unsupported`
- [x] 2.2 Enforce the 256 KB bound with a counting reader; verify `scenario_media_probe_probe_reads_at_most_256_kb`
- [x] 2.3 Orientation and display dimensions; verify `scenario_media_probe_rotated_orientation_swaps_dimensions`
- [x] 2.4 Embedded preview location for EXIF/RAW/HEIC; verify `scenario_media_probe_embedded_preview_found_in_raw` and `scenario_media_probe_no_preview_reported_as_absent`
- [x] 2.5 Read-only open helper (the only way `ps-media` callers open files in tests); verify `scenario_media_probe_probe_never_opens_for_writing`
- [x] 2.6 Error records and a proptest/fuzz harness over mutated headers; verify `scenario_media_probe_truncated_jpeg_reports_error` and `scenario_media_probe_fuzzed_headers_never_panic`
- [x] 2.7 `ps-media::capabilities()` and HEIC-off probing; verify `scenario_media_probe_heic_without_decoder` and `scenario_media_probe_availability_listed` (run with `--no-default-features`)

## 3. Metadata

- [x] 3.1 EXIF extraction including GPS conversion; verify `scenario_media_metadata_camera_exif_extracted` and `scenario_media_metadata_gps_converted_to_decimal_degrees`
- [x] 3.2 QuickTime/MP4 metadata; verify `scenario_media_metadata_phone_video_metadata`
- [x] 3.3 Capture-date resolution with provenance; verify `scenario_media_metadata_fallback_to_file_time_flagged` and `scenario_media_metadata_datetimeoriginal_preferred`
- [x] 3.4 JPEG quality estimation from DQT tables; verify `scenario_media_metadata_quality_estimate_within_tolerance`
- [x] 3.5 Camera-original signal; verify `scenario_media_metadata_maker_notes_mark_camera_original`
- [x] 3.6 Full tag list and corrupt-block tolerance; verify `scenario_media_metadata_all_tags_listed` and `scenario_media_metadata_corrupt_exif_keeps_file`

## 4. Classifier

- [x] 4.1 Implement `ps-media::classify` with the ADR-0015 rule order; verify `scenario_photo_classification_class_and_reason_assigned` and `scenario_photo_classification_classification_is_deterministic`
- [x] 4.2 Camera EXIF and size rules; verify `scenario_photo_classification_camera_exif_wins_over_size`, `scenario_photo_classification_tiny_image_is_junk`, `scenario_photo_classification_boundary_size_is_kept`, `scenario_photo_classification_custom_threshold_applied`
- [x] 4.3 Cache folders and graphics; verify `scenario_photo_classification_lightroom_previews_are_cache`, `scenario_photo_classification_thumbs_db_neighbour_is_cache`, `scenario_photo_classification_palette_png_is_graphic`, `scenario_photo_classification_ico_is_graphic`
- [x] 4.4 Video rules and fallback; verify `scenario_photo_classification_phone_video_is_photo`, `scenario_photo_classification_short_clip_is_junk`, `scenario_photo_classification_plain_large_jpeg_is_likely_photo`

## 5. Decode and thumbnails

- [x] 5.1 DCT-scaled analysis decode with orientation; verify `scenario_thumbnails_dct_scaled_decode_chosen` and `scenario_thumbnails_small_image_decoded_at_full_size`
- [x] 5.2 Grid thumbnail encode and dominant colour; verify `scenario_thumbnails_thumbnail_long_edge_is_384` and `scenario_thumbnails_orientation_applied_to_thumbnail`
- [x] 5.3 Embedded-preview path (RAW/EXIF/HEIC); verify `scenario_thumbnails_embedded_preview_preferred` and `scenario_thumbnails_small_preview_not_used`
- [x] 5.4 `ps-catalog::thumbs` store (`thumbs.db`); verify `scenario_thumbnails_duplicates_share_one_thumbnail` and `scenario_thumbnails_thumbnail_available_offline`
- [x] 5.5 2048 px preview LRU with cap; verify `scenario_thumbnails_preview_cache_stays_under_cap`
- [x] 5.6 Poster frames and placeholders; verify `scenario_thumbnails_video_placeholder_without_tool` and `scenario_thumbnails_poster_frame_from_configured_tool` (fake ffmpeg script)

## 6. Benchmarks

- [x] 6.1 Add criterion bench `ps-media/benches/analyze.rs` (probe, analysis decode, thumbnail on 12 MP JPEGs) and a `perf-budgets.toml` entry `analysis-decode-thumb` (max 100 ms, derived from ADR-0026); verify `scenario_thumbnails_decode_within_per_file_budget` prints a `measure` line and `cargo xtask bench --list` shows the entry

## 7. Verification

- [ ] 7.1 `cargo xtask ci` is green
- [x] 7.2 `cargo test -p ps-media --no-default-features` passes (HEIC off)
- [x] 7.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 7.4 `openspec validate media-analysis --strict` passes
