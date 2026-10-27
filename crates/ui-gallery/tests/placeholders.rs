//! Headless tests of placeholder tiles (`preview-placeholders`, ADR-0109): an item without a preview is drawn as
//! a placeholder tile with its format, "No preview" and a status mark, at every tile size, in the grid and the
//! loupe; the info button asks for the "Why no preview?" dialog without changing the selection.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{AppContext as _, Modifiers, TestAppContext, px, size};
use ps_model::{ItemId, PlaceholderStatus as S, PreviewReason as R};
use support::*;
use ui_gallery::grid::CellPaint;
use ui_gallery::loupe::{LoupeEvent, Shown};
use ui_gallery::model::GalleryEvent;
use ui_gallery::placeholder::{Lod, NO_PREVIEW};
use ui_gallery::source::GalleryQuery;
use ui_gallery::theme::MIN_CELL;
use ui_gallery::{GalleryModel, LoupeView};

fn source() -> FakeSource {
    let mut s = FakeSource::new(40);
    s.placeholders.insert(
        ItemId(0),
        badge("TIFF", R::UnsupportedVariant, S::NotSupported),
    );
    s.placeholders
        .insert(ItemId(1), badge("HEIC", R::DecoderMissing, S::NeedsLibrary));
    s.placeholders
        .insert(ItemId(2), badge("JPEG", R::Unreadable, S::Damaged));
    s.placeholders
        .insert(ItemId(3), badge("TIFF", R::Unreadable, S::ReadyToFill));
    s
}

#[gpui_kit::test]
fn scenario_gallery_tiff_without_a_preview_shows_a_placeholder_tile(cx: &mut TestAppContext) {
    let (grid, _model, cx) = open_grid(cx, Arc::new(source()));
    frames(cx, 3);
    cx.update(|_, cx| {
        let g = grid.read(cx);
        assert_eq!(
            g.painted(0),
            Some(CellPaint::PlaceholderTile {
                status: S::NotSupported,
                lod: Lod::Full
            }),
            "a placeholder tile, no flat fill"
        );
        let (_, parts) = g
            .painted_placeholders()
            .iter()
            .find(|(i, _)| *i == 0)
            .expect("the tile's parts");
        assert_eq!(parts.badge.as_ref().map(|b| b.text.as_ref()), Some("TIFF"));
        assert_eq!(
            parts.caption.as_ref().map(|c| c.text.as_ref()),
            Some(NO_PREVIEW)
        );
        assert_eq!(parts.status, S::NotSupported, "the not-supported mark");
        // Each reason gets its own mark.
        for (i, s) in [(1, S::NeedsLibrary), (2, S::Damaged), (3, S::ReadyToFill)] {
            assert!(
                matches!(g.painted(i), Some(CellPaint::PlaceholderTile { status, .. }) if status == s),
                "{i}: {:?}",
                g.painted(i)
            );
        }
        // Items with previews are unaffected.
        assert!(matches!(
            g.painted(5),
            Some(CellPaint::Texture { .. } | CellPaint::Placeholder(_))
        ));
    });
}

#[gpui_kit::test]
fn scenario_gallery_small_tiles_keep_glyph_and_badge(cx: &mut TestAppContext) {
    let (grid, _model, cx) = open_grid(cx, Arc::new(source()));
    grid.update(cx, |g, cx| g.set_cell(MIN_CELL, None, cx));
    frames(cx, 3);
    cx.update(|_, cx| {
        let g = grid.read(cx);
        assert_eq!(g.cell_size(), MIN_CELL, "the smallest setting");
        for (i, parts) in g.painted_placeholders() {
            assert_eq!(parts.lod, Lod::Small, "{i}");
            assert!(parts.caption.is_none(), "{i}: no caption at small sizes");
            let b = parts.badge.as_ref().expect("the format badge");
            // The chip holds the whole label (headless text is 0.6 em per character, wider than real text).
            let natural = 2.0 * ui_gallery::theme::BADGE_PAD
                + ui_gallery::theme::BADGE_PX * 0.6 * b.text.chars().count() as f32;
            assert!(
                f32::from(b.bounds.size.width) >= natural,
                "{i}: {} is not clipped",
                b.text
            );
            let s = parts.surface;
            assert!(
                b.bounds.left() >= s.left()
                    && b.bounds.right() <= s.right()
                    && b.bounds.bottom() <= s.bottom(),
                "{i}: the badge is inside the tile"
            );
            assert!(
                parts.glyph.left() >= s.left() && parts.glyph.bottom() <= s.bottom(),
                "{i}: glyph inside"
            );
        }
        assert!(!g.painted_placeholders().is_empty());
    });
}

#[gpui_kit::test]
fn info_button_on_hover_reports_the_item_and_keeps_the_selection(cx: &mut TestAppContext) {
    let (grid, model, cx) = open_grid(cx, Arc::new(source()));
    let events = Rc::new(RefCell::new(Vec::new()));
    let e2 = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&model, move |_, e: &GalleryEvent, _| {
            e2.borrow_mut().push(e.clone())
        })
        .detach();
    });
    // Select item 5, then hover the TIFF placeholder.
    grid.update(cx, |g, cx| g.click(5, Modifiers::none(), cx));
    let centre = cx.update(|_, cx| grid.read(cx).cell_center(0, cx));
    cx.simulate_mouse_move(centre, None, Modifiers::none());
    frames(cx, 2);
    let info = cx
        .update(|_, cx| grid.read(cx).info_button_center(0))
        .expect("the info button is drawn on hover");
    cx.simulate_click(info, Modifiers::none());
    frames(cx, 1);
    assert!(
        events
            .borrow()
            .contains(&GalleryEvent::PlaceholderInfo(ItemId(0))),
        "{:?}",
        events.borrow()
    );
    let selected = cx.update(|_, cx| model.read(cx).selection.focus);
    assert_eq!(selected, Some(5), "the selection is unchanged");
}

#[gpui_kit::test]
fn scenario_gallery_loupe_shows_the_large_placeholder(cx: &mut TestAppContext) {
    init(cx);
    let model = cx.new(|cx| {
        let mut m = GalleryModel::new(Arc::new(source()), GalleryQuery::default());
        m.set_query(GalleryQuery::default(), cx);
        m
    });
    let m2 = model.clone();
    let (loupe, cx) = cx.add_window_view(move |_, cx| LoupeView::new(m2, cx));
    cx.simulate_resize(size(px(1400.0), px(900.0)));
    frames(cx, 2);
    let events = Rc::new(RefCell::new(Vec::new()));
    let e2 = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&loupe, move |_, e: &LoupeEvent, _| {
            e2.borrow_mut().push(e.clone())
        })
        .detach();
    });
    // The HEIC without a decoder.
    loupe.update(cx, |l, cx| l.open(1, cx));
    frames(cx, 4);
    let shown = cx.update(|_, cx| loupe.read(cx).shown().clone());
    let Shown::Placeholder { reason, info } = shown else {
        panic!("expected the large placeholder, got {shown:?}");
    };
    assert!(reason.contains("No HEIC decoder"), "{reason}");
    cx.simulate_click(info.center(), Modifiers::none());
    frames(cx, 1);
    assert!(
        events
            .borrow()
            .contains(&LoupeEvent::PlaceholderInfo(ItemId(1))),
        "{:?}",
        events.borrow()
    );
}
