# Design

## Context

ADR-0020 defines the provider layer, insight schema, queue and fixture testing; ADR-0021 makes AI advisory and
local by default with explicit hosted consent; ADR-0031 keys in the keychain (the `ps-service::secrets` wrapper
already exists from `app-shell`); ADR-0029 nothing else leaves the machine. tokio is allowed only for network
I/O in `ps-llm` (ADR-0009). Results feed XMP sidecars later (ADR-0022).

## Goals / Non-Goals

**Goals:**
- Every wire format tested offline with recorded fixtures; one opt-in live LM Studio test.
- A queue that cannot surprise the user: never starts by itself, never leaks keys, never changes decisions.

**Non-Goals:**
- Streaming token output, function calling, model management, local model embedding.

## Decisions

- **`ps-model::insight`.** `ImageInsight { schema_version: 1, subject, description, category: Category,
  adults: u8, children: u8, pets: Vec<Pet { kind, count }>, is_photo: { verdict: PhotoVerdict, confidence: f32 },
  tags: Vec<String> }`; `Category` = portrait, people, group, pets, animals, nature, landscape, travel,
  architecture, food, event, document, screenshot, other. A JSON Schema generated from the type is embedded in
  prompts (and passed as `response_format: json_schema` for OpenAI-compatible servers that support it, Gemini
  `responseSchema`, Anthropic via prompt + tool-free JSON).
- **`ps-llm`.** `trait VisionProvider { async fn describe(&self, img: &PreviewJpeg, prompt: &Prompt) ->
  Result<RawResponse>; async fn test(&self) -> TestReport; }`; impls `openai_compat`, `anthropic`, `gemini` on
  `reqwest` (rustls) inside a dedicated tokio runtime owned by `ps-llm` (ADR-0009). `validate::parse_insight`
  with one repair round (the repair prompt includes the validation error). Retry policy: 429 -> retry-after or
  backoff from 2 s; 5xx/timeouts -> backoff 2 s, 8 s; max 2 retries (spec). Timeouts: connect 5 s, request 120 s
  (local vision models are slow).
- **Recorded fixtures.** `ps-llm/tests/fixtures/<provider>/*.json` request/response pairs served by a tiny
  in-process HTTP server (std `TcpListener`, test-only) that asserts request shape (with image bytes replaced
  by a hash) and replays the response.
- **Preview source.** ADR-0020 says "1024 px JPEG preview from the thumbnail pipeline"; ADR-0016 has only 384 px
  thumbnails and 2048 px previews. We downscale the 2048 px preview (generating it from the source if needed) to
  1024 px, quality 85, in memory. If the source is offline and no 2048 px preview is cached, the item is
  deferred ("source offline"), not failed.
- **Queue (migration 5).** `ai_queue(content_id PK, priority, state, attempts, last_error, enqueued_at)`,
  `ai_result(content_id, provider, model, schema_version, insight JSON, latency_ms, at, PRIMARY KEY(content_id,
  provider, model, schema_version))`, `ai_correction(content_id, field, value JSON, at)`. Runner in
  `ps-service::ai` with a semaphore for concurrency and a token bucket for requests/min; state transitions go
  through the catalog writer (ADR-0005). At start-up a `running` queue becomes `paused`.
- **Effective insight.** `ps-service::ai::effective(content_id)` = latest result for the active provider/model
  overlaid with user corrections; used by filters, inspector and export.
- **Advisory boundary.** `ps-dedup` and the export planner do not depend on AI tables; a test asserts groups and
  keepers are byte-identical before and after an AI run (ADR-0021). Suggestions: `is_photo.verdict` of
  screenshot/document/graphic with confidence >= 0.7 creates an `ai_suggestion` row; nothing else.
- **UI.** `ui-workspace::views::ai_review` (live list fed by a 500 ms poll of new results via snapshot
  counters, ADR-0019), AI section in the inspector, provider settings section with consent dialog and key entry.
- **CLI.** `photosquirrel-cli ai providers|test|start|pause|status|results` on the ADR-0027 conventions.

## Risks / Trade-offs

- [Small local models return sloppy JSON] -> schema in prompt, one repair round, failures visible.
- [Counting children is unreliable] -> labelled as AI estimates; corrections easy.
- [Hosted costs] -> default rate limit 30/min for hosted providers; queue shows count before start.

## Verification

- `cargo test -p ps-llm` (recorded fixtures for all three providers; all `scenario_llm_providers_*` except live).
- `PHOTOSQUIRREL_LIVE_LLM=1 cargo test -p ps-llm --test live -- --nocapture` with LM Studio running.
- `cargo test -p ps-service ai`, `cargo test -p ui-workspace ai_review`, `cargo test -p photosquirrel-cli ai`.
- `cargo xtask screenshot --all-surfaces` (AI review, consent dialog), `cargo xtask ci`,
  `openspec validate ai-insights --strict`.
