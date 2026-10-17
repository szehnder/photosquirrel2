# 0013. Duplicate grouping: same image at any size, never bursts

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The user chose: auto-group exact copies plus resized/recompressed/thumbnail versions of the same shot; show burst / similar shots only as suggestions. A false merge could hide a unique photo, so precision matters more than recall.

## Decision
A **duplicate group** is a connected component of edges of two kinds:
- **exact**: identical content identity (ADR-0011);
- **derivative**: pHash distance <= 6 AND dHash distance <= 8 AND aspect ratio within 2% (after orientation) AND no contradicting metadata (different capture timestamps more than 2 s apart when both have EXIF DateTimeOriginal, or different camera serial/model when both present).
Union-find builds groups; a guard splits any component whose members' capture timestamps span > 60 s (chain-merge protection). Pairs that pass the hash test but fail the metadata test become **similar-shot suggestions**, stored separately, never collapsed. Groups are recomputed incrementally for new files.

## Alternatives considered
- **Hash distance only** - merges burst shots and lookalike skies.
- **Clustering by embeddings** - not explainable enough for a safety-critical decision.

## Consequences
- Every group stores *why* each member joined (edge kind, distances), shown in the UI.

## Revisit when
The corpus gate (ADR-0025) or user feedback shows systematic false merges.
