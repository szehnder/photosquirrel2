# Tasks

## 1. Data plumbing

- [x] 1.1 `ps-service::library` (ID list, detail pages, preview, thumbnails) and the synthetic 1M catalog generator in `ps-fixtures`; verify `cargo test -p ps-service library` returns 1M IDs and 512-item pages (`ps_service::library::Library`, with the data types in `ps_model::library`; `ui-workspace::library_backend::CatalogBackend` is now a thin `GallerySource` adapter over it; `tests/library.rs`: `library_id_list_and_512_item_pages` on a 1,500-item synthetic catalog, ignored `library_queries_over_1m`: 1M IDs in 211 ms, a 512-item page in 2.7 ms, release; deviation: the generator lives in `ps_service::library::synthetic`, not `ps-fixtures`, because it writes through `ps-catalog` and encodes thumbnails with `ps-media`, and a core crate must not depend on the catalog, ADR-0003)
- [x] 1.2 Catalog migration `0007_gallery.sql` (filter indexes, `user_inclusion`); verify migration upgrade test in `ps-catalog`

## 2. Grid

- [x] 2.1 Grid element with arithmetic layout and exact scroll range; verify `scenario_gallery_scrollbar_reflects_total` and `scenario_gallery_jump_to_any_position`
- [x] 2.2 Zoom with anchor; verify `scenario_gallery_zoom_clamps_to_range` and `scenario_gallery_zoom_keeps_anchor_item`
- [x] 2.3 Decode scheduler with priorities and cancellation; verify `scenario_gallery_visible_thumbnails_load_first` and `scenario_gallery_offscreen_requests_cancelled`
- [x] 2.4 Dominant-colour placeholder and fade-in; verify `scenario_gallery_dominant_colour_placeholder`
- [x] 2.5 Texture LRU; verify `scenario_gallery_texture_cache_bounded`
- [x] 2.6 Stacks and expansion overlay; verify `scenario_gallery_group_shows_as_one_stack` and `scenario_gallery_expand_a_stack`
- [x] 2.7 Selection model; verify `scenario_gallery_shift_click_selects_range`
- [x] 2.8 Live updates with anchor preservation; verify `scenario_gallery_new_items_keep_scroll_anchor`

## 3. Navigation

- [x] 3.1 Sort and filter bar; verify `scenario_gallery_filter_by_camera_model`
- [x] 3.2 Timeline scrubber; verify `scenario_gallery_scrubber_jumps_to_year`
- [x] 3.3 Loupe with preview and offline fallback; verify `scenario_gallery_loupe_opens_selected_photo`, `scenario_gallery_loupe_returns_to_same_position`, `scenario_gallery_offline_photo_shows_thumbnail`
- [x] 3.4 Excluded view; verify `scenario_gallery_excluded_view_lists_reasons`

## 4. Inspector

- [x] 4.1 Inspector sections with mono metadata; verify `scenario_photo_inspector_inspector_shows_camera_exif` and `scenario_photo_inspector_missing_fields_shown_as_absent`
- [x] 4.2 Copies list; verify `scenario_photo_inspector_inspector_lists_all_copies`
- [x] 4.3 Tag list search and copy-to-clipboard; verify `scenario_photo_inspector_search_raw_tags` and `scenario_photo_inspector_copy_hash_to_clipboard`
- [x] 4.4 Reveal in file manager (recorded opener); verify `scenario_photo_inspector_reveal_in_file_manager`
- [x] 4.5 Multi-selection summary; verify `scenario_photo_inspector_multi_selection_summary`
- [x] 4.6 User include/exclude through `ps-service`; verify `scenario_photo_inspector_include_an_excluded_file` and `scenario_photo_inspector_exclude_a_photo`

## 5. Budgets

- [ ] 5.1 `--render-bench` and `perf-budgets.toml` entry `gallery-scroll-p95` (8.3 ms, app source, `render_p95`); verify `scenario_gallery_1m_scroll_p95_within_8_3_ms`
- [x] 5.2 First gallery frame measurement and entry `first-gallery-frame` (600 ms); verify `scenario_gallery_first_gallery_frame_within_600_ms`

## 6. Verification

- [ ] 6.1 `cargo xtask ci` is green
- [ ] 6.2 Gate: `cargo xtask bench --check` reports 1M-item scroll p95 <= 8.3 ms on the reference machine
- [ ] 6.3 `cargo xtask screenshot --all-surfaces` reviewed (grid, stacks, loupe, inspector, excluded)
- [x] 6.4 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [x] 6.5 `openspec validate gallery --strict` passes
