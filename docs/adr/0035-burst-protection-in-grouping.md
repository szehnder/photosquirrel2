# 0035. Stricter capture-time rule for derivative edges (burst protection)

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
ADR-0013 allows derivative edges between images whose capture times differ by up to 2 s. Cameras and phones shoot bursts many frames per second; those frames have near-identical perceptual hashes and would merge, contradicting "bursts are suggestions only". Resized/recompressed copies preserve EXIF capture time exactly when they preserve EXIF at all.

## Decision
This ADR supersedes the timestamp clause of ADR-0013. A derivative edge additionally requires, when **both** images carry a capture time: equal capture time to the second, and equal sub-second values when both have them. Additionally, two images that **both** look like camera originals (maker notes present, same camera model) with identical stored pixel dimensions but different content identities are never derivatives - two originals are two shots. Pairs rejected by these rules but within hash thresholds become similar-shot suggestions. The chain-merge guard of ADR-0013 (split components spanning > 60 s) stays.

## Alternatives considered
- **Keep the 2 s window** - merges bursts.

## Consequences
- The ground-truth corpus includes same-second bursts with and without sub-second times.

## Revisit when
The corpus or users show that legitimate derivatives have shifted timestamps (e.g. timezone-adjusted exports); then add offset-aware matching.
