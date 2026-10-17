# 0020. Own LLM vision provider layer; LM Studio default

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The user wants candidate photos described by a configurable LLM: subject, category (portrait, people, pets, nature, travel...), people count with adults vs. children, pets. Default is LM Studio locally; the major hosted providers must work via settings.

## Decision
`ps-llm` defines a small `VisionProvider` trait with three wire implementations: **OpenAI-compatible chat completions** (LM Studio default at `http://localhost:1234/v1`, Ollama, OpenAI, OpenRouter, Groq, any compatible server), **Anthropic Messages**, and **Google Gemini**. Requests send a 1024 px JPEG preview from the thumbnail pipeline (never the original) and ask for JSON matching a versioned schema (`ImageInsight v1`); responses are validated and repaired once on schema errors. A persisted **AI queue** in the catalog processes photos (`photo`/`likely_photo` keepers first) with configurable concurrency and rate limits; it pauses, resumes and survives restarts. Results are stored with provider, model, schema version and latency, and are visible in the review panel as they arrive. API keys live in the OS keychain (ADR-0031).

## Alternatives considered
- **Generic multi-provider SDK crates** - large dependency trees, vision support uneven.
- **Embedding a local model** - heavy; LM Studio already manages local models well.

## Consequences
- Recorded fixtures test every provider wire format without network.

## Revisit when
A provider becomes impossible to express with these three wire formats.
