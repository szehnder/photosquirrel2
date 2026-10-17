# 0003. Layered Cargo workspace

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The engine (scanning, hashing, dedup, catalog, export) must be testable and benchmarkable without a window, and reusable from a CLI.

## Decision
Crates are layered and the layering is enforced by `cargo xtask layering`:
- **core** (pure, no tokio, no gpui-kit): `ps-model`, `ps-media`, `ps-hash`, `ps-dedup`, `ps-fixtures`.
- **I/O** (may use threads, SQLite, tokio for network; no gpui-kit): `ps-catalog`, `ps-scan`, `ps-llm`, `ps-export`, `ps-service`, `photosquirrel-cli`.
- **UI** (gpui-kit): `ui-theme`, `ui-gallery`, `ui-workspace`, `photosquirrel` (binary).
Dependencies only point downward (UI -> I/O -> core). `ps-service` is the single facade the UI and CLI use (ADR-0027).

## Alternatives considered
- **One big crate** - slow incremental builds, no enforced boundaries.

## Consequences
- Every crate must be classified in `xtask/src/layering.rs`.

## Revisit when
A layer needs to call upward; that signals a missing abstraction, not a reason to break layering.
