//! `photosquirrel --render-bench [SECONDS]` (ADR-0026, gallery budget `gallery-scroll-p95`): a bare window with
//! the gallery grid over a synthetic 1,000,000-item list whose 3,000 distinct thumbnails are decoded and
//! uploaded before timing (warm textures), then a scripted fling. Prints the report and quits.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use ui_gallery::bench::{FlingBench, SyntheticSource};
use ui_gallery::model::{TexKey, render_image};
use ui_gallery::source::GalleryQuery;
use ui_gallery::{GalleryGrid, GalleryModel};

pub const ITEMS: usize = 1_000_000;
pub const DISTINCT: usize = 3_000;
/// Window size of the benchmark (logical px).
const WINDOW: [f32; 2] = [1600.0, 1000.0];
/// Cells small enough to use half-size (192 px) textures, so all 3,000 fit the 512 MB budget.
const CELL: f32 = 150.0;

pub fn run(seconds: f32, placeholder_percent: u32) {
    gpui_kit::application()
        .with_assets(ui_workspace::assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui_theme::init(cx);
            ui_gallery::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(WINDOW[0]), px(WINDOW[1])),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("PhotoSquirrel render bench".into()),
                    ..Default::default()
                }),
                app_id: Some("photosquirrel".into()),
                ..Default::default()
            };
            let opened = cx.open_window(options, move |_, cx| {
                let source =
                    Arc::new(SyntheticSource::new(ITEMS, DISTINCT).with_placeholders(placeholder_percent));
                let model = cx.new(|cx| {
                    let mut m = GalleryModel::new(source.clone(), GalleryQuery::default());
                    m.apply_list(source.list(), cx);
                    for n in 0..DISTINCT as u64 {
                        if let Some(img) = render_image(source.bgra(n, 192)) {
                            m.insert_texture(
                                TexKey {
                                    thumb: ui_gallery::source::ThumbKey::synthetic(n),
                                    level: 1,
                                },
                                img,
                            );
                        }
                    }
                    m
                });
                cx.new(|cx| {
                    let mut grid = GalleryGrid::new(model, cx);
                    grid.set_cell(CELL, None, cx);
                    grid.start_bench(FlingBench::new(Duration::from_secs_f32(seconds)), |report, cx| {
                        println!("{report}");
                        cx.quit();
                    });
                    grid
                })
            });
            match opened {
                Ok(handle) => {
                    // Watchdog: a hidden or occluded window gets no frames (Wayland); report what was
                    // recorded instead of waiting forever.
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs_f32(seconds + 20.0))
                            .await;
                        let _ = handle.update(cx, |grid, _, cx| {
                            eprintln!(
                                "render-bench: frames stalled ({} recorded); is the window visible and focused?",
                                grid.stats.paint_ms.len()
                            );
                            // No `frame interval ms` line: the measurement is invalid, not fast.
                            println!("gallery render-bench stalled: no frame timing (frames={})", grid.stats.paint_ms.len());
                            cx.quit();
                        });
                    })
                    .detach();
                }
                Err(e) => {
                    log::error!("failed to open the bench window: {e:#}");
                    cx.quit();
                }
            }
        });
}
