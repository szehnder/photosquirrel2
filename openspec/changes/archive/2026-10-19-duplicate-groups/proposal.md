# Proposal

## Why

The core promise: one best copy of every photo out of many overlapping backups. Exact copies and resized,
recompressed or thumbnail versions of the same shot must collapse into one group with an explainable keeper,
while bursts and similar shots must never be collapsed. A false merge could hide a unique photo, so precision is
gated on the ground-truth corpus before any UI or export is built.

## What Changes

- Exact edges from identical full identities; sampled identities only with matching size, duration and
  dimensions (ADR-0011).
- Derivative edges: pHash <= 6 AND dHash <= 8 AND aspect within 2% AND no contradicting metadata (ADR-0013).
- Union-find grouping with the 60 s chain-merge guard; similar-shot suggestions stored separately.
- Per-member explanations (edge kind, distances); incremental regrouping for new files.
- User-requested separation of a member ("not a duplicate"), persisted as a constraint.
- Explainable lexicographic best-copy ranking, RAW+JPEG keeper sets, `thumbnail_cache` never keeper when a better
  copy exists, persisted keeper overrides (ADR-0014).
- Corpus gate: precision >= 0.995, recall >= 0.97, keeper accuracy >= 0.99, zero must-not-merge violations (ADR-0025).
- Service API for groups, members, reasons, suggestions and overrides (ADR-0027).

## Capabilities

### New Capabilities
- `duplicate-detection`: deciding which files are copies of the same shot and grouping them, with reasons.
- `best-copy`: choosing and explaining the best copy (keeper) of each group, with user overrides.

### Modified Capabilities
- None.

## Impact

- ADRs: 0011, 0012, 0013, 0014, 0015, 0025, 0026, 0027. New ADR needed for user separation constraints (see design).
- Crates: `ps-model` (group, edge, reason, suggestion types), `ps-dedup` (edge rules, union-find, guard, ranking),
  `ps-catalog` (migration: groups, members, suggestions, overrides, separations), `ps-service` (groups API),
  `ps-scan` (post-scan grouping step).

## Out of scope

- Review UI (`duplicate-review`); similar-shot browsing UI (later `similar-shots`).
- Video perceptual dedup and CNN embeddings (ADR-0011, ADR-0012).
- Manual merging of groups the engine kept apart.
