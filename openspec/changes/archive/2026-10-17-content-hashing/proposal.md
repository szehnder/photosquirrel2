# Proposal

## Why

Finding the best copy of every photo requires two kinds of identity: byte-exact identity for true copies and
perceptual identity for the same shot at another size or compression. Both must be fast enough for 1M+ files,
deterministic so results are reproducible, and computed from the single read the scanner already performs.

## What Changes

- **Content identity**: full BLAKE3 for files up to 256 MB; a marked *sampled* identity (size + first, middle,
  last 4 MB) for larger files, computed from the single analysis read or a stream for files over 64 MB
  (ADR-0010, ADR-0011).
- **Perceptual hashes**: our own 64-bit DCT pHash and gradient dHash on a 32x32 luminance image derived from the
  oriented downscaled decode, versioned (ADR-0012).
- **Near-duplicate index**: an in-memory multi-index hash table (4 x 16-bit bands with 1-bit neighbour probing)
  returning verified candidates within distance 7, rebuilt from stored hashes in under 1 s for 1M entries.
- Hamming-distance helpers using hardware popcount (ADR-0032).
- Criterion benchmarks for the ADR-0026 budgets: pHash+dHash <= 0.3 ms per image, lookup <= 1 ms p95 over 1M,
  index build < 1 s.

## Capabilities

### New Capabilities
- `content-identity`: byte-exact identity of a file's content, full or sampled.
- `perceptual-hashing`: perceptual hashes of images and fast near-duplicate candidate lookup.

### Modified Capabilities
- None.

## Impact

- ADRs: 0010, 0011, 0012, 0025, 0026, 0032.
- Crates: `ps-model` (`ContentId`, `IdentityKind`, `PHash`, `DHash`, `HashVersion`), `ps-hash` (BLAKE3 identity,
  pHash, dHash, MIH index, benches).

## Out of scope

- Deciding groups from candidates (`duplicate-groups`); persisting hashes (`catalog`, `scanning`).
- Re-hashing from the thumbnail store after an algorithm version bump (`scanning`, incremental-rescan).
- Video perceptual hashing and CNN embeddings (later `similar-shots`, `video-perceptual-dedup`).
