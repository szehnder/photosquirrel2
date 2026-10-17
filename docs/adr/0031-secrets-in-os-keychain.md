# 0031. API keys in the OS keychain

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Hosted LLM providers need API keys; storing them in plain settings files is unsafe.

## Decision
Provider API keys are stored with the `keyring` crate (macOS Keychain, Windows Credential Manager, Secret Service on Linux). If no keychain is available, keys are kept for the session only and the UI says so. Environment variables (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`) are honoured as an override.

## Alternatives considered
- **Encrypted file with a master password** - more UI, little gain.

## Consequences
- Settings files never contain secrets.

## Revisit when
Never.
