# Tasks

## 1. ADR and schema

- [x] 1.1 ADR-0033 "User separation of group members" and ADR-0035 "Burst protection" accepted (verify: listed in `docs/adr/README.md`)
- [ ] 1.2 Catalog migration 2 (groups, members, suggestions, overrides, separations); verify `cargo test -p ps-catalog` migration test upgrades a v1 catalog

## 2. Edges

- [ ] 2.1 Exact and sampled edges; verify `scenario_duplicate_detection_exact_copies_grouped` and `scenario_duplicate_detection_sampled_match_needs_metadata`
- [ ] 2.2 Derivative rule with aspect and metadata checks; verify `scenario_duplicate_detection_resized_thumbnail_joins_group`, `scenario_duplicate_detection_crop_not_merged`, `scenario_duplicate_detection_threshold_boundary_respected`
- [ ] 2.3 Suggestions for metadata contradictions; verify `scenario_duplicate_detection_shots_5_s_apart_only_suggested` and `scenario_duplicate_detection_different_camera_not_merged`

## 3. Grouping

- [ ] 3.1 Union-find with explanations; verify `scenario_duplicate_detection_membership_reason_recorded`
- [ ] 3.2 Chain-merge guard; verify `scenario_duplicate_detection_chain_spanning_60_s_split`
- [ ] 3.3 Incremental regrouping in `ps-scan::post`; verify `scenario_duplicate_detection_incremental_equals_full_regroup`
- [ ] 3.4 Separation constraints (ADR-0033); verify `scenario_duplicate_detection_separated_member_stays_apart`

## 4. Ranking

- [ ] 4.1 Lexicographic comparator with reasons; verify `scenario_best_copy_camera_original_beats_png_export`, `scenario_best_copy_larger_pixel_count_wins`, `scenario_best_copy_metadata_breaks_a_tie`, `scenario_best_copy_shallower_path_breaks_final_tie`, `scenario_best_copy_keeper_reasons_listed`
- [ ] 4.2 Upscale detection; verify `scenario_best_copy_upscale_does_not_win`
- [ ] 4.3 RAW+JPEG keeper sets; verify `scenario_best_copy_raw_and_jpeg_kept_together`
- [ ] 4.4 Cache-class rule; verify `scenario_best_copy_cache_copy_never_keeper`
- [ ] 4.5 Total order proptest; verify `scenario_best_copy_ranking_ignores_input_order`
- [ ] 4.6 Persisted overrides via `ps-service::groups`; verify `scenario_best_copy_keeper_override_persists` and `scenario_best_copy_override_survives_rescan`

## 5. Corpus gate

- [ ] 5.1 Corpus gate test for grouping; verify `scenario_duplicate_detection_corpus_grouping_gate`
- [ ] 5.2 Corpus gate test for keepers; verify `scenario_best_copy_corpus_keeper_gate`
- [ ] 5.3 Burst subset of the corpus passes with ADR-0035 rules; verify `scenario_duplicate_detection_same_second_burst_only_suggested` and `scenario_duplicate_detection_two_camera_originals_not_merged`

## 6. Verification

- [ ] 6.1 `cargo xtask ci` is green
- [ ] 6.2 Gate: precision >= 0.995, recall >= 0.97, keeper accuracy >= 0.99 on the medium corpus (`cargo test -p ps-service --test corpus_gate --release`)
- [ ] 6.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 6.4 `openspec validate duplicate-groups --strict` passes
