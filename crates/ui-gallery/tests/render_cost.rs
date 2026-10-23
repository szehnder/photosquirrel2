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
use ui_gallery::model::{TexKey, render_image};
use ui_gallery::source::{GalleryQuery, ThumbKey};
use ui_gallery::{GalleryGrid, GalleryModel};

#[gpui_kit::test]
// A wall-clock budget: run alone, after the other tests, by `cargo xtask timing` (part of `cargo xtask ci`).
#[ignore = "timing"]
fn gallery_frame_cpu_cost_flinging_1m(cx: &mut TestAppContext) {
    support::init(cx);
    let source = Arc::new(SyntheticSource::new(1_000_000, 3_000));
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
    for f in 0..frames {
        let t = f as f64 / frames as f64;
        grid.update(cx, |g, cx| g.scroll_to_fraction(fling_path(t), cx));
        let start = Instant::now();
        cx.update(|window, cx| window.render_frame(cx));
        times.push(start.elapsed().as_secs_f32() * 1000.0);
        cx.run_until_parked();
    }
    let p = Percentiles::of(&times[100..]);
    let paint = Percentiles::of(&cx.update(|_, cx| grid.read(cx).stats.paint_ms.clone()));
    println!(
        "gallery headless frame ms (layout+prepaint+paint, 1M items, warm textures): p50={:.3} p95={:.3} max={:.3}; grid paint p95={:.3}",
        p.p50, p.p95, p.max, paint.p95
    );
    println!("measure gallery-frame-cpu-p95={:.3} ms", p.p95);
    if !cfg!(debug_assertions) {
        assert!(
            p.p95 <= 8.3,
            "headless frame p95 {:.3} ms exceeds 8.3 ms",
            p.p95
        );
    }
}
