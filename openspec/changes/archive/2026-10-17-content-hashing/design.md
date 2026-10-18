# Design

## Context

Implements ADR-0011 (content identity) and ADR-0012 (perceptual hashes and MIH index) in the core crate
`ps-hash`, consuming the oriented analysis decode from `media-analysis` (ADR-0010). Budgets come from ADR-0026;
popcount relies on the x86-64-v2 baseline (ADR-0032).

## Goals / Non-Goals

**Goals:**
- Pure, deterministic, allocation-light functions; no I/O except through a caller-supplied reader.
- Measured budgets as criterion benches registered in `perf-budgets.toml`.

**Non-Goals:**
- Grouping decisions (ADR-0013) and persistence; CNN embeddings (ADR-0012 alternatives).

## Decisions

- **Types (`ps-model`).** `ContentId([u8; 32])` with `IdentityKind { Full, Sampled }`; `PHash(u64)`, `DHash(u64)`,
  `HashVersion(u16)` starting at 1; `PerceptualHashes { version, phash, dhash }`.
- **`ps-hash::identity`.** `identity_from_buffer(&[u8])` for <= 64 MB buffers; `identity_from_reader(R, len)`
  streams in 1 MB chunks with `blake3::Hasher::update_rayon` disabled (determinism and to keep CPU threads for
  decode; BLAKE3 is I/O-bound here). Sampled: hasher fed `len.to_le_bytes()`, then the three 4 MB windows; the
  middle window starts at `len/2 - 2 MB`. The 256 MB / 4 MB / 64 MB constants are named in `ps-model::limits`.
- **`ps-hash::phash`.** Input: the oriented decode from `ps-media::decode`. Luminance (BT.601) -> 32x32 area
  resample -> 2-D DCT-II (separable, precomputed f32 cosine table) -> top-left 8x8 excluding nothing, median of
  the 64 values (DC included, documented), bit = coeff > median. Fixed iteration order for determinism.
- **`ps-hash::dhash`.** 9x8 resample of the same 32x32 luminance, bit = left < right, row-major.
- **`ps-hash::index` (MIH).** Four `FxHashMap<u16, Vec<u32>>` tables (rustc-hash) over the pHash bands; a query
  probes each band's exact key and its 16 one-bit neighbours (17 x 4 = 68 probes), deduplicates candidate
  slots with a generation-stamped bitset and verifies both hashes with `count_ones`. Storage is struct-of-arrays
  (`Vec<u64>` pHash, `Vec<u64>` dHash, `Vec<FileKey>`), so 1M entries cost ~40 MB. Build sorts per band then
  bulk-inserts; incremental `insert` for new files.
- **Golden values.** `ps-hash/tests/golden.rs` stores expected hashes for the reference fixtures; changing an
  algorithm requires bumping `HashVersion` and the goldens together (ADR-0012 consequence).

## Risks / Trade-offs

- [Float differences across platforms change a bit near the median] -> the orientation scenario allows
  distance 2; goldens are checked on all three CI OSes; if they diverge, switch the DCT to fixed-point.
- [Planted-neighbour recall test is probabilistic] -> fixed seed via `rand_chacha`.

## Verification

- `cargo test -p ps-hash -p ps-model` (all `scenario_content_identity_*`, `scenario_perceptual_hashing_*`).
- `cargo test --release -p ps-hash --test budgets` (budget scenarios print `measure` lines).
- `cargo bench -p ps-hash --bench hashing` and `cargo xtask bench --check`.
- `cargo xtask layering`, `cargo xtask ci`, `openspec validate content-hashing --strict`.
