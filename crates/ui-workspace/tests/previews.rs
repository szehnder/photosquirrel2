//! Placeholders in the app (`preview-placeholders`, ADR-0109): the info button and the "Why no preview?" dialog
//! for each reason, Fill in previews from the dialog and the Library notice, Show them and the No preview filter.
//! Catalogs are built from fixture files in temp dirs by a real engine.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext, px, size};
use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::{
    DecoderEntry, DecoderFormat, DecoderReport, Fix, FixStep, ItemId, NoPreviewFilter,
    PreviewReason, RunStatus,
};
use ps_service::scan::{ScanOptions, ScanService};
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::library::{FilterBar, LibraryHub};
use ui_workspace::views::preview_help::{
    DAMAGED_TEXT, FILL_NOW_TEXT, HelpState, LATER_VERSION_TEXT, RESTART_TEXT,
};
use ui_workspace::views::{LibraryMode, ViewId};

const HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");
const STORE_URL: &str = "https://apps.microsoft.com/detail/9NMZLZ57R3T7";
const STORE_LINK: &str = "ms-windows-store://pdp/?ProductId=9NMZLZ57R3T7";

/// A Windows system missing the HEVC Video Extensions, as detection reports it.
fn windows_without_hevc() -> Decoders {
    let mut report = DecoderReport::default();
    report.set(DecoderEntry {
        format: DecoderFormat::Heic,
        available: false,
        backend: None,
        version: None,
        reason: Some(
            "no HEIC decoder on this system: the HEVC Video Extensions are not installed".into(),
        ),
        fix: Some(Fix {
            summary: "Install the HEVC Video Extensions".into(),
            steps: vec![
                FixStep {
                    text: "Install the HEVC Video Extensions from the Microsoft Store".into(),
                    url: Some(STORE_URL.into()),
                    open: Some(STORE_LINK.into()),
                    ..FixStep::default()
                },
                FixStep {
                    text: "Then restart PhotoSquirrel".into(),
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

/// `n` different HEIC files (the sample with different declared sizes).
fn heics(n: u32) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| ps_fixtures::heif::heif_with_size(HEIC, 4032 - i * 8, 3024 - i * 6).unwrap())
        .collect()
}

/// A JPEG whose data ends right after its scan header.
fn damaged_jpeg() -> Vec<u8> {
    let full = ps_fixtures::media::jpeg(1600, 1200, 77, 90);
    let sos = full.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let len = u16::from_be_bytes([full[sos + 2], full[sos + 3]]) as usize;
    full[..sos + 2 + len].to_vec()
}

struct Data {
    _tmp: tempfile::TempDir,
    data: PathBuf,
}

/// One source holding `files` (relative path, bytes), scanned without a HEIC decoder.
fn scanned(files: Vec<(String, Vec<u8>)>) -> Data {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("photos");
    for (rel, b) in &files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default())
        .unwrap()
        .with_decoders(windows_without_hevc());
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    assert_eq!(svc.wait().unwrap().status, RunStatus::Completed);
    Data { _tmp: tmp, data }
}

fn mixed() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = heics(4)
        .into_iter()
        .enumerate()
        .map(|(i, b)| (format!("DCIM/IMG_{i:04}.HEIC"), b))
        .collect();
    v.push(("DCIM/IMG_9000.JPG".into(), damaged_jpeg()));
    v.push((
        "scans/big.tif".into(),
        ps_fixtures::tiff_image::bigtiff_rgb(800, 600, 3),
    ));
    for i in 0..3u64 {
        v.push((
            format!("DCIM/IMG_{:04}.JPG", 100 + i),
            ps_fixtures::media::jpeg(800, 600, 40 + i, 85),
        ));
    }
    v
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
    vcx.simulate_resize(size(px(1440.0), px(1000.0)));
    let s = shell.clone();
    vcx.update(|w, cx| s.update(cx, |s, cx| s.show_view(ViewId::Library, w, cx)));
    vcx.run_until_parked();
    (backend, shell, vcx)
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

/// The item of the file whose name ends with `suffix`.
fn item(backend: &CatalogBackend, suffix: &str) -> ItemId {
    let r = backend.scan().unwrap().engine().catalog().read();
    let id = (1..200)
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

fn library_ready(cx: &mut VisualTestContext) {
    wait_long(cx, "the library list", |cx| {
        cx.update(|_, cx| {
            LibraryHub::library(cx).is_some_and(|l| {
                let m = l.read(cx).model().read(cx);
                m.state == ui_gallery::model::ListState::Ready && !m.is_empty()
            })
        })
    });
}

/// Open the dialog for `id` and wait for its content.
fn open_dialog(shell: &Entity<Shell>, id: ItemId, cx: &mut VisualTestContext) {
    let s = shell.clone();
    cx.update(|w, cx| {
        s.update(cx, |s, cx| {
            s.open_preview_help(id, w, cx);
        })
    });
    let s = shell.clone();
    wait_long(cx, "the dialog's content", move |cx| {
        cx.update(|_, cx| {
            s.read(cx)
                .preview_help()
                .is_some_and(|d| !matches!(d.read(cx).state(), HelpState::Loading))
        })
    });
    wait_until(cx, "the dialog drawn", |cx| {
        exists("preview-help-reason", cx)
    });
}

#[gpui_kit::test]
fn scenario_gallery_info_button_opens_the_dialog(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    library_ready(&mut cx);
    let heic = item(&backend, "IMG_0000.HEIC");
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let (grid, model) = cx.update(|_, cx| {
        let l = lib.read(cx);
        (l.grid().clone(), l.model().clone())
    });
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(heic))
        .expect("the HEIC is listed");
    let before = cx.update(|_, cx| model.read(cx).selection.focus);
    let centre = cx.update(|_, cx| grid.read(cx).cell_center(index, cx));
    cx.simulate_mouse_move(centre, None, Modifiers::none());
    let g = grid.clone();
    wait_until(&mut cx, "the info button", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            g.read(cx).info_button_center(index).is_some()
        })
    });
    let info = cx.update(|_, cx| grid.read(cx).info_button_center(index).unwrap());
    cx.simulate_click(info, Modifiers::none());
    let s = shell.clone();
    wait_until(&mut cx, "the dialog", move |cx| {
        cx.update(|_, cx| s.read(cx).preview_help().is_some())
    });
    let dialog_item = cx.update(|_, cx| shell.read(cx).preview_help().unwrap().read(cx).item());
    assert_eq!(dialog_item, heic, "the dialog is for that file");
    assert_eq!(
        cx.update(|_, cx| model.read(cx).selection.focus),
        before,
        "the selection is unchanged"
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_missing_decoder_shows_install_steps(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    open_dialog(&shell, item(&backend, "IMG_0001.HEIC"), &mut cx);
    let reason = label("preview-help-reason", &mut cx).unwrap();
    assert!(reason.contains("no HEIC decoder"), "{reason}");
    let step = label("preview-help-fix-step-0", &mut cx).unwrap();
    assert!(
        step.contains(STORE_URL),
        "the Store link shown as a URL: {step}"
    );
    assert!(
        exists("preview-help-fix-open-0", &mut cx),
        "opened only by its button"
    );
    assert_eq!(
        label("preview-help-restart", &mut cx).as_deref(),
        Some(RESTART_TEXT)
    );
    assert!(
        RESTART_TEXT
            .to_lowercase()
            .contains("restart photosquirrel and fill in previews"),
        "then restart PhotoSquirrel and fill in previews"
    );
    assert!(
        label("preview-help-still", &mut cx)
            .unwrap()
            .contains("catalogued")
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_decoder_available_offers_fill_in_previews(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, fake());
    let heic = item(&backend, "IMG_0002.HEIC");
    open_dialog(&shell, heic, &mut cx);
    assert_eq!(
        label("preview-help-fill-now", &mut cx).as_deref(),
        Some(FILL_NOW_TEXT)
    );
    let fill = label("preview-help-fill", &mut cx).unwrap();
    assert!(fill.contains("Fill in previews (4 HEIC files)"), "{fill}");
    click("preview-help-fill", &mut cx);
    // The job runs for HEIC files: they get previews and perceptual hashes.
    let b = backend.clone();
    wait_long(&mut cx, "the HEIC previews", move |_| {
        let r = b.scan().unwrap().engine().catalog().read();
        let f = r.file_by_id(heic.into()).unwrap().unwrap();
        b.scan().unwrap().active().is_none() && f.hashes.is_some() && f.preview.is_none()
    });
    let s = shell.clone();
    wait_until(&mut cx, "the dialog closed", move |cx| {
        cx.update(|_, cx| s.read(cx).preview_help().is_none())
    });
    // Only HEIC files were in the job: the damaged JPEG keeps its placeholder.
    let jpeg = item(&backend, "IMG_9000.JPG");
    let r = backend.scan().unwrap().engine().catalog().read();
    assert!(
        r.file_by_id(jpeg.into())
            .unwrap()
            .unwrap()
            .preview
            .is_some()
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_unsupported_variant_explains_the_limit(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    open_dialog(&shell, item(&backend, "big.tif"), &mut cx);
    assert_eq!(
        label("preview-help-unsupported", &mut cx).as_deref(),
        Some("This version can't read BigTIFF files.")
    );
    assert!(
        label("preview-help-still", &mut cx)
            .unwrap()
            .contains("catalogued, matched by exact content and exported")
    );
    assert_eq!(
        label("preview-help-update", &mut cx).as_deref(),
        Some(LATER_VERSION_TEXT)
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_damaged_file_shows_the_error(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    open_dialog(&shell, item(&backend, "IMG_9000.JPG"), &mut cx);
    let error = label("preview-help-error", &mut cx).unwrap();
    assert!(!error.is_empty(), "the decoder's error");
    assert_eq!(
        label("preview-help-damaged", &mut cx).as_deref(),
        Some(DAMAGED_TEXT)
    );
    assert!(DAMAGED_TEXT.contains("may be damaged") && DAMAGED_TEXT.contains("another app"));
    assert!(exists("preview-help-report", &mut cx));
}

#[gpui_kit::test]
fn scenario_photo_inspector_show_files_with_the_same_reason(cx: &mut TestAppContext) {
    let mut files: Vec<(String, Vec<u8>)> = heics(13)
        .into_iter()
        .enumerate()
        .map(|(i, b)| (format!("DCIM/IMG_{i:04}.HEIC"), b))
        .collect();
    files.push((
        "DCIM/IMG_9999.JPG".into(),
        ps_fixtures::media::jpeg(800, 600, 9, 85),
    ));
    let d = scanned(files);
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    library_ready(&mut cx);
    open_dialog(&shell, item(&backend, "IMG_0005.HEIC"), &mut cx);
    assert_eq!(
        label("preview-help-others", &mut cx).as_deref(),
        Some("12 other files have no preview for this reason")
    );
    click("preview-help-show", &mut cx);
    let s = shell.clone();
    wait_until(&mut cx, "the dialog closed", move |cx| {
        cx.update(|_, cx| s.read(cx).preview_help().is_none())
    });
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let l2 = lib.clone();
    wait_long(&mut cx, "the filtered list", move |cx| {
        cx.update(|_, cx| {
            let m = l2.read(cx).model().read(cx);
            m.state == ui_gallery::model::ListState::Ready && m.len() == 13
        })
    });
    let f = cx.update(|_, cx| lib.read(cx).filters.clone());
    assert_eq!(
        f.no_preview,
        Some(NoPreviewFilter {
            reason: Some(PreviewReason::DecoderMissing),
            format: Some(ps_model::Format::Heic),
        })
    );
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).active_view()),
        ViewId::Library
    );
}

#[gpui_kit::test]
fn scenario_gallery_filter_to_heic_files_waiting_for_a_decoder(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, _shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    library_ready(&mut cx);
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    lib.update(&mut cx, |l, cx| {
        l.set_filters(
            FilterBar {
                no_preview: Some(NoPreviewFilter {
                    reason: Some(PreviewReason::DecoderMissing),
                    format: Some(ps_model::Format::Heic),
                }),
                ..FilterBar::default()
            },
            cx,
        )
    });
    let l2 = lib.clone();
    wait_long(&mut cx, "the filtered list", move |cx| {
        cx.update(|_, cx| {
            let m = l2.read(cx).model().read(cx);
            m.state == ui_gallery::model::ListState::Ready && m.len() == 4
        })
    });
    let mut ids: Vec<ItemId> = cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
    ids.sort();
    let mut want: Vec<ItemId> = (0..4)
        .map(|i| item(&backend, &format!("IMG_{i:04}.HEIC")))
        .collect();
    want.sort();
    assert_eq!(ids, want, "exactly the HEIC items waiting for a decoder");
}

/// TIFFs a version with TIFF generation 0 catalogued with placeholders (0.2.0): their rows and thumbnails made
/// to look like migration 24 leaves them.
fn age_tiffs(data: &Path) {
    let svc = ScanService::open(data, ScanOptions::default()).unwrap();
    let thumbs = svc.engine().thumbs().clone();
    let r = svc.engine().catalog().read();
    let ids: Vec<ps_model::ContentId> = (1..100)
        .filter_map(|i| r.file_by_id(ps_model::FileId(i)).ok().flatten())
        .filter(|f| f.format == Some(ps_model::Format::Tiff))
        .filter_map(|f| f.identity)
        .collect();
    let probe = ps_media::probe_bytes(&[], 0, None);
    let mut ph = ps_media::thumb::placeholder(&probe, None, "preview unavailable");
    if let Some(p) = ph.placeholder.as_mut() {
        p.generation = 0;
        p.format = Some(ps_model::Format::Tiff);
    }
    for id in &ids {
        thumbs.put_many([(id, &ph)]).unwrap();
    }
    drop(svc);
    let c = rusqlite::Connection::open(data.join(ps_service::scan::CATALOG_FILE)).unwrap();
    c.execute(
        "UPDATE file SET phash = NULL, dhash = NULL, hash_version = NULL, preview_gen = 0,
                         preview_reason = 'unreadable'
         WHERE format = 'tiff'",
        [],
    )
    .unwrap();
}

#[gpui_kit::test]
fn scenario_gallery_notice_after_an_update_that_adds_tiff_variants(cx: &mut TestAppContext) {
    let files: Vec<(String, Vec<u8>)> = (0..37u64)
        .map(|i| {
            (
                format!("scans/scan_{i:02}.tif"),
                ps_fixtures::tiff_image::tiff_image(&ps_fixtures::tiff_image::TiffSpec::new(
                    480,
                    320,
                    600 + i,
                )),
            )
        })
        .collect();
    let d = scanned(files);
    age_tiffs(&d.data);
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    library_ready(&mut cx);
    wait_long(&mut cx, "the notice", |cx| {
        exists("library-preview-notice", cx)
    });
    assert_eq!(
        label("library-preview-notice", &mut cx).as_deref(),
        Some("37 TIFF files can get previews now")
    );
    assert!(
        exists("library-preview-fill", &mut cx),
        "offers Fill in previews"
    );
    click("library-preview-fill", &mut cx);
    let b = backend.clone();
    wait_long(&mut cx, "the TIFF previews", move |_| {
        let s = b.scan().unwrap();
        s.active().is_none()
            && s.preview_status(&[])
                .unwrap()
                .iter()
                .all(|r| r.format != "tiff")
    });
    // The notice goes once the counts are read again after the job.
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    lib.update(&mut cx, |l, cx| l.load_previews(cx));
    wait_long(&mut cx, "the notice gone", |cx| {
        !exists("library-preview-notice", cx)
    });
    let _ = (shell, LibraryMode::Grid);
}
