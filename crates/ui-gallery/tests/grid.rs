//! Headless tests of the gallery grid (spec `gallery`): layout, jumps, zoom, prioritised decoding,
//! placeholders, the texture budget, stacks, selection, live updates and the scrubber.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Modifiers, MouseButton, TestAppContext, px, size};
use ps_model::{ItemId, SortKey};
use support::*;
use ui_gallery::bench::SyntheticSource;
use ui_gallery::decode::DecodeQueue;
use ui_gallery::grid::CellPaint;
use ui_gallery::index::StackRole;
use ui_gallery::theme::{MAX_CELL, MIN_CELL};

#[gpui_kit::test]
fn scenario_gallery_scrollbar_reflects_total(cx: &mut TestAppContext) {
    let (grid, _, cx) = open_grid(cx, Arc::new(SyntheticSource::new(1_000_000, 3_000)));
    cx.update(|_, cx| {
        let g = grid.read(cx);
        let l = g.layout(cx);
        assert_eq!(l.count, 1_000_000);
        let rows = 1_000_000usize.div_ceil(l.columns());
        assert_eq!(l.rows(), rows);
        let expected =
            2.0 * f64::from(l.padding) + rows as f64 * f64::from(l.pitch()) - f64::from(l.gap);
        assert!((l.content_height() - expected).abs() < 1e-6);
        assert!(
            (l.max_scroll(g.viewport_height()) - (expected - f64::from(g.viewport_height()))).abs()
                < 1e-6
        );
    });
}

#[gpui_kit::test]
fn scenario_gallery_jump_to_any_position(cx: &mut TestAppContext) {
    let (grid, _, cx) = open_grid(cx, Arc::new(SyntheticSource::new(1_000_000, 3_000)));
    let start = Instant::now();
    grid.update(cx, |g, cx| g.scroll_to_fraction(0.5, cx));
    let mut drawn = false;
    while start.elapsed() < Duration::from_secs(2) {
        frames(cx, 1);
        drawn = cx.update(|_, cx| {
            matches!(
                grid.read(cx).painted(500_000),
                Some(CellPaint::Placeholder(_) | CellPaint::Texture { .. })
            )
        });
        if drawn {
            break;
        }
    }
    let elapsed = start.elapsed();
    assert!(drawn, "item 500,000 is drawn");
    assert!(cx.update(|_, cx| grid.read(cx).visible_range(cx).contains(&500_000)));
    assert!(
        elapsed < Duration::from_millis(100),
        "drawn after {elapsed:?}"
    );
}

#[gpui_kit::test]
fn scenario_gallery_zoom_clamps_to_range(cx: &mut TestAppContext) {
    let (grid, _, cx) = open_grid(cx, Arc::new(FakeSource::new(500)));
    for _ in 0..40 {
        grid.update(cx, |g, cx| g.zoom_by(-1.0, None, cx));
    }
    assert_eq!(cx.update(|_, cx| grid.read(cx).cell_size()), MIN_CELL);
    for _ in 0..40 {
        grid.update(cx, |g, cx| g.zoom_by(1.0, None, cx));
    }
    assert_eq!(cx.update(|_, cx| grid.read(cx).cell_size()), MAX_CELL);
    grid.update(cx, |g, cx| g.set_cell(9.0, None, cx));
    assert_eq!(cx.update(|_, cx| grid.read(cx).cell_size()), MIN_CELL);
}

#[gpui_kit::test]
fn scenario_gallery_zoom_keeps_anchor_item(cx: &mut TestAppContext) {
    let (grid, _, cx) = open_grid(cx, Arc::new(SyntheticSource::new(100_000, 3_000)));
    grid.update(cx, |g, cx| g.jump_to_index(12_340, cx));
    frames(cx, 1);
    let p = cx.update(|_, cx| grid.read(cx).cell_center(12_345, cx));
    assert_eq!(
        cx.update(|_, cx| grid.read(cx).index_at(p, cx)),
        Some(12_345)
    );
    for steps in [2.0, 1.0, -3.0, -2.0, 4.0] {
        grid.update(cx, |g, cx| g.zoom_by(steps, Some(p), cx));
        frames(cx, 1);
        assert_eq!(
            cx.update(|_, cx| grid.read(cx).index_at(p, cx)),
            Some(12_345),
            "after zooming {steps} steps"
        );
    }
}

#[gpui_kit::test]
fn scenario_gallery_visible_thumbnails_load_first(cx: &mut TestAppContext) {
    init(cx);
    let source = Arc::new(FakeSource::new(5_000));
    let model = gpui_kit::AppContext::new(cx, |cx| {
        let mut m = ui_gallery::GalleryModel::new(source.clone(), Default::default());
        m.decode = DecodeQueue::recording();
        m.set_query(Default::default(), cx);
        m
    });
    let m2 = model.clone();
    let (grid, cx) = cx.add_window_view(move |_, cx| {
        let mut g = ui_gallery::GalleryGrid::new(m2, cx);
        g.set_cell(128.0, None, cx);
        g
    });
    cx.simulate_resize(size(px(1480.0), px(800.0)));
    frames(cx, 30);
    let visible = cx.update(|_, cx| grid.read(cx).visible_range(cx));
    assert_eq!(visible.len(), 60, "60 visible cells");
    let log = cx.update(|_, cx| model.read(cx).decode.completed_log.clone().unwrap());
    assert!(log.len() > 60, "prefetch continued after the viewport");
    let first: Vec<usize> = log[..60]
        .iter()
        .map(|k| id_of(&k.thumb).0 as usize)
        .collect();
    let mut sorted = first.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, visible.collect::<Vec<_>>());
}

#[gpui_kit::test]
fn scenario_gallery_offscreen_requests_cancelled(cx: &mut TestAppContext) {
    let source = Arc::new(FakeSource::new(100_000));
    let (grid, model, cx) = open_grid(cx, source.clone());
    // Details are known everywhere (pages keep up with a fling); only decoding lags behind.
    let ids: Vec<ItemId> = cx.update(|_, cx| model.read(cx).index.ids.to_vec());
    let rows = ui_gallery::source::GallerySource::summaries(source.as_ref(), &ids).unwrap();
    model.update(cx, |m, _| m.insert_summaries(rows));
    frames(cx, 3);
    // Fling 50,000 items in one second: 60 frames, background work never gets to run in between.
    for f in 1..=60 {
        let target = 50_000 * f / 60;
        grid.update(cx, |g, cx| g.jump_to_index(target, cx));
        cx.update(|window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.render_frame(cx)
        });
    }
    frames(cx, 10);
    let (visible, cols, rows_per_screen) = cx.update(|_, cx| {
        let g = grid.read(cx);
        let l = g.layout(cx);
        (
            g.visible_range(cx),
            l.columns(),
            (g.viewport_height() / l.pitch()).ceil() as usize,
        )
    });
    let screen = cols * rows_per_screen;
    let lo = visible.start.saturating_sub(screen);
    let hi = visible.end + 2 * screen + cols;
    let (stats, keys) = cx.update(|_, cx| {
        let m = model.read(cx);
        let keys: Vec<usize> = m
            .textures
            .keys()
            .map(|k| id_of(&k.thumb).0 as usize)
            .collect();
        (m.decode.stats, keys)
    });
    assert!(
        stats.cancelled > 0,
        "pending decodes were cancelled: {stats:?}"
    );
    let far: Vec<usize> = keys
        .iter()
        .copied()
        .filter(|i| *i < lo && *i > 1_000 || *i > hi)
        .collect();
    assert!(
        far.is_empty(),
        "uploaded far from the final viewport: {far:?}"
    );
}

#[gpui_kit::test]
fn scenario_gallery_dominant_colour_placeholder(cx: &mut TestAppContext) {
    let mut source = FakeSource::new(200);
    source.no_thumbs = true;
    let (grid, _, cx) = open_grid(cx, Arc::new(source));
    frames(cx, 3);
    for i in [0usize, 7, 19] {
        assert_eq!(
            cx.update(|_, cx| grid.read(cx).painted(i)),
            Some(CellPaint::Placeholder(colour_of(i as u32)))
        );
    }
}

#[gpui_kit::test]
fn scenario_gallery_texture_cache_bounded(cx: &mut TestAppContext) {
    let (grid, model, cx) = open_grid(cx, Arc::new(FakeSource::new(100_000)));
    grid.update(cx, |g, cx| g.set_cell(96.0, None, cx));
    frames(cx, 1);
    let mut index = 0;
    while index < 100_000 {
        grid.update(cx, |g, cx| g.jump_to_index(index, cx));
        frames(cx, 1);
        index += cx
            .update(|_, cx| grid.read(cx).visible_range(cx).len())
            .max(1)
            * 2;
    }
    let (peak, peak_bytes, evictions, budget) = cx.update(|_, cx| {
        let t = &model.read(cx).textures;
        (t.peak_entries, t.peak_bytes, t.evictions, t.budget())
    });
    assert_eq!(budget.entries, 3_000);
    assert_eq!(budget.bytes, 512 * 1024 * 1024);
    assert!(peak <= 3_000, "{peak} textures");
    assert!(peak_bytes <= 512 * 1024 * 1024);
    assert!(evictions > 0, "the cache evicted while scrolling");
}

#[gpui_kit::test]
fn scenario_gallery_group_shows_as_one_stack(cx: &mut TestAppContext) {
    let (grid, model, cx) = open_grid(cx, Arc::new(FakeSource::new(20).stack(5, 4)));
    frames(cx, 5);
    cx.update(|_, cx| {
        let m = model.read(cx);
        assert_eq!(m.len(), 20, "one tile for the group");
        assert_eq!(
            m.index.role(5),
            StackRole::Keeper {
                count: 4,
                expanded: false
            }
        );
        assert!(matches!(
            grid.read(cx).painted(5),
            Some(CellPaint::Texture { .. })
        ));
    });
}

#[gpui_kit::test]
fn scenario_gallery_expand_a_stack(cx: &mut TestAppContext) {
    let (grid, model, cx) = open_grid(cx, Arc::new(FakeSource::new(20).stack(5, 4)));
    frames(cx, 3);
    grid.update(cx, |g, cx| g.click(5, Modifiers::default(), cx));
    cx.update(|window, cx| {
        let f = grid.read(cx).focus_handle().clone();
        window.focus(&f, cx)
    });
    cx.simulate_keystrokes("s");
    frames(cx, 3);
    cx.update(|_, cx| {
        let m = model.read(cx);
        assert_eq!(m.len(), 23);
        assert_eq!(
            m.index.role(5),
            StackRole::Keeper {
                count: 4,
                expanded: true
            }
        );
        for i in 6..9 {
            assert_eq!(m.index.role(i), StackRole::Member { keeper: ItemId(5) });
        }
        assert_eq!(m.index.id(9), Some(ItemId(6)));
    });
}

#[gpui_kit::test]
fn scenario_gallery_shift_click_selects_range(cx: &mut TestAppContext) {
    let (grid, model, cx) = open_grid(cx, Arc::new(FakeSource::new(200)));
    grid.update(cx, |g, cx| g.set_cell(120.0, None, cx));
    frames(cx, 1);
    let p10 = cx.update(|_, cx| grid.read(cx).cell_center(10, cx));
    let p20 = cx.update(|_, cx| grid.read(cx).cell_center(20, cx));
    cx.simulate_click(p10, Modifiers::default());
    frames(cx, 1);
    assert_eq!(cx.update(|_, cx| grid.read(cx).index_at(p20, cx)), Some(20));
    cx.simulate_click(p20, Modifiers::shift());
    cx.update(|_, cx| {
        let m = model.read(cx);
        assert_eq!(m.selection.len(), 11);
        assert_eq!(
            m.selected_ids(100),
            (10..=20).map(ItemId).collect::<Vec<_>>()
        );
    });
    let _ = MouseButton::Left;
}

#[gpui_kit::test]
fn scenario_gallery_new_items_keep_scroll_anchor(cx: &mut TestAppContext) {
    let source = Arc::new(FakeSource::new(5_000));
    let (grid, model, cx) = open_grid(cx, source.clone());
    grid.update(cx, |g, cx| g.jump_to_index(2_503, cx));
    grid.update(cx, |g, cx| {
        let s = g.scroll() + 37.0;
        g.set_scroll(s, cx)
    });
    frames(cx, 2);
    let (top, y) = cx.update(|_, cx| {
        let g = grid.read(cx);
        let top = g.top_index(cx).unwrap();
        (
            model.read(cx).index.id(top).unwrap(),
            g.cell_center(top, cx).y,
        )
    });
    // 1,000 items with earlier dates arrive.
    {
        let mut list = source.list.lock().unwrap();
        let mut keys: Vec<SortKey> = (0..1_000u32)
            .map(|i| key_for(200_000 + i, Y2000_MS - i64::from(i + 1) * DAY_MS))
            .collect();
        keys.reverse();
        keys.extend(list.keys.iter().copied());
        list.ids = keys.iter().map(SortKey::item).collect();
        list.keys = keys;
    }
    grid.update(cx, |g, cx| g.refresh_keeping_anchor(cx));
    frames(cx, 3);
    cx.update(|_, cx| {
        let g = grid.read(cx);
        let m = model.read(cx);
        assert_eq!(m.len(), 6_000);
        let now_top = g.top_index(cx).unwrap();
        assert_eq!(
            m.index.id(now_top),
            Some(top),
            "the same item is at the top"
        );
        assert!(
            (g.cell_center(now_top, cx).y - y).abs() < px(0.5),
            "at the same height"
        );
    });
}

#[gpui_kit::test]
fn scenario_gallery_scrubber_jumps_to_year(cx: &mut TestAppContext) {
    // One photo every 3 days from 2005 to 2015.
    let base = ps_model::days_from_civil(2005, 1, 1) * DAY_MS;
    let source = Arc::new(FakeSource::with_dates(1_340, |i| {
        base + i64::from(i) * 3 * DAY_MS + 3_600_000
    }));
    let (grid, model, cx) = open_grid(cx, source);
    frames(cx, 2);
    let p = cx
        .update(|_, cx| grid.read(cx).scrubber_point(2012, cx))
        .expect("2012 on the scrubber");
    cx.simulate_click(p, Modifiers::default());
    frames(cx, 2);
    let start_2012 = ps_model::days_from_civil(2012, 1, 1) * DAY_MS;
    let expected = (0..1_340u32)
        .find(|i| base + i64::from(*i) * 3 * DAY_MS >= start_2012)
        .unwrap();
    cx.update(|_, cx| {
        let first = grid.read(cx).visible_range(cx).start;
        assert_eq!(model.read(cx).index.id(first), Some(ItemId(expected)));
    });
}
