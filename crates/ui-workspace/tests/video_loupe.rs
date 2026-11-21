//! Videos in the loupe and the inspector (video-previews-and-play, ADR-0134): the large poster frame with a play
//! mark and its duration, the placeholder without a poster tool, and Open in player through the recording opener.
//! The poster tool is a shell script; catalogs are built from fixture MOVs by a real engine.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, SharedString, TestAppContext, VisualTestContext, px, size};
use ps_model::{ItemId, RunStatus};
use ps_service::scan::{ScanOptions, ScanService};
use support::*;
use ui_gallery::loupe::{LoupeView, Shown};
use ui_workspace::gallery_inspector::{GalleryInspector, OpenerGlobal, RecordingOpener};
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::library::{LibraryHub, LibraryView};
use ui_workspace::views::{LibraryMode, ViewId};

/// A stand-in for ffmpeg printing a 1920x1080 JPEG.
fn fake_ffmpeg(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let frame = dir.join("frame.jpg");
    std::fs::write(&frame, ps_fixtures::media::jpeg(1920, 1080, 9, 85)).unwrap();
    let tool = dir.join("ffmpeg");
    std::fs::write(&tool, format!("#!/bin/sh\ncat '{}'\n", frame.display())).unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    tool
}

struct Data {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    tool: PathBuf,
}

/// One source with an iPhone MOV (12.5 s) and a JPEG, scanned with or without the poster tool.
fn scanned(with_tool: bool) -> Data {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("phone");
    std::fs::create_dir_all(root.join("DCIM")).unwrap();
    let mov = ps_fixtures::media::movie(&ps_fixtures::media::MovieSpec::iphone());
    std::fs::write(root.join("DCIM/IMG_5000.MOV"), mov).unwrap();
    std::fs::write(
        root.join("DCIM/IMG_0100.JPG"),
        ps_fixtures::media::jpeg(800, 600, 40, 85),
    )
    .unwrap();
    let tools = tmp.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let tool = fake_ffmpeg(&tools);
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, options(with_tool.then(|| tool.clone()))).unwrap();
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    assert_eq!(svc.wait().unwrap().status, RunStatus::Completed);
    Data {
        _tmp: tmp,
        root,
        data,
        tool,
    }
}

/// Options with exactly this poster tool: it stands for this (injected) machine's `PATH`, whatever the real one
/// has, and no tool is configured.
fn options(tool: Option<PathBuf>) -> ScanOptions {
    let t = tool.clone();
    ScanOptions {
        poster_tool: tool,
        poster_tool_default: Some(Arc::new(move || t.clone())),
        ..ScanOptions::default()
    }
}

fn open_app(
    cx: &mut TestAppContext,
    d: &Data,
    tool: Option<PathBuf>,
) -> (Arc<CatalogBackend>, Arc<RecordingOpener>, VisualTestContext) {
    let svc = ScanService::open(&d.data, options(tool)).unwrap();
    let backend = Arc::new(CatalogBackend::with_service(&d.data, svc));
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    vcx.simulate_resize(size(px(1440.0), px(1000.0)));
    let opener = Arc::new(RecordingOpener::default());
    let o2 = opener.clone();
    vcx.update(|_, cx| cx.set_global(OpenerGlobal(o2)));
    let s = shell.clone();
    vcx.update(|w, cx| s.update(cx, |s, cx| s.show_view(ViewId::Library, w, cx)));
    vcx.run_until_parked();
    (backend, opener, vcx)
}

/// Like `wait_until`, with real time for the engine's and the tool's threads as well.
fn wait_long(
    cx: &mut VisualTestContext,
    what: &str,
    mut cond: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if cond(cx) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(Duration::from_millis(20));
    }
}

fn item(backend: &CatalogBackend, suffix: &str) -> ItemId {
    let r = backend.scan().unwrap().engine().catalog().read();
    let id = (1..50)
        .map(ps_model::FileId)
        .find(|f| {
            r.file_by_id(*f)
                .ok()
                .flatten()
                .is_some_and(|x| String::from_utf8_lossy(x.key.path.as_bytes()).ends_with(suffix))
        })
        .unwrap_or_else(|| panic!("no file {suffix}"));
    ItemId::try_from(id).unwrap()
}

fn library(cx: &mut VisualTestContext) -> Entity<LibraryView> {
    wait_long(cx, "the library list", |cx| {
        cx.update(|_, cx| {
            LibraryHub::library(cx).is_some_and(|l| {
                let m = l.read(cx).model().read(cx);
                m.state == ui_gallery::model::ListState::Ready && !m.is_empty()
            })
        })
    });
    cx.update(|_, cx| LibraryHub::library(cx).unwrap())
}

/// Select `id`, wait for its summary, open the loupe on it and focus it.
fn loupe_on(
    lib: &Entity<LibraryView>,
    id: ItemId,
    cx: &mut VisualTestContext,
) -> Entity<LoupeView> {
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(id))
        .expect("listed");
    let m = model.clone();
    wait_long(cx, "the summary", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            m.read(cx).summary(index).is_some()
        })
    });
    model.update(cx, |m, cx| {
        let ids = m.index.ids.clone();
        m.selection.click(&ids, index);
        m.selection_changed(cx);
    });
    let l = lib.clone();
    cx.update(|w, cx| l.update(cx, |l, cx| l.set_mode(LibraryMode::Loupe, w, cx)));
    cx.run_until_parked();
    let loupe = cx.update(|_, cx| lib.read(cx).loupe().clone());
    // Already in the loupe: move it to this item.
    loupe.update(cx, |l, cx| l.open(index, cx));
    cx.update(|w, cx| {
        let f = loupe.read(cx).focus_handle().clone();
        w.focus(&f, cx);
    });
    loupe
}

fn settled(loupe: &Entity<LoupeView>, cx: &mut VisualTestContext) -> Shown {
    let l = loupe.clone();
    wait_long(cx, "the loupe's content", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            !matches!(l.read(cx).shown(), Shown::Loading | Shown::Nothing)
        })
    });
    cx.update(|_, cx| loupe.read(cx).shown().clone())
}

#[gpui_kit::test]
fn scenario_gallery_video_shown_large_in_the_loupe(cx: &mut TestAppContext) {
    let d = scanned(true);
    let (backend, _, mut cx) = open_app(cx, &d, Some(d.tool.clone()));
    let lib = library(&mut cx);
    let loupe = loupe_on(&lib, item(&backend, "IMG_5000.MOV"), &mut cx);
    let Shown::Preview { width, height, .. } = settled(&loupe, &mut cx) else {
        panic!(
            "no large preview: {:?}",
            cx.update(|_, cx| loupe.read(cx).shown().clone())
        );
    };
    assert_eq!(
        (width, height),
        (1920, 1080),
        "the large poster frame, not the 384 px thumbnail"
    );
    let (mark, notice, video) = cx.update(|w, cx| {
        w.render_frame(cx);
        let l = loupe.read(cx);
        (
            l.video_mark().cloned(),
            l.notice().map(str::to_owned),
            l.showing_video(),
        )
    });
    let mark = mark.expect("a play mark");
    assert_eq!(mark.duration.as_deref(), Some("0:12"));
    assert_eq!(notice, None, "no 'unavailable' notice");
    assert!(video);
    assert!(exists("loupe-open-in-player", &mut cx));
}

#[gpui_kit::test]
fn scenario_gallery_video_without_a_poster_tool_in_the_loupe(cx: &mut TestAppContext) {
    let d = scanned(false);
    let (backend, _, mut cx) = open_app(cx, &d, None);
    let lib = library(&mut cx);
    let loupe = loupe_on(&lib, item(&backend, "IMG_5000.MOV"), &mut cx);
    let Shown::Placeholder { reason, .. } = settled(&loupe, &mut cx) else {
        panic!(
            "no placeholder: {:?}",
            cx.update(|_, cx| loupe.read(cx).shown().clone())
        );
    };
    assert!(
        reason.to_lowercase().contains("poster-frame tool"),
        "{reason}"
    );
    assert!(cx.update(|_, cx| loupe.read(cx).notice().is_none()));
    assert!(
        exists("loupe-open-in-player", &mut cx),
        "Open in player is still offered"
    );
}

#[gpui_kit::test]
fn scenario_gallery_open_in_player_from_the_loupe(cx: &mut TestAppContext) {
    let d = scanned(true);
    let (backend, opener, mut cx) = open_app(cx, &d, Some(d.tool.clone()));
    let lib = library(&mut cx);
    let mov = d.root.join("DCIM/IMG_5000.MOV");
    let before = std::fs::metadata(&mov).unwrap().modified().unwrap();
    let bytes = std::fs::read(&mov).unwrap();
    let loupe = loupe_on(&lib, item(&backend, "IMG_5000.MOV"), &mut cx);
    settled(&loupe, &mut cx);
    cx.simulate_keystrokes("o");
    let o = opener.clone();
    wait_long(&mut cx, "the open request", move |_| {
        !o.opened.lock().unwrap().is_empty()
    });
    let opened = opener.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(
        std::fs::canonicalize(&opened[0]).unwrap(),
        std::fs::canonicalize(&mov).unwrap()
    );
    assert_eq!(std::fs::metadata(&mov).unwrap().modified().unwrap(), before);
    assert_eq!(
        std::fs::read(&mov).unwrap(),
        bytes,
        "the file is not modified"
    );
    // A photo offers nothing to open.
    let photo = item(&backend, "IMG_0100.JPG");
    let loupe = loupe_on(&lib, photo, &mut cx);
    settled(&loupe, &mut cx);
    assert!(!exists("loupe-open-in-player", &mut cx));
    cx.simulate_keystrokes("o");
    cx.run_until_parked();
    assert_eq!(opener.opened.lock().unwrap().len(), 1);
}

/// Select `id` and wait for the inspector's detail.
fn inspect(
    lib: &Entity<LibraryView>,
    id: ItemId,
    cx: &mut VisualTestContext,
) -> Entity<GalleryInspector> {
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(id))
        .expect("listed");
    model.update(cx, |m, cx| {
        let ids = m.index.ids.clone();
        m.selection.click(&ids, index);
        m.selection_changed(cx);
    });
    wait_long(cx, "the inspector", |cx| {
        cx.update(|_, cx| LibraryHub::inspector(cx).is_some())
    });
    let insp = cx.update(|_, cx| LibraryHub::inspector(cx).unwrap());
    let i2 = insp.clone();
    wait_long(cx, "the item detail", move |cx| {
        cx.update(|_, cx| i2.read(cx).detail().is_some_and(|d| d.id == id))
    });
    insp
}

/// Scroll the inspector until `id` is on screen.
fn reveal_in_inspector(id: &str, cx: &mut VisualTestContext) {
    let sid = SharedString::from(id.to_owned());
    for _ in 0..40 {
        let visible = cx.update(|window, cx| {
            window.render_frame(cx);
            let inside = window.try_find("inspector-scroll").map(|s| s.bounds());
            window.try_find(sid.clone()).is_some_and(|s| {
                s.visible()
                    && inside.is_some_and(|b| {
                        s.bounds().top() >= b.top() && s.bounds().bottom() <= b.bottom()
                    })
            })
        });
        if visible {
            return;
        }
        let at = bounds("inspector-scroll", cx).center();
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: at,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-120.0))),
            ..Default::default()
        });
    }
    panic!("{id} never came into view");
}

#[gpui_kit::test]
fn scenario_photo_inspector_open_a_video_copy_in_the_player(cx: &mut TestAppContext) {
    let d = scanned(true);
    let (backend, opener, mut cx) = open_app(cx, &d, Some(d.tool.clone()));
    let lib = library(&mut cx);
    let insp = inspect(&lib, item(&backend, "IMG_5000.MOV"), &mut cx);
    let path = cx.update(|_, cx| insp.read(cx).detail().unwrap().copies[0].path.clone());
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    reveal_in_inspector("inspector-open-in-player-0", &mut cx);
    assert_eq!(
        label("inspector-open-in-player-0", &mut cx).as_deref(),
        Some("Open in player")
    );
    click("inspector-open-in-player-0", &mut cx);
    assert_eq!(*opener.opened.lock().unwrap(), vec![PathBuf::from(&path)]);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    // A photo's copies have no such button.
    let _ = inspect(&lib, item(&backend, "IMG_0100.JPG"), &mut cx);
    cx.update(|w, cx| w.render_frame(cx));
    assert!(!exists("inspector-open-in-player-0", &mut cx));
}

#[gpui_kit::test]
fn scenario_photo_inspector_offline_video_copy(cx: &mut TestAppContext) {
    let d = scanned(true);
    // The drive is unplugged: its folder is gone.
    let moved = d.root.with_file_name("phone-unplugged");
    std::fs::rename(&d.root, &moved).unwrap();
    ScanService::open(&d.data, options(None))
        .unwrap()
        .refresh_sources()
        .unwrap();
    let (backend, opener, mut cx) = open_app(cx, &d, Some(d.tool.clone()));
    let lib = library(&mut cx);
    let insp = inspect(&lib, item(&backend, "IMG_5000.MOV"), &mut cx);
    assert!(cx.update(|_, cx| !insp.read(cx).detail().unwrap().copies[0].online));
    reveal_in_inspector("inspector-open-in-player-0", &mut cx);
    assert_eq!(
        label("inspector-open-in-player-0", &mut cx).as_deref(),
        Some("Open in player (source offline)")
    );
    // The tooltip gives the reason.
    let at = bounds("inspector-open-in-player-0", &mut cx).center();
    cx.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
    wait_long(&mut cx, "the tooltip", |cx| {
        exists("inspector-open-in-player-0-tip", cx)
    });
    click("inspector-open-in-player-0", &mut cx);
    cx.run_until_parked();
    assert!(opener.opened.lock().unwrap().is_empty(), "disabled");
}
