# Tasks

## 1. Content identity

- [ ] 1.1 Add `ContentId`, `IdentityKind` and size constants to `ps-model`; verify `cargo test -p ps-model` (hex round-trip unit test)
- [ ] 1.2 Implement full identity from buffer and from reader in `ps-hash::identity`; verify `scenario_content_identity_full_hash_matches_reference`, `scenario_content_identity_boundary_size_hashed_fully`, `scenario_content_identity_renamed_copy_has_same_identity`
- [ ] 1.3 Implement sampled identity; verify `scenario_content_identity_large_video_sampled`, `scenario_content_identity_middle_edit_changes_sampled_identity`, `scenario_content_identity_size_is_part_of_sampled_identity` (sparse temp files)
- [ ] 1.4 Single-read integration with `ps-media::decode` (shared buffer API) and streaming above 64 MB; verify `scenario_content_identity_one_read_per_file` and `scenario_content_identity_large_file_streamed`
- [ ] 1.5 Determinism check across thread counts; verify `scenario_content_identity_hash_is_stable_across_runs`

## 2. Perceptual hashes

- [ ] 2.1 Implement luminance 32x32, pHash and dHash with `HashVersion(1)` and golden values; verify `scenario_perceptual_hashing_known_image_has_known_hashes` and `scenario_perceptual_hashing_orientation_tag_matches_rotated_pixels`
- [ ] 2.2 Robustness against resize/recompress; verify `scenario_perceptual_hashing_resized_copy_stays_close` and `scenario_perceptual_hashing_different_photos_far_apart`
- [ ] 2.3 Versioned comparison API; verify `scenario_perceptual_hashing_version_mismatch_not_compared`
- [ ] 2.4 Proptest invariants (distance symmetry, distance(h, h) = 0, triangle inequality); verify `cargo test -p ps-hash` proptests

## 3. Near-duplicate index

- [ ] 3.1 Implement the MIH index with 1-bit neighbour probing and dual-hash verification; verify `scenario_perceptual_hashing_index_recall_at_distance_7` and `scenario_perceptual_hashing_only_verified_candidates_returned`

## 4. Benchmarks and budgets

- [ ] 4.1 Criterion bench `ps-hash/benches/hashing.rs` and `perf-budgets.toml` entry `phash-dhash-per-image` (0.3 ms); verify `scenario_perceptual_hashing_hash_cost_within_budget`
- [ ] 4.2 Lookup and build benches with `perf-budgets.toml` entries `near-dup-lookup-p95` (1 ms) and `near-dup-index-build` (1 s); verify `scenario_perceptual_hashing_lookup_within_budget` and `scenario_perceptual_hashing_index_build_under_one_second`
- [ ] 4.3 Record measurements; verify `cargo xtask bench --check` passes for the three hashing budgets

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [ ] 5.2 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 5.3 `openspec validate content-hashing --strict` passes
