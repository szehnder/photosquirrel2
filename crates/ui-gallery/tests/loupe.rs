//! Headless tests of the loupe (spec `gallery`: Loupe, Offline photos browsable).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext, px, size,
};
use ps_model::ItemId;
use support::*;
use ui_gallery::loupe::{LoupeEvent, Shown};
use ui_gallery::source::GalleryQuery;
use ui_gallery::{GalleryModel, LoupeView};

fn open_loupe(
    cx: &mut TestAppContext,
    source: FakeSource,
) -> (
    Entity<LoupeView>,
    Entity<GalleryModel>,
    &mut VisualTestContext,
) {
    init(cx);
    let model = cx.new(|cx| {
        let mut m = GalleryModel::new(Arc::new(source), GalleryQuery::default());
        m.set_query(GalleryQuery::default(), cx);
        m
    });
    let m2 = model.clone();
    let (loupe, vcx) = cx.add_window_view(move |_, cx| LoupeView::new(m2, cx));
    vcx.simulate_resize(size(px(1400.0), px(900.0)));
    frames(vcx, 2);
    (loupe, model, vcx)
}

#[gpui_kit::test]
fn scenario_gallery_loupe_opens_selected_photo(cx: &mut TestAppContext) {
    let (loupe, model, cx) = open_loupe(cx, FakeSource::new(50));
    loupe.update(cx, |l, cx| l.open(3, cx));
    frames(cx, 4);
    cx.update(|window, cx| {
        let l = loupe.read(cx);
        let Shown::Preview {
            width,
            height,
            fitted,
        } = l.shown().clone()
        else {
            panic!("expected the preview, got {:?}", l.shown());
        };
        assert_eq!((width, height), (2048, 1365), "the 2048 px preview");
        let vp = window.viewport_size();
        assert!(
            fitted.size.width <= vp.width && fitted.size.height <= vp.height,
            "fits the window"
        );
        // Fitted: it touches the photo area on one axis.
        assert!(fitted.size.width > vp.width * 0.8 || fitted.size.height > vp.height * 0.6);
        assert!(model.read(cx).selection.contains(ItemId(3)));
    });
}

#[gpui_kit::test]
fn scenario_gallery_loupe_returns_to_same_position(cx: &mut TestAppContext) {
    let (loupe, model, cx) = open_loupe(cx, FakeSource::new(50));
    let closed = Rc::new(RefCell::new(None));
    let c2 = closed.clone();
    cx.update(|_, cx| {
        cx.subscribe(&loupe, move |_, e: &LoupeEvent, _| {
            if let LoupeEvent::Close(i) = e {
                *c2.borrow_mut() = Some(*i);
            }
        })
        .detach();
    });
    loupe.update(cx, |l, cx| l.open(10, cx));
    cx.update(|window, cx| window.focus(&loupe.focus_handle(cx), cx));
    frames(cx, 1);
    cx.simulate_keystrokes("right right right right right");
    frames(cx, 1);
    cx.simulate_keystrokes("escape");
    frames(cx, 1);
    assert_eq!(*closed.borrow(), Some(15));
    cx.update(|_, cx| {
        let m = model.read(cx);
        assert_eq!(m.selection.focus, Some(15));
        assert_eq!(m.selected_ids(10), vec![ItemId(15)]);
    });
}

#[gpui_kit::test]
fn scenario_gallery_offline_photo_shows_thumbnail(cx: &mut TestAppContext) {
    let mut source = FakeSource::new(20);
    source.offline.insert(ItemId(7));
    let (loupe, _, cx) = open_loupe(cx, source);
    loupe.update(cx, |l, cx| l.open(7, cx));
    frames(cx, 6);
    assert_eq!(
        cx.update(|_, cx| loupe.read(cx).shown().clone()),
        Shown::Thumbnail {
            notice: "Source offline".into()
        }
    );
}
