# 0025. Testing strategy and a synthetic ground-truth corpus

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Dedup and best-copy logic is safety-critical and must be measured, not eyeballed. Real family photos cannot be committed to a public repo.

## Decision
- Unit tests per crate; property tests (`proptest`) for hashing, ranking and diffing invariants.
- `ps-fixtures` generates a deterministic **ground-truth corpus** from a seed: procedural "photos" with camera EXIF, plus derivatives (exact copies, renames, resized thumbnails, recompressions, EXIF-stripped copies, crops that must *not* merge, burst-like near-shots that must *not* merge), junk (icons, UI graphics, cache thumbnails) and nested backup-of-backup folder trees; a `truth.json` lists the true groups and the expected keeper. Gates: grouping precision >= 0.995, recall >= 0.97, keeper accuracy >= 0.99, zero merges across distinct originals in the "must not merge" set.
- Integration tests run full scans, cancel/resume, rescans and exports on generated corpora in temp dirs.
- UI tests use headless `#[gpui_kit::test]` windows; frame-time benchmarks render the gallery with synthetic data.
- LLM providers are tested with recorded HTTP fixtures; live tests opt-in via `PHOTOSQUIRREL_LIVE_LLM=1`.
- Each OpenSpec `#### Scenario:` maps to a test named `scenario_<capability>_<snake_slug>` (`cargo xtask spec-coverage`).

## Alternatives considered
- **Manual testing on the author's drives** - irreproducible; used only as a final smoke test.

## Consequences
- `cargo xtask ci` = fmt, clippy -D warnings, layering, tokens, tests, openspec validate, spec coverage.

## Revisit when
Test time exceeds 5 minutes locally.
