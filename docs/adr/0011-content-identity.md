# 0011. Content identity: BLAKE3, sampled hashing for large videos

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Exact duplicates are identified by content hash. Videos can be many GB; hashing them fully during the scan dominates I/O.

## Decision
Content identity is the BLAKE3 hash of the whole file for every file <= 256 MB. Larger files (videos) get a **sampled identity**: BLAKE3 over size + first, middle and last 4 MB, marked `sampled`. Sampled identities group exact duplicates only together with matching size, duration and dimensions. Every export computes the full BLAKE3 while copying (ADR-0022), and the ledger stores the full hash.

## Alternatives considered
- **SHA-256** - slower, no benefit for a non-adversarial setting.
- **xxHash** - faster but collisions are plausible at this scale; BLAKE3 is fast enough (multi-GB/s, I/O-bound anyway).

## Consequences
- Video dedup of re-encoded copies is out of scope for v1.

## Revisit when
Sampled collisions are ever observed in practice.
