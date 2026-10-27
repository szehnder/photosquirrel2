//! Headless CPU cost of a gallery frame while flinging through 1,000,000 items with warm textures: the
//! whole window's layout, prepaint and paint per frame on the test platform (no GPU). A proxy for the
//! windowed `photosquirrel --render-bench`, which needs a focused window on a real display.
//!
//! `cargo test --release -p ui-gallery --test render_cost -- --ignored --nocapture` (a timing test: `cargo xtask timing`)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext, px, size};
use ui_gallery::bench::{Percentiles, SyntheticSource, fling_path};
use ui_gallery::grid::CellPaint;
use ui_gallery::model::{TexKey, render_image};
use ui_gallery::source::{GalleryQuery, ThumbKey};
use ui_gallery::{GalleryGrid, GalleryModel};

#[gpui_kit::test]
// A wall-clock budget: run alone, after the other tests, by `cargo xtask timing` (part of `cargo xtask ci`).
#[ignore = "timing"]
fn gallery_frame_cpu_cost_flinging_1m(cx: &mut TestAppContext) {
    let p95 = fling(cx, 0);
    println!("measure gallery-frame-cpu-p95={p95:.3} ms");
}

/// The same fling with 30% of the items drawn as placeholder tiles (ADR-0109, ADR-0110): the same bound.
#[gpui_kit::test]
#[ignore = "timing"]
fn gallery_frame_cpu_cost_flinging_1m_with_placeholders(cx: &mut TestAppContext) {
    let p95 = fling(cx, 30);
    println!("gallery headless frame p95 with 30% placeholder tiles: {p95:.3} ms");
}

/// Fling through 1M items with `placeholders` percent placeholder tiles; the frame p95 (ms).
fn fling(cx: &mut TestAppContext, placeholders: u32) -> f32 {
    support::init(cx);
    let source = Arc::new(SyntheticSource::new(1_000_000, 3_000).with_placeholders(placeholders));
    let s2 = source.clone();
    let model = cx.new(move |cx| {
        let mut m = GalleryModel::new(s2.clone(), GalleryQuery::default());
        m.apply_list(s2.list(), cx);
        for n in 0..3_000u64 {
            let img = render_image(s2.bgra(n, 192)).unwrap();
            m.insert_texture(
                TexKey {
                    thumb: ThumbKey::synthetic(n),
                    level: 1,
                },
                img,
            );
        }
        m
    });
    let m2 = model.clone();
    let (grid, cx) = cx.add_window_view(move |_, cx| {
        let mut g = GalleryGrid::new(m2, cx);
        g.set_cell(150.0, None, cx);
        g
    });
    cx.simulate_resize(size(px(1600.0), px(1000.0)));
    support::frames(cx, 3);
    let frames = 1_200;
    let mut times = Vec::with_capacity(frames);
    let (mut cells, mut tiles) = (0usize, 0usize);
    for f in 0..frames {
        let t = f as f64 / frames as f64;
        grid.update(cx, |g, cx| g.scroll_to_fraction(fling_path(t), cx));
        let start = Instant::now();
        cx.update(|window, cx| window.render_frame(cx));
        times.push(start.elapsed().as_secs_f32() * 1000.0);
        cx.update(|_, cx| {
            let painted = grid.read(cx).painted_cells();
            // Cells whose details have loaded (a bare tile could be either).
            cells += painted
                .iter()
                .filter(|(_, p)| !matches!(p, CellPaint::Tile))
                .count();
            tiles += painted
                .iter()
                .filter(|(_, p)| matches!(p, CellPaint::PlaceholderTile { .. }))
                .count();
        });
        cx.run_until_parked();
    }
    // The fling really drew placeholder tiles, in about the share asked for (of the cells whose details had
    // loaded).
    let share = tiles as f32 / cells.max(1) as f32;
    println!(
        "placeholder tiles: {tiles} of {cells} cells with details ({:.1}%)",
        share * 100.0
    );
    if placeholders == 0 {
        assert_eq!(tiles, 0, "no placeholder tiles without placeholders");
    } else {
        let want = placeholders as f32 / 100.0;
        assert!(
            tiles > 0 && share >= want * 0.5 && share <= want * 1.5,
            "{tiles} placeholder tiles of {cells} cells, {placeholders}% asked for"
        );
    }
    let p = Percentiles::of(&times[100..]);
    let paint = Percentiles::of(&cx.update(|_, cx| grid.read(cx).stats.paint_ms.clone()));
    println!(
        "gallery headless frame ms (layout+prepaint+paint, 1M items, warm textures, {placeholders}% placeholders): p50={:.3} p95={:.3} max={:.3}; grid paint p95={:.3}",
        p.p50, p.p95, p.max, paint.p95
    );
    if !cfg!(debug_assertions) {
        assert!(
            p.p95 <= 8.3,
            "headless frame p95 {:.3} ms exceeds 8.3 ms",
            p.p95
        );
    }
    p.p95
}
