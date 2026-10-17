# Proposal

## Why

The user wants each photo described: the subject, a category (portrait, people, pets, nature, travel...), how
many people with adults versus children, and which pets, so a lifetime of photos becomes searchable and the
export carries useful keywords. It must run locally by default (LM Studio), work with hosted providers when the
user explicitly opts in, survive restarts, and be reviewable live while running, without ever influencing what
is grouped, kept or exported.

## What Changes

- `VisionProvider` layer with three wire formats: OpenAI-compatible chat completions (LM Studio default at
  `http://localhost:1234/v1`, Ollama, OpenAI, OpenRouter, Groq), Anthropic Messages and Google Gemini (ADR-0020).
- Requests carry a 1024 px JPEG preview only; responses validated against the versioned `ImageInsight v1`
  schema with one repair attempt.
- Provider settings, "Test connection", API keys in the OS keychain with env-var override, hosted-provider
  consent naming the provider (ADR-0021, ADR-0031).
- Persisted AI queue in the catalog: keepers of class `photo`/`likely_photo` first, configurable concurrency and
  rate limit, pause/resume, survives restarts, retries with backoff, one result per content identity.
- AI Review view: results streaming in live, filters (category, contains children, contains pets, people count),
  user corrections, advisory suggestions (e.g. "probably a screenshot") the user may accept.
- `photosquirrel-cli ai` subcommands (`providers`, `test`, `start`, `pause`, `status`, `results`).

## Capabilities

### New Capabilities
- `llm-providers`: talking to local and hosted vision models through three wire formats, safely and privately.
- `ai-queue`: the persisted, resumable background queue that sends photos for description and stores results.
- `ai-review`: reviewing, filtering and correcting AI insights in the application.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0009, 0016, 0019, 0020, 0021, 0026, 0027, 0029, 0031.
- Crates: `ps-model` (`ImageInsight` v1, provider config), `ps-llm` (providers, schema validation, recorded
  fixtures), `ps-catalog` (migration: ai_queue, ai_result, ai_correction), `ps-service` (AI queue API),
  `ui-workspace` (AI Review view, provider settings section), `photosquirrel-cli` (`ai` commands).

## Out of scope

- Embedded local models; face recognition/clustering (later `face-clusters`); embeddings for similar shots.
- Any automatic exclusion, grouping or keeper change based on AI output (ADR-0021).
- Video analysis by the LLM (stills and video poster frames only when available; v1 sends stills only).
