# Tasks

## 1. Schema and providers

- [ ] 1.1 `ImageInsight` v1 types and generated JSON Schema in `ps-model`; verify a unit test that the schema validates a sample and rejects a missing `category`
- [ ] 1.2 Recorded-fixture HTTP test server in `ps-llm` tests; verify it replays a fixture and fails on a shape mismatch
- [ ] 1.3 OpenAI-compatible provider; verify `scenario_llm_providers_openai_compatible_request_shape` and `scenario_llm_providers_default_is_lm_studio`
- [ ] 1.4 Anthropic provider; verify `scenario_llm_providers_anthropic_request_shape`
- [ ] 1.5 Gemini provider; verify `scenario_llm_providers_gemini_request_shape`
- [ ] 1.6 1024 px preview derivation with offline deferral; verify `scenario_llm_providers_original_never_uploaded`
- [ ] 1.7 Validation and one repair round; verify `scenario_llm_providers_valid_insight_parsed`, `scenario_llm_providers_invalid_response_repaired_once`, `scenario_llm_providers_second_failure_recorded`
- [ ] 1.8 Connection test; verify `scenario_llm_providers_test_unreachable_provider`
- [ ] 1.9 Rate-limit handling; verify `scenario_llm_providers_rate_limit_backs_off`
- [ ] 1.10 Keychain keys with env override and log redaction; verify `scenario_llm_providers_key_not_written_to_disk` and `scenario_llm_providers_env_var_overrides_key`
- [ ] 1.11 Hosted consent gate; verify `scenario_llm_providers_hosted_provider_needs_consent`

## 2. Queue

- [ ] 2.1 Migration 5 (ai_queue, ai_result, ai_correction, ai_suggestion); verify migration upgrade test in `ps-catalog`
- [ ] 2.2 Queue runner with explicit start and pause; verify `scenario_ai_queue_nothing_sent_before_start` and `scenario_ai_queue_pause_stops_new_requests`
- [ ] 2.3 Ordering and keeper-first priority; verify `scenario_ai_queue_keepers_processed_first`
- [ ] 2.4 Concurrency semaphore and token bucket; verify `scenario_ai_queue_concurrency_limit_respected`
- [ ] 2.5 Restart and crash survival (child-process kill test); verify `scenario_ai_queue_queue_survives_restart`
- [ ] 2.6 Per-content results and provenance; verify `scenario_ai_queue_duplicates_analyzed_once` and `scenario_ai_queue_result_records_provenance`
- [ ] 2.7 Retries; verify `scenario_ai_queue_failed_item_retried_twice`
- [ ] 2.8 Progress snapshot and CLI `ai` commands; verify `scenario_ai_queue_queue_progress_reported` and `scenario_ai_queue_cli_starts_queue`

## 3. Review UI

- [ ] 3.1 AI Review view with live updates; verify `scenario_ai_review_results_appear_live`
- [ ] 3.2 Inspector AI section; verify `scenario_ai_review_insight_details_shown`
- [ ] 3.3 Insight filters in gallery and review view; verify `scenario_ai_review_filter_contains_children`
- [ ] 3.4 Advisory boundary test and suggestions; verify `scenario_ai_review_ai_never_changes_keeper` and `scenario_ai_review_non_photo_verdict_is_a_suggestion`
- [ ] 3.5 Bulk accept/reject; verify `scenario_ai_review_accept_exclusion_suggestion`
- [ ] 3.6 User corrections and effective insight; verify `scenario_ai_review_user_correction_wins`
- [ ] 3.7 Provider settings section, consent dialog and key entry; verify screenshot review via `cargo xtask screenshot --all-surfaces`

## 4. Live smoke

- [ ] 4.1 Opt-in live test; verify `scenario_llm_providers_live_lm_studio_smoke` passes with `PHOTOSQUIRREL_LIVE_LLM=1` and LM Studio running, and is skipped otherwise

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [ ] 5.2 Gate: recorded-fixture tests pass for all three providers and the live LM Studio smoke passes once on the reference machine
- [ ] 5.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 5.4 `openspec validate ai-insights --strict` passes
