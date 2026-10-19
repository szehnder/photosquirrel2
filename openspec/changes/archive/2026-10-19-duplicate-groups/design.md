# Design

## Context

Implements ADR-0013 (grouping rules) and ADR-0014 (best-copy ranking) on top of identities and hashes
(ADR-0011, ADR-0012) and classes (ADR-0015). The corpus gate is ADR-0025. This is the M1 quality gate: no UI or
export work starts until the corpus scenarios pass.

## Goals / Non-Goals

**Goals:**
- Pure, deterministic grouping and ranking in `ps-dedup` (core), persisted by `ps-catalog`.
- Precision first: every edge and every keeper explainable.

**Non-Goals:**
- Similar-shot UI, embeddings, video perceptual dedup.

## Decisions

- **Crates.** `ps-dedup::edges` (exact, sampled, derivative rules and metadata contradiction check),
  `ps-dedup::group` (union-find with path compression, 60 s guard, separation constraints), `ps-dedup::rank`
  (lexicographic comparator returning `Vec<KeeperReason>`), `ps-dedup::keeper_set` (RAW+JPEG pairing).
  `ps-scan::post` runs grouping after a run's analysis completes (and after a hash upgrade); `ps-service::groups`
  exposes list/get/explain/override/separate/suggestions.
- **Candidate generation.** Exact: catalog index on `content_id`. Derivative: the MIH index (ADR-0012) queried for
  each new/changed image; all candidates verified by `ps-dedup::edges::derivative`, which also checks aspect
  (display dimensions, 2%) and metadata (ADR-0035: capture times must match to the second and sub-second; serial or model differ; two camera originals with equal dimensions).
  Rejected-by-metadata pairs become `suggestion(file_a, file_b, phash_d, dhash_d, reason)` rows.
- **Chain guard.** After union-find, components spanning > 60 s of capture time are split by sorting members by
  capture time and cutting at the largest gaps until every part spans <= 60 s; members without timestamps join
  the part of the member they have the strongest (exact, then lowest distance) edge to.
- **Incremental.** Affected components = components of new/changed files plus components reachable through
  their new edges; only those are recomputed. A property test compares with full recompute on the corpus.
- **Schema (migration 2).** `dup_group(id, keeper_file_id, keeper_set JSON, reasons JSON, member_count,
  reclaimable_bytes, reviewed_at)`, `dup_member(group_id, file_id, edge_kind, via_file_id, phash_d, dhash_d)`,
  `suggestion(...)`, `keeper_override(content_id, chosen_file_id, at)` (keyed by content identity of the chosen
  file so it survives regrouping and moves), `separation(content_a, content_b, at)`.
- **Ranking.** Format tier from `ps-media` format + camera-original signal; upscale detection: when a larger
  member has a pHash within 2 of a smaller member, a lower quality estimate and no camera-original signal, its
  pixel count is replaced by the smaller member's for comparison. Tie-break of last resort: lowest file id
  after path depth, making the order total.
- **Keeper sets.** RAW + JPEG with same camera serial/model, DateTimeOriginal within 1 s and same stem in the
  same folder (ADR-0014). The set's JPEG is the display representative.
- **New ADR needed: "User separation of group members".** ADR-0013 defines no user correction for false merges.
  A falsely merged unique photo would be `covered` (ADR-0023) and eligible for quarantine (ADR-0024). We propose
  ADR-0033: persisted pairwise separation constraints keyed by content identity, enforced as cannot-link
  constraints during union-find (a component containing a separated pair is split along the weakest path).
- **Gate tests.** `ps-service/tests/corpus_gate.rs` generates the medium corpus (seed 42), scans it, groups it and
  evaluates with `ps-fixtures::eval`; prints `measure grouping-precision=...` lines for the record.

## Risks / Trade-offs

- [ADR-0013's 2 s rule lets bursts shot < 2 s apart merge when hashes are close (the corpus includes such
  bursts)] -> if the gate fails on bursts, propose a superseding ADR that treats any DateTimeOriginal difference
  (including SubSecTimeOriginal) as contradicting when both values are present. Not changed here.
- [Members without capture timestamps defeat the chain guard] -> strongest-edge attachment; covered by corpus
  EXIF-stripped copies.
- [Upscale heuristic misfires on edited exports] -> tie-breaker only below tier.

## Verification

- `cargo test -p ps-dedup` (unit and proptests for rules and ranking).
- `cargo test -p ps-service --test corpus_gate --release` (gate scenarios).
- `cargo test -p ps-catalog -p ps-service groups`.
- `cargo xtask ci`, `openspec validate duplicate-groups --strict`.
