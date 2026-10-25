//! Photos waiting for a decoder in the app (ADR-0089, ADR-0091): the run summary's notice and "How to enable HEIC
//! previews", the rescan offered once a decoder is available, the inspector's explanation, and decoder
//! detection that never holds up the window.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::{DecoderEntry, DecoderFormat, DecoderReport, Fix, FixStep, ItemId, SourceId};
use ps_service::scan::{ScanOptions, ScanService};
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::ViewId;

const HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

/// No HEIC decoder, with a reason and a fix as detection reports them.
fn missing() -> Decoders {
    let mut report = DecoderReport::default();
    report.set(DecoderEntry {
        format: DecoderFormat::Heic,
        available: false,
        backend: None,
        version: None,
        reason: Some(
            "no HEIC decoder on this system: the HEIF library (libheif) is not installed".into(),
        ),
        fix: Some(Fix {
            summary: "Install libheif1 and its HEVC plugin".into(),
            steps: vec![
                FixStep {
                    text: "Install libheif and its HEVC decoder plugin".into(),
                    command: Some("sudo apt install libheif1 libheif-plugin-libde265".into()),
                    ..FixStep::default()
                },
                FixStep {
                    text: "Then restart PhotoSquirrel and rescan your sources".into(),
                    ..FixStep::default()
                },
            ],
            restart: true,
        }),
        assembly: None,
    });
    Decoders::from_parts(None, report)
}

fn fake() -> Decoders {
    Decoders::with_heif(Arc::new(FakeHeif::new()))
}

struct Data {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    heic_source: SourceId,
    other_source: SourceId,
}

/// Two sources: three HEIC photos, and a JPEG; scanned without a HEIC decoder.
fn scanned_without_decoder() -> Data {
    let tmp = tempfile::tempdir().unwrap();
    let heics = tmp.path().join("iPhone");
    std::fs::create_dir_all(heics.join("DCIM")).unwrap();
    for i in 0..3u8 {
        let b = ps_fixtures::heif::heif_with_transforms(HEIC, i, None).unwrap();
        std::fs::write(heics.join(format!("DCIM/IMG_{i}.HEIC")), b).unwrap();
    }
    let other = tmp.path().join("Camera");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join("IMG_0001.JPG"),
        ps_fixtures::media::quality_pair(5).1,
    )
    .unwrap();
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default())
        .unwrap()
        .with_decoders(missing());
    let heic_source = svc.add_source(&heics).unwrap().source.id;
    let other_source = svc.add_source(&other).unwrap().source.id;
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    Data {
        _tmp: tmp,
        data,
        heic_source,
        other_source,
    }
}

fn open_app(
    cx: &mut TestAppContext,
    data: &Path,
    decoders: Decoders,
) -> (Arc<CatalogBackend>, Entity<Shell>, VisualTestContext) {
    let svc = ScanService::open(data, ScanOptions::default())
        .unwrap()
        .with_decoders(decoders);
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

/// Like `wait_until`, with real time for the engine's threads (a scan) as well.
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

#[gpui_kit::test]
fn scenario_scan_progress_notice_shown_for_heic_without_a_decoder(cx: &mut TestAppContext) {
    let d = scanned_without_decoder();
    let (_backend, shell, mut cx) = open_app(cx, &d.data, missing());
    show(&shell, ViewId::Sources, &mut cx);
    wait_long(&mut cx, "the notice", |cx| {
        exists("summary-undecoded-heif", cx)
    });
    assert_eq!(
        label("summary-undecoded-heif", &mut cx).as_deref(),
        Some("3 HEIC photos have no preview")
    );
    assert_eq!(
        label("summary-undecoded-how-heif", &mut cx).as_deref(),
        Some("How to enable HEIC previews")
    );
    assert!(!exists("summary-fix-heif", &mut cx), "closed until asked");
    click("summary-undecoded-how-heif", &mut cx);
    wait_long(&mut cx, "the fix", |cx| exists("summary-fix-heif", cx));
    let report = missing();
    let e = report.report().entry(DecoderFormat::Heic).unwrap();
    assert_eq!(
        label("summary-fix-heif-reason", &mut cx),
        e.reason.clone(),
        "the reason from the decoder report"
    );
    let steps = &e.fix.as_ref().unwrap().steps;
    assert_eq!(
        label("summary-fix-heif-step-0", &mut cx).as_deref(),
        Some(
            "Install libheif and its HEVC decoder plugin: sudo apt install libheif1 libheif-plugin-libde265"
        )
    );
    assert!(
        exists("summary-fix-heif-copy-0", &mut cx),
        "a Copy button for the command"
    );
    assert_eq!(
        label("summary-fix-heif-step-1", &mut cx).as_deref(),
        Some(steps[1].text.as_str())
    );
}

#[gpui_kit::test]
fn scenario_scan_progress_rescan_offered_once_the_decoder_is_available(cx: &mut TestAppContext) {
    let d = scanned_without_decoder();
    let (backend, shell, mut cx) = open_app(cx, &d.data, fake());
    show(&shell, ViewId::Sources, &mut cx);
    wait_long(&mut cx, "the rescan offer", |cx| {
        exists("summary-undecoded-rescan-heif", cx)
    });
    assert_eq!(
        label("summary-undecoded-heif", &mut cx).as_deref(),
        Some("HEIC decoder found. Rescan to fill in 3 previews")
    );
    let runs_before = backend.scan().unwrap().runs().unwrap().len();
    click("summary-undecoded-rescan-heif", &mut cx);
    let b = backend.clone();
    wait_long(&mut cx, "the rescan", move |_| {
        let s = b.scan().unwrap();
        let runs = s.runs().unwrap();
        runs.len() > runs_before && s.active().is_none()
    });
    let run = backend
        .scan()
        .unwrap()
        .runs()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(
        run.sources,
        vec![d.heic_source],
        "a run of that source only"
    );
    assert_ne!(d.heic_source, d.other_source);
    assert!(
        backend.scan().unwrap().undecoded(&[]).unwrap().is_empty(),
        "the previews are filled in"
    );
}

fn heic_item(backend: &CatalogBackend) -> ItemId {
    let r = backend.scan().unwrap().engine().catalog().read();
    let id = (1..10)
        .map(ps_model::FileId)
        .find(|f| {
            r.file_by_id(*f)
                .ok()
                .flatten()
                .is_some_and(|x| x.decoder_missing.is_some())
        })
        .expect("a waiting HEIC");
    ItemId::try_from(id).unwrap()
}

fn select(id: ItemId, cx: &mut VisualTestContext) {
    wait_long(cx, "the library", |cx| {
        cx.update(|_, cx| ui_workspace::views::library::LibraryHub::library(cx).is_some())
    });
    let lib = cx.update(|_, cx| ui_workspace::views::library::LibraryHub::library(cx).unwrap());
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    let m2 = model.clone();
    wait_long(cx, "the item in the list", move |cx| {
        cx.update(|_, cx| m2.read(cx).index.position_of(id).is_some())
    });
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(id))
        .unwrap();
    model.update(cx, |m, cx| {
        let ids = m.index.ids.clone();
        m.selection.click(&ids, index);
        m.selection_changed(cx);
    });
    wait_long(cx, "the inspector detail", move |cx| {
        cx.update(|_, cx| {
            ui_workspace::views::library::LibraryHub::inspector(cx)
                .is_some_and(|i| i.read(cx).detail().is_some_and(|d| d.id == id))
        })
    });
}

#[gpui_kit::test]
fn scenario_photo_inspector_inspector_explains_a_heic_without_a_decoder(cx: &mut TestAppContext) {
    let d = scanned_without_decoder();
    let (backend, shell, mut cx) = open_app(cx, &d.data, missing());
    show(&shell, ViewId::Library, &mut cx);
    select(heic_item(&backend), &mut cx);
    wait_long(&mut cx, "the explanation", |cx| {
        exists("inspector-preview-unavailable", cx)
    });
    assert_eq!(
        label("inspector-preview-unavailable", &mut cx).as_deref(),
        Some("Preview unavailable: no HEIC decoder on this system")
    );
    click("inspector-preview-how", &mut cx);
    wait_long(&mut cx, "the fix steps", |cx| {
        exists("inspector-fix-step-0", cx)
    });
    assert!(
        label("inspector-fix-step-0", &mut cx)
            .unwrap()
            .contains("sudo apt install libheif1 libheif-plugin-libde265")
    );
    assert!(!exists("inspector-preview-rescan", &mut cx));
}

#[gpui_kit::test]
fn scenario_photo_inspector_inspector_asks_for_a_rescan_once_the_decoder_is_available(
    cx: &mut TestAppContext,
) {
    let d = scanned_without_decoder();
    let (backend, shell, mut cx) = open_app(cx, &d.data, fake());
    show(&shell, ViewId::Library, &mut cx);
    select(heic_item(&backend), &mut cx);
    wait_long(&mut cx, "the rescan hint", |cx| {
        exists("inspector-preview-rescan", cx)
    });
    assert_eq!(
        label("inspector-preview-rescan", &mut cx).as_deref(),
        Some("A HEIC decoder is available now. Rescan this source to fill in the preview.")
    );
    assert!(!exists("inspector-preview-unavailable", &mut cx));
}

#[gpui_kit::test]
fn scenario_decoder_support_startup_detection_does_not_block_the_window(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let hub = ps_service::decoders::DecoderHub::lazy(|| {
        std::thread::sleep(Duration::from_secs(2));
        fake()
    });
    let svc = ScanService::open(&data, ScanOptions::default())
        .unwrap()
        .with_decoder_hub(hub.clone());
    let backend = Arc::new(CatalogBackend::with_service(&data, svc));
    let started = Instant::now();
    init_app(cx, backend, None);
    let (shell, mut cx) = open(cx);
    cx.run_until_parked();
    let first_frame = started.elapsed();
    let frames = cx.update(|_, cx| shell.read(cx).frames_drawn());
    assert!(frames > 0, "the first frame is drawn");
    // The fast-start budget of the window (the detector takes 2 s; the frame does not wait for it).
    assert!(
        first_frame < ps_fixtures::ci_slack(Duration::from_millis(1000)),
        "first frame after {first_frame:?}"
    );
    assert!(
        !exists("status-decoders", &mut cx),
        "no status before detection finishes"
    );
    wait_long(&mut cx, "the decoder status", |cx| {
        exists("status-decoders", cx)
    });
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(
        label("status-decoders", &mut cx).as_deref(),
        Some("HEIC: fake HEIC decoder")
    );
}
