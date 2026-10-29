//! Damaged files in the app (`damaged-raw-reason`, ADR-0115): the run summary's damaged count and warning with
//! Show, and the Reports view's damaged line drilling into the gallery. Catalogs are built from fixture files in
//! temp dirs by a real engine.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_fixtures::dng::{Cut, cut};
use ps_model::{ItemId, NoPreviewFilter, PreviewReason, SourceId};
use ps_service::scan::{ScanOptions, ScanService};
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::ViewId;
use ui_workspace::views::library::LibraryHub;

struct Data {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    card: SourceId,
}

/// A card with `cut_cr2s` cut CR2s and three intact JPEGs, plus a second source with one more cut CR2 (never
/// shown for the card); scanned, and exported when `export`.
fn scanned(cut_cr2s: u64, export: bool) -> Data {
    let tmp = tempfile::tempdir().unwrap();
    let write = |root: &Path, rel: &str, b: &[u8]| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    };
    let card = tmp.path().join("card");
    for i in 0..cut_cr2s {
        let shot = ps_fixtures::dng::converted_burst_frame(100 + i, 0, 0, 960);
        write(
            &card,
            &format!("DCIM/100CANON/IMG_{:04}.CR2", 34 + i),
            &cut(&shot.cr2, Cut::AfterPreview).unwrap(),
        );
    }
    for i in 0..3u64 {
        write(
            &card,
            &format!("DCIM/100CANON/IMG_{:04}.JPG", 200 + i),
            &ps_fixtures::media::jpeg(800, 600, 300 + i, 85),
        );
    }
    let other = tmp.path().join("other");
    let shot = ps_fixtures::dng::converted_burst_frame(150, 0, 0, 960);
    write(
        &other,
        "DCIM/IMG_9000.CR2",
        &cut(&shot.cr2, Cut::InPreview).unwrap(),
    );
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let card_id = svc.add_source(&card).unwrap().source.id;
    let other_id = svc.add_source(&other).unwrap().source.id;
    // The other source first, so the card's run is the last one.
    svc.start(&[other_id]).unwrap();
    svc.wait().unwrap();
    svc.start(&[card_id]).unwrap();
    let s = svc.wait().unwrap();
    assert_eq!(s.damaged, cut_cr2s);
    if export {
        svc.exports()
            .run(
                &tmp.path().join("Library"),
                ps_service::export::profile_settings("google").unwrap(),
                &Default::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
    }
    Data {
        _tmp: tmp,
        data,
        card: card_id,
    }
}

fn open_app(
    cx: &mut TestAppContext,
    data: &Path,
) -> (Arc<CatalogBackend>, Entity<Shell>, VisualTestContext) {
    let svc = ScanService::open(data, ScanOptions::default()).unwrap();
    let backend = Arc::new(CatalogBackend::with_service(data, svc));
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    // Tall enough that the whole summary is painted (headless tests see painted elements only).
    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(1440.0), gpui_kit::px(2400.0)));
    (backend, shell, vcx)
}

fn show(shell: &Entity<Shell>, view: ViewId, cx: &mut VisualTestContext) {
    let shell = shell.clone();
    cx.update(|w, cx| shell.update(cx, |s, cx| s.show_view(view, w, cx)));
    cx.run_until_parked();
}

/// Like `wait_until`, with real time for the engine's threads as well.
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

/// The items of the card's cut CR2s.
fn damaged_items(backend: &CatalogBackend, source: SourceId) -> Vec<ItemId> {
    let r = backend.scan().unwrap().engine().catalog().read();
    let mut v: Vec<ItemId> = r
        .source_files(source, None, 1000)
        .unwrap()
        .into_iter()
        .filter(|f| {
            f.preview
                .is_some_and(|p| p.reason == PreviewReason::Unreadable)
        })
        .map(|f| ItemId::try_from(f.id).unwrap())
        .collect();
    v.sort();
    v
}

/// Wait until the Library lists exactly `want`, filtered to the damaged files of `source`.
fn library_shows(cx: &mut VisualTestContext, want: &[ItemId], source: SourceId) {
    let n = want.len();
    wait_long(cx, "the filtered list", move |cx| {
        cx.update(|_, cx| {
            LibraryHub::library(cx).is_some_and(|l| {
                let m = l.read(cx).model().read(cx);
                m.state == ui_gallery::model::ListState::Ready && m.len() == n
            })
        })
    });
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let mut ids: Vec<ItemId> = cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
    ids.sort();
    assert_eq!(ids, want, "exactly the damaged files");
    let f = cx.update(|_, cx| lib.read(cx).filters.clone());
    assert_eq!(
        f.no_preview,
        Some(NoPreviewFilter {
            reason: Some(PreviewReason::Unreadable),
            format: None,
        })
    );
    assert_eq!(f.sources, Some(vec![source]));
}

#[gpui_kit::test]
fn scenario_scan_progress_dashboard_warns_about_damaged_files(cx: &mut TestAppContext) {
    let d = scanned(2, false);
    let (backend, shell, mut cx) = open_app(cx, &d.data);
    show(&shell, ViewId::Sources, &mut cx);
    wait_long(&mut cx, "the notice", |cx| {
        exists("summary-damaged-notice", cx)
    });
    assert_eq!(
        label("summary-damaged", &mut cx).as_deref(),
        Some("damaged 2")
    );
    let text = label("summary-damaged-notice", &mut cx).unwrap();
    assert!(
        text.starts_with("2 damaged files could not be read"),
        "{text}"
    );
    assert!(text.contains("keep the original card or drive"), "{text}");
    click("summary-damaged-show", &mut cx);
    let s = shell.clone();
    wait_long(&mut cx, "the Library", move |cx| {
        cx.update(|_, cx| s.read(cx).active_view() == ViewId::Library)
    });
    let want = damaged_items(&backend, d.card);
    assert_eq!(want.len(), 2);
    library_shows(&mut cx, &want, d.card);
}

#[gpui_kit::test]
fn scenario_completion_report_reports_view_drills_into_damaged_files(cx: &mut TestAppContext) {
    let d = scanned(1, true);
    let (backend, shell, mut cx) = open_app(cx, &d.data);
    show(&shell, ViewId::Reports, &mut cx);
    let line = format!("report-damaged-{}", d.card.0);
    wait_long(&mut cx, "the report", |cx| exists(&line, cx));
    let text = label(&line, &mut cx).unwrap();
    assert!(text.starts_with("1 damaged file: copied as is"), "{text}");
    assert_eq!(
        label(&format!("report-badge-{}", d.card.0), &mut cx).as_deref(),
        Some("100% accounted for, 1 damaged")
    );
    click(&format!("report-damaged-show-{}", d.card.0), &mut cx);
    let s = shell.clone();
    wait_long(&mut cx, "the Library", move |cx| {
        cx.update(|_, cx| s.read(cx).active_view() == ViewId::Library)
    });
    let want = damaged_items(&backend, d.card);
    assert_eq!(want.len(), 1, "the cut CR2");
    library_shows(&mut cx, &want, d.card);
}
