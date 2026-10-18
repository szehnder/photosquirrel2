# Tasks

## 1. Building blocks

- [x] 1.1 Seeded RNG plumbing and the output-writer trait in `ps-fixtures`; verify a unit test that two writers fed the same seed receive identical byte streams
- [x] 1.2 Procedural image synthesis and camera EXIF writer; verify `scenario_test_corpus_originals_classify_as_photos` (uses the `media-analysis` classifier)
- [x] 1.3 Minimal DNG and ISO-BMFF video writers; verify unit tests that `ps-media` probes their dimensions, duration and make/model

## 2. Corpus content

- [x] 2.1 Derivative generation; verify `scenario_test_corpus_every_derivative_kind_present`
- [x] 2.2 Must-not-merge sets; verify `scenario_test_corpus_must_not_merge_sets_recorded`
- [x] 2.3 Junk and cache files; verify `scenario_test_corpus_junk_has_expected_classes`
- [x] 2.4 Drive roots and nested backup layout; verify `scenario_test_corpus_copies_spread_across_drives`

## 3. Truth and determinism

- [x] 3.1 `truth.json` model (truth_version 1) and writer; verify `scenario_test_corpus_truth_lists_every_file` and `scenario_test_corpus_each_group_has_one_keeper`
- [x] 3.2 Fixed mtimes and sorted output; verify `scenario_test_corpus_same_seed_same_corpus` and `scenario_test_corpus_different_seed_differs`

## 4. Evaluator

- [x] 4.1 Implement `ps-fixtures::eval::Score`; verify `scenario_test_corpus_perfect_grouping_scores_one` and `scenario_test_corpus_false_merge_lowers_precision`

## 5. CLI

- [x] 5.1 Add the clap skeleton and `fixtures generate` to `photosquirrel-cli`; verify `scenario_test_corpus_cli_generates_corpus` and `scenario_test_corpus_refuses_non_empty_output`
- [x] 5.2 Presets small/medium/large; verify `scenario_test_corpus_small_preset_is_quick` (release test printing `measure corpus-small-generate=<s> s`)

## 6. Verification

- [ ] 6.1 `cargo xtask ci` is green
- [x] 6.2 Gate: `cargo run -p photosquirrel-cli -- fixtures generate --seed 42 --preset small --out <tmp>` writes the corpus and `truth.json`
- [x] 6.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 6.4 `openspec validate ground-truth-corpus --strict` passes
