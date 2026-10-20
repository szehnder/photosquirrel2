# Proposal

## Why

Browsing a lifetime of photos must feel instant: fling through a million thumbnails without a dropped frame,
jump to 2009 in one drag, open any photo in a loupe, and see every EXIF detail and every copy of it. This is the
heart of the "Lightroom on steroids" experience and the place where the user reviews what the engine found.

## What Changes

- Custom virtualized GPUI grid over the complete ordered ID list (exact scrollbar, O(1) jumps), fixed cells
  zoomable 96-512 px, details paged in 512-item pages (ADR-0018).
- Prioritised lazy thumbnail decoding (visible, then 2 screens ahead in the scroll direction), cancellation,
  dominant-colour placeholders with fade-in, texture LRU (512 MB / 3,000 textures) (ADR-0016, ADR-0018).
- Duplicate groups shown as one stack (keeper tile with a count badge), expandable.
- Sort and filter (date, name, size; class, media kind, source, folder, date range, camera, GPS).
- Timeline scrubber; Excluded view with reasons; keyboard/mouse selection; loupe with 2048 px previews.
- Photo inspector: file, capture, camera and exposure, GPS, quality, hash, classification reasons, all copies,
  full searchable tag list, multi-selection summary, user include/exclude.
- Render benchmark: p95 <= 8.3 ms flinging 1M items; first gallery frame <= 600 ms with a 1M catalog (ADR-0026).

## Capabilities

### New Capabilities
- `gallery`: the virtualized photo grid, loupe, filters, timeline and selection.
- `photo-inspector`: the panel showing everything known about the selected photo(s).

### Modified Capabilities
- None.

## Impact

- ADRs: 0005, 0015, 0016, 0018, 0023, 0026, 0027, 0028.
- Crates: `ui-gallery` (grid element, texture LRU, decode scheduler, loupe, scrubber), `ui-workspace` (Library and
  Excluded views, inspector panel), `ps-service` (library queries, item details, user exclusion),
  `ps-catalog` (filter/sort extensions, migration for user include/exclude), `photosquirrel` (`--render-bench`).

## Out of scope

- Justified (aspect-preserving) layout (later `justified-layout`); map view; face clusters.
- Editing photos; rating/flags/colour labels; similar-shot browsing.
- Duplicate group review workflow (`duplicate-review`); AI filters (`ai-insights` adds them).
