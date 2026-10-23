//! Change `settings-wiring`, headless against the real engine: a Scanning setting changed in the app reaches the
//! library engine for the next rescan (task 1.3), and a scan paused on battery says why on the dashboard (task
//! 2.4).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{TestAppContext, VisualTestContext};
use ps_model::{PhotoClass, RunStatus};
use ps_service::scan::{ScanOptions, ScanService};
use support::*;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::scan_backend::ScanBackend;
use ui_workspace::service::LibraryBackend;
use ui_workspace::settings::AppSettings;
use ui_workspace::views::ViewId;

fn class_of(backend: &CatalogBackend, name: &str) -> Option<PhotoClass> {
    let svc = backend.scan().unwrap();
    let source = svc.sources().unwrap()[0].id;
    svc.engine()
        .catalog()
        .read()
        .source_files(source, None, 100)
        .unwrap()
        .into_iter()
        .find(|r| r.key.path.display() == name)
        .and_then(|r| r.class)
}

/// Let real time pass for the engine while the fake GPUI clock drives the views' polling.
fn pump(cx: &mut VisualTestContext, ms: u64) {
    for _ in 0..ms / 10 {
        std::thread::sleep(Duration::from_millis(10));
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
    }
}

fn wait(
    cx: &mut VisualTestContext,
    what: &str,
    mut cond: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let limit = ps_fixtures::ci_slack(Duration::from_secs(10));
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if cond(cx) {
            return;
        }
        assert!(start.elapsed() < limit, "timed out waiting for {what}");
        pump(cx, 10);
    }
}

#[gpui_kit::test]
fn threshold_changed_in_the_app_reclassifies_on_the_next_rescan(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let root = dirs.root.path().join("source");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("narrow.jpg"),
        ps_fixtures::media::jpeg(150, 1000, 1, 90),
    )
    .unwrap();
    std::fs::write(
        root.join("photo.jpg"),
        ps_fixtures::media::jpeg(1600, 1200, 2, 90),
    )
    .unwrap();
    // The app's own backend: the engine opens lazily with the settings the shell applies.
    let backend = Arc::new(CatalogBackend::new(&dirs.data()));
    init_app(cx, backend.clone(), Some(&dirs));
    let (_shell, mut cx) = open(cx);
    cx.run_until_parked();

    LibraryBackend::add_source(&*backend, root.clone()).unwrap();
    backend.scan().unwrap().wait();
    assert_eq!(class_of(&backend, "narrow.jpg"), Some(PhotoClass::Junk));

    // The settings window's edit path.
    cx.update(|_, cx| {
        AppSettings::update(cx, |s| s.scanning.classifier.min_side_px = 100).unwrap()
    });
    cx.run_until_parked();
    let path = dirs.config().join("settings.toml");
    wait(&mut cx, "the settings write", |_| {
        std::fs::read_to_string(&path).is_ok_and(|t| t.contains("min_side_px = 100"))
    });
    LibraryBackend::rescan(&*backend).unwrap();
    let summary = backend.scan().unwrap().wait().unwrap();
    assert_eq!(summary.diff.reclassified, 1);
    assert_eq!(
        class_of(&backend, "narrow.jpg"),
        Some(PhotoClass::LikelyPhoto)
    );
    assert_eq!(
        class_of(&backend, "photo.jpg"),
        Some(PhotoClass::LikelyPhoto)
    );
}

struct Battery(AtomicBool);

impl ps_service::power::PowerSource for Battery {
    fn on_battery(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

fn slow_backend(data: &Path, root: &Path) -> Arc<CatalogBackend> {
    std::fs::create_dir_all(root).unwrap();
    for i in 0..40u64 {
        std::fs::write(
            root.join(format!("p{i:02}.jpg")),
            ps_fixtures::media::jpeg(320, 240, i, 80),
        )
        .unwrap();
    }
    let slow = ScanOptions {
        max_readers: 1,
        read_delay: Some(Duration::from_millis(200)),
        ..ScanOptions::default()
    };
    let backend = Arc::new(CatalogBackend::with_service(
        data,
        ScanService::open(data, slow).unwrap(),
    ));
    ScanBackend::add_source(&*backend, root).unwrap();
    backend
}

#[gpui_kit::test]
fn dashboard_says_a_scan_paused_on_battery(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let backend = slow_backend(&dirs.data(), &dirs.root.path().join("source"));
    let power = Arc::new(Battery(AtomicBool::new(false)));
    backend
        .library()
        .set_power_source(power.clone(), Duration::from_millis(20));
    init_app(cx, backend.clone(), Some(&dirs));
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    cx.update(|_, cx| AppSettings::update(cx, |s| s.general.pause_on_battery = true).unwrap());
    cx.run_until_parked();
    let s = shell.clone();
    cx.update(|w, cx| s.update(cx, |s, cx| s.show_view(ViewId::Sources, w, cx)));
    wait(&mut cx, "the sources", |cx| {
        label("scan-start", cx).is_some_and(|l| l.starts_with("Scan"))
    });
    click("scan-start", &mut cx);
    wait(&mut cx, "the scan to run", |_| {
        ScanBackend::snapshot(&*backend).is_some_and(|s| s.status == RunStatus::Running)
    });

    power.0.store(true, Ordering::SeqCst);
    wait(&mut cx, "the battery note", |cx| {
        label("scan-power-note", cx).is_some_and(|l| l.contains("battery power"))
    });
    assert_eq!(label("scan-status", &mut cx).as_deref(), Some("Paused"));
    assert!(exists("scan-resume", &mut cx));

    power.0.store(false, Ordering::SeqCst);
    wait(&mut cx, "the resume offer", |cx| {
        label("scan-power-note", cx).is_some_and(|l| l.contains("Resume"))
    });
    click("scan-resume", &mut cx);
    wait(&mut cx, "the note to clear", |cx| {
        !exists("scan-power-note", cx)
    });
    backend.scan().unwrap().cancel().unwrap();
    backend.scan().unwrap().wait();
}

/// capture-dates task 6.3 (ADR-0071): the last run's summary says how many dates the current rules changed and
/// how many files need an analysis refresh; "Refresh analysis" rescans those sources with the refresh on.
#[gpui_kit::test]
fn scan_summary_offers_the_analysis_refresh_and_reports_re_dated_files(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let root = dirs.root.path().join("source");
    let beach = root.join("2019-07-04 Beach");
    std::fs::create_dir_all(&beach).unwrap();
    for i in 0..3u64 {
        std::fs::write(
            beach.join(format!("p{i}.jpg")),
            ps_fixtures::media::jpeg(800, 600, 60 + i * 31, 88),
        )
        .unwrap();
    }
    let data = dirs.data();
    let utc = || ScanOptions {
        zone: Some(Arc::new(ps_media::FixedZone(0))),
        ..ScanOptions::default()
    };
    {
        let svc = ScanService::open(&data, utc()).unwrap();
        svc.add_source(&root).unwrap();
        svc.start(&[]).unwrap();
        svc.wait().unwrap();
    }
    // A catalog of an older version: no class facts, dates by the previous rules.
    {
        let c = rusqlite::Connection::open(data.join(ps_catalog::CATALOG_FILE)).unwrap();
        c.execute_batch(
            "UPDATE file SET class_facts = NULL, classifier_hash = NULL,
                 capture_provenance = 'file-mtime', capture_ts = (mtime_ns / 1000000000) * 1000,
                 capture_offset_min = 0, capture_undated = 1, capture_precision = 0, date_version = 1,
                 metadata = json_set(metadata, '$.capture_date', json_object(
                     'value', json_object('local_seconds', mtime_ns / 1000000000, 'millis', 0,
                                          'offset_minutes', 0),
                     'provenance', 'file_mtime', 'undated', json('true')));",
        )
        .unwrap();
    }
    let backend = Arc::new(CatalogBackend::with_service(
        &data,
        ScanService::open(&data, utc()).unwrap(),
    ));
    // The rescan that finds them: dates re-dated at once, the refresh only offered.
    ScanBackend::start(&*backend, &[]).unwrap();
    let s = backend.scan().unwrap().wait().unwrap();
    assert_eq!(
        (s.diff.redated, s.diff.needs_refresh),
        (3, 3),
        "{:?}",
        s.diff
    );

    init_app(cx, backend.clone(), Some(&dirs));
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    let sh = shell.clone();
    cx.update(|w, cx| sh.update(cx, |s, cx| s.show_view(ViewId::Sources, w, cx)));
    wait(&mut cx, "the refresh offer", |cx| {
        label("summary-needs-refresh", cx)
            .is_some_and(|l| l.starts_with("3 files need an analysis refresh"))
    });
    assert!(
        label("summary-redated", &mut cx).is_some_and(|l| l.starts_with("3 capture dates updated")),
        "{:?}",
        label("summary-redated", &mut cx)
    );
    let facts = || {
        let c = rusqlite::Connection::open(data.join(ps_catalog::CATALOG_FILE)).unwrap();
        c.query_row(
            "SELECT count(*) FROM file WHERE class_facts IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(facts(), 0);
    click("scan-refresh-analysis", &mut cx);
    wait(&mut cx, "the refreshing run to end", |_| {
        ScanBackend::snapshot(&*backend)
            .is_some_and(|s| s.status == RunStatus::Completed && s.run != ps_model::ScanRunId(2))
    });
    backend.scan().unwrap().wait();
    assert_eq!(facts(), 3, "the run refreshed the analysis");
}
