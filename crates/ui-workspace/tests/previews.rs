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
                // The closing step of every Linux and Windows fix (ps-sysdecode's guidance).
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
    open_app_with(cx, data, decoders, ScanOptions::default(), None)
}

fn open_app_with(
    cx: &mut TestAppContext,
    data: &Path,
    decoders: Decoders,
    options: ScanOptions,
    dirs: Option<&Dirs>,
) -> (Arc<CatalogBackend>, Entity<Shell>, VisualTestContext) {
    let svc = ScanService::open(data, options)
        .unwrap()
        .with_decoders(decoders);
    let backend = Arc::new(CatalogBackend::with_service(data, svc));
    init_app(cx, backend.clone(), dirs);
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
    // One closing line: the report's "restart and rescan" step is not shown beside it.
    assert!(
        !exists("preview-help-fix-step-1", &mut cx),
        "only the install step: {:?}",
        label("preview-help-fix-step-1", &mut cx)
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

/// Shift+I on a selected placeholder opens "Why no preview?" for it, in the grid and in the loupe (I alone toggles
/// the inspector).
#[gpui_kit::test]
fn shift_i_opens_the_dialog_in_the_grid_and_the_loupe(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    library_ready(&mut cx);
    let heic = item(&backend, "IMG_0001.HEIC");
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let (grid, model) = cx.update(|_, cx| {
        let l = lib.read(cx);
        (l.grid().clone(), l.model().clone())
    });
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(heic))
        .expect("the HEIC is listed");
    // Wait for its summary (the placeholder badge), then select it and focus the grid.
    let m = model.clone();
    wait_long(&mut cx, "the HEIC's summary", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            m.read(cx)
                .summary(index)
                .is_some_and(|s| s.placeholder.is_some())
        })
    });
    grid.update(&mut cx, |g, cx| g.click(index, Modifiers::none(), cx));
    cx.update(|w, cx| {
        let f = grid.read(cx).focus_handle().clone();
        w.focus(&f, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-i");
    let s = shell.clone();
    wait_long(&mut cx, "the dialog from the grid", move |cx| {
        cx.update(|_, cx| s.read(cx).preview_help().is_some())
    });
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).preview_help().unwrap().read(cx).item()),
        heic
    );
    let s = shell.clone();
    cx.update(|w, cx| s.update(cx, |s, cx| s.close_overlay(w, cx)));
    cx.run_until_parked();
    assert!(cx.update(|_, cx| shell.read(cx).preview_help().is_none()));
    // The loupe on the same item.
    let l = lib.clone();
    cx.update(|w, cx| l.update(cx, |l, cx| l.set_mode(LibraryMode::Loupe, w, cx)));
    cx.run_until_parked();
    let loupe = cx.update(|_, cx| lib.read(cx).loupe().clone());
    cx.update(|w, cx| {
        let f = loupe.read(cx).focus_handle().clone();
        w.focus(&f, cx);
        w.render_frame(cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-i");
    let s = shell.clone();
    wait_long(&mut cx, "the dialog from the loupe", move |cx| {
        cx.update(|_, cx| s.read(cx).preview_help().is_some())
    });
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).preview_help().unwrap().read(cx).item()),
        heic
    );
}

/// Fill in previews from the Library notice, a job shorter than a live-update poll: the tiles it filled in show
/// their thumbnails without a restart, and the notice's counts are read again.
#[gpui_kit::test]
fn filled_in_tiles_show_their_thumbnails_after_a_short_job(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, _shell, mut cx) = open_app(cx, &d.data, fake());
    library_ready(&mut cx);
    let heic = item(&backend, "IMG_0002.HEIC");
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let (grid, model) = cx.update(|_, cx| {
        let l = lib.read(cx);
        (l.grid().clone(), l.model().clone())
    });
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(heic))
        .expect("the HEIC is listed");
    let g = grid.clone();
    wait_long(&mut cx, "the HEIC's placeholder tile", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            matches!(
                g.read(cx).painted(index),
                Some(ui_gallery::grid::CellPaint::PlaceholderTile { .. })
            )
        })
    });
    wait_long(&mut cx, "the notice", |cx| {
        exists("library-preview-notice", cx)
    });
    click("library-preview-fill", &mut cx);
    let g = grid.clone();
    wait_long(&mut cx, "the filled-in thumbnail", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            matches!(
                g.read(cx).painted(index),
                Some(ui_gallery::grid::CellPaint::Texture { .. })
            )
        })
    });
    let r = backend.scan().unwrap().engine().catalog().read();
    assert!(
        r.file_by_id(heic.into())
            .unwrap()
            .unwrap()
            .preview
            .is_none(),
        "filled in"
    );
    // The counts are read again: nothing is offered any more, and the notice stays away.
    wait_long(&mut cx, "the counts read again", |cx| {
        cx.update(|_, cx| {
            LibraryHub::library(cx).is_some_and(|l| {
                let l = l.read(cx);
                !l.previews().is_empty() && l.previews().iter().all(|r| r.fillable == 0)
            })
        })
    });
    assert!(!exists("library-preview-notice", &mut cx));
}

/// A refused Fill in previews (another run is going) is said in a toast, and the notice comes back.
#[gpui_kit::test]
fn a_refused_fill_in_shows_a_toast_and_keeps_the_notice(cx: &mut TestAppContext) {
    let d = scanned(mixed());
    let (backend, shell, mut cx) = open_app(cx, &d.data, fake());
    library_ready(&mut cx);
    wait_long(&mut cx, "the notice", |cx| {
        exists("library-preview-notice", cx)
    });
    // Another process is scanning this catalog (its run is recorded as running).
    let c = rusqlite::Connection::open(d.data.join(ps_service::scan::CATALOG_FILE)).unwrap();
    c.execute(
        "INSERT INTO scan_run (status, started_at) VALUES ('running', 1)",
        [],
    )
    .unwrap();
    click("library-preview-fill", &mut cx);
    let s = shell.clone();
    wait_long(&mut cx, "the toast", move |cx| {
        cx.update(|_, cx| s.read(cx).toast().is_some())
    });
    let title = cx.update(|_, cx| shell.read(cx).toast().unwrap().title.to_string());
    assert_eq!(title, "Could not fill in previews");
    wait_long(&mut cx, "the notice back", |cx| {
        exists("library-preview-notice", cx)
    });
    let _ = backend;
}

/// A catalog made by 0.2.0 without a poster-frame tool: the video's stored placeholder records no reason and its
/// row nothing (migration 24 cannot tell it from a poster frame). Opened by this build, migration 25's task marks it
/// from the thumbnail store (ADR-0111): its tile is a placeholder that says why, with the info button, and the
/// dialog explains the poster-frame tool.
#[gpui_kit::test]
fn videos_from_older_catalogs_show_why_they_have_no_preview(cx: &mut TestAppContext) {
    let mut files = mixed();
    files.push((
        "DCIM/IMG_5000.MOV".into(),
        ps_fixtures::media::movie(&ps_fixtures::media::MovieSpec::iphone()),
    ));
    let d = scanned(files);
    {
        let svc = ScanService::open(&d.data, ScanOptions::default()).unwrap();
        let r = svc.engine().catalog().read();
        let video = (1..100)
            .filter_map(|i| r.file_by_id(ps_model::FileId(i)).ok().flatten())
            .find(|f| f.format == Some(ps_model::Format::Mov))
            .expect("the video is catalogued");
        assert!(video.preview.is_some(), "this build marks it as it scans");
        let probe = ps_media::probe_bytes(&[], 0, None);
        let mut ph = ps_media::thumb::placeholder(&probe, None, "video (no poster-frame tool)");
        if let Some(p) = ph.placeholder.as_mut() {
            p.media_kind = ps_model::MediaKind::Video;
            p.format = Some(ps_model::Format::Mov);
        }
        assert!(
            ph.placeholder.as_ref().unwrap().reason.is_none(),
            "as 0.2.0 stored it"
        );
        svc.engine()
            .thumbs()
            .put_many([(&video.identity.unwrap(), &ph)])
            .unwrap();
    }
    // The catalog as migration 24 left it (0.2.0's rows, the video unmarked).
    let c = rusqlite::Connection::open(d.data.join(ps_service::scan::CATALOG_FILE)).unwrap();
    c.execute_batch(
        "UPDATE file SET preview_gen = NULL, preview_reason = NULL WHERE format = 'mov';
         DROP TABLE catalog_task;
         ALTER TABLE file DROP COLUMN colour;
         PRAGMA user_version = 24;",
    )
    .unwrap();
    drop(c);
    let (backend, shell, mut cx) = open_app(cx, &d.data, windows_without_hevc());
    library_ready(&mut cx);
    let video = item(&backend, "IMG_5000.MOV");
    let r = backend.scan().unwrap().engine().catalog().read();
    assert_eq!(
        r.file_by_id(video.into()).unwrap().unwrap().preview,
        Some(ps_model::PreviewStamp {
            generation: 0,
            reason: PreviewReason::NoPosterTool
        }),
        "marked at open"
    );
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let (grid, model) = cx.update(|_, cx| {
        let l = lib.read(cx);
        (l.grid().clone(), l.model().clone())
    });
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(video))
        .expect("the video is listed");
    // The app resolves the poster-frame tool from its settings: none is configured here, so `ffmpeg` on this
    // machine's `PATH` counts when there is one, and the video can get its frame now (ADR-0114).
    let tool_on_path = ps_media::poster::ffmpeg_on_path().is_some();
    let want = if tool_on_path {
        ps_model::PlaceholderStatus::ReadyToFill
    } else {
        ps_model::PlaceholderStatus::NeedsLibrary
    };
    let g = grid.clone();
    wait_long(&mut cx, "the video's placeholder tile", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            matches!(
                g.read(cx).painted(index),
                Some(ui_gallery::grid::CellPaint::PlaceholderTile { status, .. }) if status == want
            )
        })
    });
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
    wait_long(&mut cx, "the dialog's content", move |cx| {
        cx.update(|_, cx| {
            s.read(cx)
                .preview_help()
                .is_some_and(|d| !matches!(d.read(cx).state(), HelpState::Loading))
        })
    });
    wait_until(&mut cx, "the dialog drawn", |cx| {
        exists("preview-help-reason", cx)
    });
    let reason = label("preview-help-reason", &mut cx).unwrap();
    assert!(reason.contains("poster-frame tool (ffmpeg)"), "{reason}");
    if tool_on_path {
        // A tool is available now: Fill in previews gives the video its frame.
        assert!(reason.contains("available now"), "{reason}");
        let fill = label("preview-help-fill", &mut cx).unwrap();
        assert!(fill.contains("Fill in previews (MOV file)"), "{fill}");
        return;
    }
    // What to do: install ffmpeg (or, where one is set up, when the frame comes).
    let fix = label("preview-help-fix", &mut cx).or_else(|| label("preview-help-poster", &mut cx));
    assert!(
        fix.as_deref()
            .is_some_and(|f| f.contains("ffmpeg") || f.contains("poster-frame tool")),
        "{fix:?}"
    );
}

/// A fake poster-frame tool in `dir` (ADR-0114): fails on every video whose name contains `fail` with ffmpeg's
/// "moov atom not found", else prints a JPEG frame.
#[cfg(unix)]
fn fake_ffmpeg(dir: &Path, name: &str, fail: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let frame = dir.join("frame.jpg");
    std::fs::write(&frame, ps_fixtures::media::jpeg(640, 360, 9, 85)).unwrap();
    let tool = dir.join(name);
    let fail = if fail.is_empty() {
        String::new()
    } else {
        format!(
            "case \"$src\" in *{fail}*) echo \"$src: moov atom not found\" >&2; exit 1;; esac\n"
        )
    };
    std::fs::write(
        &tool,
        format!(
            "#!/bin/sh\nsrc=\nwhile [ $# -gt 0 ]; do [ \"$1\" = -i ] && src=$2; shift; done\n{fail}cat '{}'\n",
            frame.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    tool
}

/// "Why no preview?" for a video whose poster frame failed (ADR-0114): ffmpeg's error, what still works, Try
/// again and the other videos; Try again runs the tool once more and the video gets its frame.
#[cfg(unix)]
#[gpui_kit::test]
fn poster_failed_dialog_shows_the_error_and_tries_again(cx: &mut TestAppContext) {
    let tools = tempfile::tempdir().unwrap();
    let failing = fake_ffmpeg(tools.path(), "ffmpeg-old", "IMG_50");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("photos");
    std::fs::create_dir_all(root.join("DCIM")).unwrap();
    for i in 0..2i64 {
        let m = ps_fixtures::media::movie(&ps_fixtures::media::MovieSpec {
            creation_unix: Some(1_562_265_000 + i * 60),
            ..ps_fixtures::media::MovieSpec::iphone()
        });
        std::fs::write(root.join(format!("DCIM/IMG_500{i}.MOV")), m).unwrap();
    }
    std::fs::write(
        root.join("DCIM/IMG_0100.JPG"),
        ps_fixtures::media::jpeg(800, 600, 40, 85),
    )
    .unwrap();
    let data = tmp.path().join("data");
    {
        let svc = ScanService::open(
            &data,
            ScanOptions {
                poster_tool: Some(failing.clone()),
                ..ScanOptions::default()
            },
        )
        .unwrap()
        .with_decoders(windows_without_hevc());
        svc.add_source(&root).unwrap();
        svc.start(&[]).unwrap();
        assert_eq!(svc.wait().unwrap().status, RunStatus::Completed);
    }
    // ffmpeg is updated: the new one reads both videos. It is the configured tool, so the app's settings resolve to
    // it whatever this machine's `PATH` has.
    let fixed = fake_ffmpeg(tools.path(), "ffmpeg-new", "");
    let dirs = Dirs::new();
    let mut settings = ps_model::settings::Settings::default();
    settings.scanning.poster_frame_tool = Some(fixed.clone());
    dirs.store().save(&settings).unwrap();
    let (backend, shell, mut cx) = open_app_with(
        cx,
        &data,
        windows_without_hevc(),
        ScanOptions {
            poster_tool: Some(fixed),
            ..ScanOptions::default()
        },
        Some(&dirs),
    );
    library_ready(&mut cx);
    let video = item(&backend, "IMG_5000.MOV");
    open_dialog(&shell, video, &mut cx);
    let help = cx
        .update(|_, cx| {
            shell
                .read(cx)
                .preview_help()
                .and_then(|d| d.read(cx).help().cloned())
        })
        .expect("the dialog's content");
    assert_eq!(help.reason, PreviewReason::PosterFailed);
    assert_eq!(help.status, ps_model::PlaceholderStatus::Damaged);
    let reason = label("preview-help-reason", &mut cx).unwrap();
    assert!(
        reason.starts_with("ffmpeg could not get a frame from this video: "),
        "{reason}"
    );
    assert!(reason.contains("moov atom not found"), "{reason}");
    assert!(
        reason.ends_with("The video is catalogued and exported as usual."),
        "{reason}"
    );
    assert!(
        !reason.contains("none is set up"),
        "not the missing-tool text: {reason}"
    );
    let error = label("preview-help-error", &mut cx).unwrap();
    assert!(error.contains("exit status"), "{error}");
    assert_eq!(
        label("preview-help-retry-note", &mut cx).as_deref(),
        Some(ui_workspace::views::preview_help::POSTER_RETRY_TEXT)
    );
    assert_eq!(
        label("preview-help-others", &mut cx).as_deref(),
        Some("1 other video has no preview for this reason")
    );
    assert_eq!(
        label("preview-help-retry", &mut cx).as_deref(),
        Some("Try again")
    );
    click("preview-help-retry", &mut cx);
    let b = backend.clone();
    wait_long(&mut cx, "the poster frames", move |_| {
        let scan = b.scan().unwrap();
        let r = scan.engine().catalog().read();
        let f = r.file_by_id(video.into()).unwrap().unwrap();
        scan.active().is_none()
            && f.preview.is_none()
            && scan
                .engine()
                .thumbs()
                .get(&f.identity.unwrap())
                .unwrap()
                .is_some_and(|t| t.kind == ps_model::ThumbKind::PosterFrame)
    });
    let other = item(&backend, "IMG_5001.MOV");
    let r = backend.scan().unwrap().engine().catalog().read();
    assert_eq!(
        r.file_by_id(other.into()).unwrap().unwrap().preview,
        None,
        "Try again retried every failed poster"
    );
}
