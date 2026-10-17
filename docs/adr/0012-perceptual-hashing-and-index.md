# 0012. Perceptual hashes and a near-duplicate index

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Backups contain the same photo at different sizes (thumbnails, web exports) and recompressions. These are not byte-identical.

## Decision
Each analyzed image gets two 64-bit perceptual hashes computed by our own code on a 32x32 luminance image derived from the downscaled decode, after applying EXIF orientation: a **DCT pHash** (low 8x8 frequencies vs median) and a **gradient dHash**. Candidate pairs come from a multi-index hash table (4 x 16-bit bands; pigeonhole guarantees recall at Hamming distance <= 3 per hash, and we probe 1-bit neighbours of each band to reach <= 7) and are then verified on both hashes. The index is rebuilt in memory from the catalog at start-up (1M hashes in < 1 s).

## Alternatives considered
- **`image_hasher` crate** - fine, but we want deterministic, documented algorithms and control over orientation and pre-processing.
- **CNN embeddings** - much heavier; reserved for the optional "similar shots" suggestions.
- **BK-tree** - degrades at larger radii; MIH is faster for our radius.

## Consequences
- Hash algorithms are versioned; changing them triggers re-hashing from the thumbnail store, not from the sources.

## Revisit when
Precision/recall on the ground-truth corpus drops below the gate in ADR-0025.
