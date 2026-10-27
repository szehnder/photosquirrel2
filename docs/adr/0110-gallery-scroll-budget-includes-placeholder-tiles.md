# 0110. The gallery scroll budget is measured with 30% placeholder tiles

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0026](0026-performance-budgets.md) (what `gallery-scroll-p95` measures)

## Context
`preview-placeholders` ([ADR-0109](0109-placeholders-carry-a-reason-and-a-decoder-generation.md)) draws every item
without a preview as a placeholder tile: a surface, a hatch, a glyph, a format chip, a caption and a status mark,
up to eight primitives and three text runs per cell, where a thumbnail is one image. A library catalogued by 0.2.0
can have thousands of them (the owner's TIFFs), and the render bench flung only thumbnails, so a costly tile would
not show in `gallery-scroll-p95`.

## Decision
- `photosquirrel --render-bench` takes `--bench-placeholders <percent>`; the synthetic source then draws that share
  of its 1,000,000 items (spread evenly, every status in turn) as placeholder tiles whose thumbnails have no texture.
- The `gallery-scroll-p95` budget runs `--render-bench 10 --bench-placeholders 30`. Its bound is unchanged:
  p95 <= 8.3 ms.
- The headless proxy (`ui-gallery/tests/render_cost.rs`) gets the same fling with 30% placeholders, held to the same
  bound, beside the plain one.
- Placeholder tiles batch into the grid's layers (five extra layers per frame, not per cell) and their text is shaped
  through GPUI's line cache.

## Alternatives considered
- **A second budget for placeholders** - two budgets for one interaction; the mixed fling is the harder case and
  covers both.
- **Measure only the headless proxy** - it has no GPU and misses atlas and layer costs.

## Consequences
- A regression in placeholder drawing fails the same budget as a regression in thumbnail drawing.
- The baseline is re-recorded with placeholders; earlier values (thumbnails only) are reported beside it once.

## Revisit when
The grid draws another kind of cell (a video poster strip, an AI badge) with its own primitives.
