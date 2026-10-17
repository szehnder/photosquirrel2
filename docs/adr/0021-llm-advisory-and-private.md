# 0021. LLM output is advisory; local by default

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
LLM answers can be wrong, and sending family photos to a cloud service is a privacy decision.

## Decision
AI insights never decide grouping, keeper choice, export or cleanup by themselves; they only add labels, filters and review suggestions (e.g. "contains children" filter, "screenshot - probably not a photo" suggestion the user accepts). The default provider is local (LM Studio). Choosing a hosted provider requires an explicit consent toggle that names the provider and states images will be uploaded. Nothing is sent anywhere until the user starts the AI queue.

## Alternatives considered
- **Auto-exclude images the LLM calls non-photos** - risk of silently dropping real photos.

## Consequences
- Safety of the dedup/export path is independent of model quality.

## Revisit when
Never relaxed for destructive or exclusion decisions.
