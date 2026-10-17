# 0010. Probe first, then a single full read per file

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Reading every byte twice (once to hash, once to decode) doubles scan time on slow drives. Many files (icons, UI assets, tiny web images) are not worth reading at all.

## Decision
Analysis has two steps. **Probe** reads at most the first 256 KB: magic-number sniffing, dimensions (header), EXIF block and embedded thumbnail. The photo classifier (ADR-0015) runs on probe data; files classified as junk are recorded as `skipped` with a reason and never fully read. **Analyze** reads candidates once into memory (buffers > 64 MB are streamed instead) and computes, from that single buffer: BLAKE3 content hash, full metadata, a downscaled decode (JPEG DCT scaling to the smallest size >= 512 px), perceptual hashes and the grid thumbnail.

## Alternatives considered
- **Hash everything first, analyze unique files later** - two passes over slow media.

## Consequences
- One read per candidate file; junk costs only a probe.

## Revisit when
Never for photos; videos follow ADR-0011.
