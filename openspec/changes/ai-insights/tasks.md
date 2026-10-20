# Tasks

## 1. Schema and providers

- [x] 1.1 `ImageInsight` v1 types and generated JSON Schema in `ps-model`; verify a unit test that the schema validates a sample and rejects a missing `category`
- [x] 1.2 Recorded-fixture HTTP test server in `ps-llm` tests; verify it replays a fixture and fails on a shape mismatch
- [x] 1.3 OpenAI-compatible provider; verify `scenario_llm_providers_openai_compatible_request_shape` and `scenario_llm_providers_default_is_lm_studio`
- [x] 1.4 Anthropic provider; verify `scenario_llm_providers_anthropic_request_shape`
- [x] 1.5 Gemini provider; verify `scenario_llm_providers_gemini_request_shape`
- [x] 1.6 1024 px preview derivation with offline deferral; verify `scenario_llm_providers_original_never_uploaded`
- [x] 1.7 Validation and one repair round; verify `scenario_llm_providers_valid_insight_parsed`, `scenario_llm_providers_invalid_response_repaired_once`, `scenario_llm_providers_second_failure_recorded`
- [x] 1.8 Connection test; verify `scenario_llm_providers_test_unreachable_provider`
- [x] 1.9 Rate-limit handling; verify `scenario_llm_providers_rate_limit_backs_off`
- [x] 1.10 Keychain keys with env override and log redaction; verify `scenario_llm_providers_key_not_written_to_disk` and `scenario_llm_providers_env_var_overrides_key`
- [x] 1.11 Hosted consent gate; verify `scenario_llm_providers_hosted_provider_needs_consent`

## 2. Queue

- [x] 2.1 Migration 5 (ai_queue, ai_result, ai_correction, ai_suggestion); verify migration upgrade test in `ps-catalog` (registered after migration 0004; `ai_insights_migration_applies_on_latest_schema` upgrades a v4 catalog; store `ps_service::ai::CatalogQueueStore` over `ps_catalog::stores`)
- [x] 2.2 Queue runner with explicit start and pause; verify `scenario_ai_queue_nothing_sent_before_start` and `scenario_ai_queue_pause_stops_new_requests` (engine `ps_llm::queue`, MemoryStore; catalog-backed store deferred to integration)
- [x] 2.3 Ordering and keeper-first priority; verify `scenario_ai_queue_keepers_processed_first` (`ps_service::ai::enqueue`/`queue_priority` over stored group keepers; test in `ps-service/tests/ai_queue.rs`)
- [x] 2.4 Concurrency semaphore and token bucket; verify `scenario_ai_queue_concurrency_limit_respected`
- [x] 2.5 Restart and crash survival (child-process kill test); verify `scenario_ai_queue_queue_survives_restart` (child process killed after 10 of 50; restart shows 10 done, paused, resuming sends 40)
- [x] 2.6 Per-content results and provenance; verify `scenario_ai_queue_duplicates_analyzed_once` and `scenario_ai_queue_result_records_provenance`
- [x] 2.7 Retries; verify `scenario_ai_queue_failed_item_retried_twice`
- [x] 2.8 Progress snapshot and CLI `ai` commands; verify `scenario_ai_queue_queue_progress_reported` and `scenario_ai_queue_cli_starts_queue` (deferred to integration: progress snapshot done in `AiQueue::progress`; CLI owned by headless-cli) (`ai start|pause|status|results|providers|test` in photosquirrel-cli; `scenario_ai_queue_queue_progress_reported`, `scenario_ai_queue_cli_starts_queue`)

## 3. Review UI

- [x] 3.1 AI Review view with live updates; verify `scenario_ai_review_results_appear_live` (`ui-workspace::views::ai_review`, 500 ms stamp poll)
- [ ] 3.2 Inspector AI section; verify `scenario_ai_review_insight_details_shown` (details shown in the AI Review detail pane and the scenario passes there; the gallery inspector's AI section is still to do)
- [ ] 3.3 Insight filters in gallery and review view; verify `scenario_ai_review_filter_contains_children` (review view done and the scenario passes; gallery filter bar still to do)
- [x] 3.4 Advisory boundary test and suggestions; verify `scenario_ai_review_ai_never_changes_keeper` and `scenario_ai_review_non_photo_verdict_is_a_suggestion`
- [x] 3.5 Bulk accept/reject; verify `scenario_ai_review_accept_exclusion_suggestion`
- [x] 3.6 User corrections and effective insight; verify `scenario_ai_review_user_correction_wins` (corrections in an interim `ai-review.json` keyed by content until the catalog exposes `ai_correction` writes)
- [x] 3.7 Provider settings section, consent dialog and key entry; verify screenshot review via `cargo xtask screenshot --all-surfaces` (`settings_ai.rs`, `tests/ai_settings.rs`, `docs/screenshots/ai-settings-dark.png`; request timeout row added)

## 4. Live smoke

- [ ] 4.1 Opt-in live test; verify `scenario_llm_providers_live_lm_studio_smoke` passes with `PHOTOSQUIRREL_LIVE_LLM=1` and LM Studio running, and is skipped otherwise (skip path verified; live run pending: LM Studio server off, no model loaded)

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [ ] 5.2 Gate: recorded-fixture tests pass for all three providers and the live LM Studio smoke passes once on the reference machine
- [ ] 5.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 5.4 `openspec validate ai-insights --strict` passes
