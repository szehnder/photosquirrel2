# 0087. TIFF decode budgets: 12 MP analysis time and streamed working memory (extends 0026)

- Status: accepted
- Date: 2026-10-24
- Deciders: maintainers
- Extends: [0026](0026-performance-budgets.md)

## Context
ADR-0085 and ADR-0086 add a TIFF decoder whose time and memory are new acceptance criteria: TIFF decodes are far
more expensive than JPEG DCT-scaled decodes (LZW and 16-bit samples), and the one-pass decode must stay bounded.

## Decision
Two component budgets, measured by tests in `ps-media` (`tests/tiff.rs`) and guarded like the others by
`cargo xtask bench` (relative regressions in CI, ADR-0057):

- `tiff-analysis-decode`: median analysis decode plus grid thumbnail of a 12 MP (4000x3000) 16-bit RGB LZW TIFF
  with horizontal predictor and its directories after the data, from memory, on one core. Bound: 1000 ms
  (measured 320 ms on the reference machine; LZW decompression by the `tiff` crate dominates, as it did for the
  previous `image` decode).
- `tiff-stream-memory`: working memory (largest strip, compressed plus decoded, and the downsampling accumulator)
  the decoder reports for a 24 MP (6000x4000) 16-bit greyscale TIFF with 16-row strips decoded through a reader,
  as a TIFF above 64 MB is. Bound: 4 MB (measured 0.94 MB), far below the 32 MB chunk cap and the 64 MB the lane
  reserves.

## Alternatives considered
- **A whole-scan budget for TIFF archives** - dominated by disk speed; the component budgets isolate the code.
- **Measured peak RSS** - noisy across allocators and runners; the decoder's own accounting is deterministic.

## Consequences
- A slower TIFF decoder or a larger working set fails `cargo xtask bench --check`.

## Revisit when
The decoder changes its chunk strategy, or the reference machine changes.
