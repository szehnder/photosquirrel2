# 0018. Virtualized gallery grid with a full ID index and a texture LRU

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The gallery must scroll smoothly through 1M items, jump anywhere instantly (scrollbar, date scrubber), and lazy-load thumbnails without stutter. Infinite scrolling with page fetches makes the scrollbar lie and makes "jump to 2009" slow.

## Decision
- For the current filter + sort, the catalog returns the **complete ordered list of item IDs** plus a compact sort key (16 bytes per item: 16 MB for 1M), queried off the UI thread. The scrollbar is therefore exact and any position is addressable in O(1).
- The grid is a custom GPUI element with fixed-size cells (zoomable 96-512 px) laid out arithmetically; only visible cells plus a prefetch margin are touched each frame.
- Item details (filename, flags) load in pages of 512 around the viewport into an LRU.
- Thumbnail textures live in an LRU keyed by content id (budget 512 MB GPU / 3,000 textures). Decode requests are prioritised: visible first, then the scroll direction's prefetch window (2 screens ahead), and are cancelled when they scroll out. Cells without a texture draw the stored dominant colour, then fade the image in.
- A timeline scrubber maps dates to offsets using the sort keys.

## Alternatives considered
- **Page-based infinite scroll** - inaccurate scrollbar, slow jumps.
- **gpui `uniform_list` of rows** - workable, but a custom element gives us one draw layer for all images (see gpui notes on paint layers).

## Consequences
- Frame budget: p95 <= 8.3 ms while flinging at 1M items (ADR-0026).

## Revisit when
Justified (aspect-preserving) layout is requested; it needs prefix sums over aspect ratios, which this design supports.
