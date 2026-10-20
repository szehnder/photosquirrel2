# Design

## Context

ADR-0018 fixes the grid architecture (full ID index, fixed cells, paged details, texture LRU, prioritised
decode, scrubber). Thumbnails come from `thumbs.db` (ADR-0016) and previews from the 2048 px LRU. Queries go
through `ps-service` off the UI thread (ADR-0005, ADR-0027). Budgets: ADR-0026. Styling: ADR-0028 tokens only.
User exclusion is the `excluded` disposition source of ADR-0023 and must never be an AI decision (ADR-0021).

## Goals / Non-Goals

**Goals:**
- p95 <= 8.3 ms flinging 1M items; first gallery frame <= 600 ms; exact scrollbar.
- One paint layer for all thumbnails (`docs/gpui-notes.md`: batch inside `window.paint_layer`).

**Non-Goals:**
- Justified layout (ADR-0018 revisit), map view, ratings/flags.

## Decisions

- **Crates/modules.** `ui-gallery::grid` (custom GPUI `Element`: arithmetic layout `index -> (row, col)`, visible
  range + prefetch margin, one `paint_layer` for dominant-colour quads and one for images), `ui-gallery::index`
  (`Arc<[ItemId]>` + sort keys from `ps-service::library::id_list`), `ui-gallery::details` (512-item pages in an
  LRU of 64 pages), `ui-gallery::textures` (LRU keyed by content id, 3,000 entries / 512 MB accounting by
  `w*h*4`, `window.drop_image` on eviction), `ui-gallery::decode` (priority queue on a background pool:
  visible > forward prefetch 2 screens > backward 0.5 screen; generation counter cancels stale requests;
  `zune-jpeg` decode to BGRA), `ui-gallery::scrubber`, `ui-gallery::loupe` (fit/100% zoom, 2048 px preview via
  `ps-service::library::preview`, falls back to thumbnail with "source offline").
  `ui-workspace::views::{library, excluded}` and `ui-workspace::inspector`.
- **Stacks.** The default Library filter is `keepers_only = true`: the ID list includes one row per group
  (keeper) plus ungrouped files; the sort key's 4 flag bytes carry the member count for the badge. Expanding a
  stack splices the group's member IDs into a local overlay index after the keeper (no re-query of the 1M list).
- **Live updates.** During scans the view re-queries the ID list at most every 2 s; the new list is diffed by
  locating the current top item via its sort key (binary search) to keep the anchor.
- **Filters.** `ps-model::Filter` gains camera model, GPS presence, folder prefix and date range; each maps to
  an indexed SQL predicate (new covering index `(camera_model, capture_ts, id)` in migration `0007_gallery.sql`).
- **User include/exclude (migration `0007_gallery.sql`).** `user_inclusion(file_id, decision, reason, at)`; the classifier result
  is kept and the user decision wins in queries and in later rescans (ADR-0015 "nothing silently disappears").
- **Reveal.** `opener` logic per OS (`xdg-open` on the parent folder / `dbus FileManager1.ShowItems` on Linux,
  `open -R` on macOS, `explorer /select,` on Windows) behind a trait so tests record the call.
- **Render bench.** `photosquirrel --render-bench` loads a synthetic 1M catalog (generated into a temp data dir by
  `ps-fixtures::synthetic_catalog`, thumbnails shared from 3,000 content ids so textures are warm), flings for
  10 s and prints `frame interval ms: p50=.. p95=..`, which `cargo xtask bench` reads (`render_p95`). First
  gallery frame uses `--measure-startup` with the 1M catalog and a `measure first-gallery-frame` line.

## Risks / Trade-offs

- [Texture upload spikes when flinging into cold regions] -> cap uploads per frame (e.g. 24) and keep the
  dominant-colour fallback; uploads beyond the cap wait for the next frame.
- [1M ID list memory and query time] -> measured in `catalog` (300 ms budget); query runs while the shell paints.
- [GPUI frame pacing under Wayland when occluded] -> render bench runs focused; noted in gpui-notes.

## Verification

- `cargo test -p ui-gallery -p ui-workspace` (headless `#[gpui_kit::test]` for all `scenario_gallery_*` and
  `scenario_photo_inspector_*`).
- `cargo build --release -p photosquirrel && cargo xtask bench --check` (render p95, first gallery frame).
- `cargo xtask screenshot --all-surfaces` (grid, stack expanded, loupe, inspector, excluded view).
- `cargo xtask ci`, `openspec validate gallery --strict`.
