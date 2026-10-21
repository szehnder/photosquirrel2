//! Settings that reach the scanner (change `settings-wiring`): the one options builder used by the app's
//! [`Library`] and the CLI, re-classification from stored probe facts on rescan (ADR-0051), changes applied
//! between runs and never to a running one, hidden folders (ADR-0052), the preview cache cap and extra cache
//! patterns.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ps_model::settings::Settings;
use ps_model::{PhotoClass, SourceId};
use ps_scan::{Instrumentation, SourceFs};
use ps_service::library::Library;
use ps_service::scan::{RunSummary, ScanOptions, ScanService};
use ps_service::settings::{SettingsStore, scan_options};

/// Plain JPEGs (no camera EXIF): the size rule decides. 150 x 1000 and 1200 x 180 are junk only because their
/// shorter side is under 200 px.
const FILES: &[(&str, u32, u32)] = &[
    ("icon.jpg", 64, 64),
    ("narrow.jpg", 150, 1000),
    ("wide.jpg", 1200, 180),
    ("photo.jpg", 1600, 1200),
];

fn write_source(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    for (i, (name, w, h)) in FILES.iter().enumerate() {
        std::fs::write(
            root.join(name),
            ps_fixtures::media::jpeg(*w, *h, 40 + i as u64, 90),
        )
        .unwrap();
    }
}

fn instrumented(opts: ScanOptions) -> (ScanOptions, Arc<Instrumentation>) {
    let (fs, inst) = SourceFs::instrumented();
    (ScanOptions { fs, ..opts }, inst)
}

/// Class by file name, as the catalog stores it.
fn classes(svc: &ScanService) -> BTreeMap<String, Option<PhotoClass>> {
    let mut out = BTreeMap::new();
    for s in svc.sources().unwrap() {
        for r in svc
            .engine()
            .catalog()
            .read()
            .source_files(s.id, None, 10_000)
            .unwrap()
        {
            out.insert(r.key.path.display().into_owned(), r.class);
        }
    }
    out
}

fn scan(svc: &ScanService, sources: &[SourceId]) -> RunSummary {
    svc.start(sources).unwrap();
    svc.wait().unwrap()
}

/// File names opened through the instrumented file layer.
fn opened(inst: &Instrumentation) -> BTreeSet<String> {
    inst.opens()
        .iter()
        .map(|o| o.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn expect(pairs: &[(&str, PhotoClass)]) -> BTreeMap<String, Option<PhotoClass>> {
    pairs
        .iter()
        .map(|(n, c)| (n.to_string(), Some(*c)))
        .collect()
}

fn defaults() -> BTreeMap<String, Option<PhotoClass>> {
    expect(&[
        ("icon.jpg", PhotoClass::Junk),
        ("narrow.jpg", PhotoClass::Junk),
        ("wide.jpg", PhotoClass::Junk),
        ("photo.jpg", PhotoClass::LikelyPhoto),
    ])
}

fn lowered() -> BTreeMap<String, Option<PhotoClass>> {
    expect(&[
        ("icon.jpg", PhotoClass::Junk),
        ("narrow.jpg", PhotoClass::LikelyPhoto),
        ("wide.jpg", PhotoClass::LikelyPhoto),
        ("photo.jpg", PhotoClass::LikelyPhoto),
    ])
}

/// Settings file -> service rescan -> catalog, as `photosquirrel-cli scan` runs it: the CLI opens the service
/// with `scan_options` from the settings file each time.
#[test]
fn scenario_settings_threshold_change_reclassifies() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    write_source(&root);
    let data = tmp.path().join("data");
    let store = SettingsStore::resolve(None, Some(&data)).unwrap();
    store.save(&Settings::default()).unwrap();

    let source = {
        let svc = ScanService::open(&data, scan_options(&store.load().settings)).unwrap();
        let id = svc.add_source(&root).unwrap().source.id;
        scan(&svc, &[id]);
        assert_eq!(classes(&svc), defaults());
        id
    };

    // The user lowers the minimum side to 100 px in the settings file; the next CLI scan reads it.
    store
        .update(|s| s.scanning.classifier.min_side_px = 100)
        .unwrap();
    let (opts, inst) = instrumented(scan_options(&store.load().settings));
    let svc = ScanService::open(&data, opts).unwrap();
    let summary = scan(&svc, &[source]);
    assert_eq!(classes(&svc), lowered());
    assert_eq!(summary.diff.reclassified, 2);
    assert_eq!(summary.diff.unchanged, FILES.len() as u64);
    // Re-classification read nothing: the only files opened are the two that became photos, read once each to
    // be analysed (hashes, metadata, thumbnail) like any newly found photo.
    assert_eq!(opened(&inst), names(&["narrow.jpg", "wide.jpg"]));
    assert_eq!(inst.open_count(), 2);
    drop(svc);

    // Raising the side above every file: photos become junk without a single read.
    store
        .update(|s| s.scanning.classifier.min_side_px = 2_000)
        .unwrap();
    let (opts, inst) = instrumented(scan_options(&store.load().settings));
    let svc = ScanService::open(&data, opts).unwrap();
    let summary = scan(&svc, &[source]);
    assert_eq!(summary.diff.reclassified, 3);
    assert!(classes(&svc).values().all(|c| *c == Some(PhotoClass::Junk)));
    assert_eq!(inst.open_count(), 0, "{:?}", inst.opens());

    // Unchanged settings: the next rescan has nothing to re-classify.
    let summary = scan(&svc, &[source]);
    assert_eq!(summary.diff.reclassified, 0);
    assert_eq!(inst.open_count(), 0);
}

/// The app's path: the [`Library`] gets the settings applied (as the settings window does) and rescans from
/// the engine it already has open; the outcome equals the CLI's.
#[test]
fn scenario_settings_app_rescan_uses_changed_thresholds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    write_source(&root);

    // The app.
    let data = tmp.path().join("app");
    let (opts, inst) = instrumented(ScanOptions::default());
    let lib = Library::with_service(&data, ScanService::open(&data, opts).unwrap());
    let mut settings = Settings::default();
    lib.apply_settings(&settings);
    let id = lib.scan().unwrap().add_source(&root).unwrap().source.id;
    scan(lib.scan().unwrap(), &[id]);
    assert_eq!(classes(lib.scan().unwrap()), defaults());

    settings.scanning.classifier.min_side_px = 100;
    lib.apply_settings(&settings);
    inst.reset();
    let summary = scan(lib.scan().unwrap(), &[id]);
    let app = classes(lib.scan().unwrap());
    assert_eq!(app, lowered());
    assert_eq!(summary.diff.reclassified, 2);
    assert_eq!(opened(&inst), names(&["narrow.jpg", "wide.jpg"]));

    // The CLI over the same source with the same settings file contents.
    let cli_data = tmp.path().join("cli");
    let store = SettingsStore::resolve(None, Some(&cli_data)).unwrap();
    store.save(&Settings::default()).unwrap();
    let svc = ScanService::open(&cli_data, scan_options(&store.load().settings)).unwrap();
    let cli_id = svc.add_source(&root).unwrap().source.id;
    scan(&svc, &[cli_id]);
    drop(svc);
    store
        .update(|s| s.scanning.classifier.min_side_px = 100)
        .unwrap();
    let svc = ScanService::open(&cli_data, scan_options(&store.load().settings)).unwrap();
    scan(&svc, &[cli_id]);
    assert_eq!(classes(&svc), app, "the app and the CLI agree");
}

/// Task 1.2: a change made while a scan runs applies to the next scan, never to the running one.
#[test]
fn settings_change_during_a_run_applies_to_the_next_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    write_source(&root);
    let data = tmp.path().join("data");
    let slow = ScanOptions {
        read_delay: Some(Duration::from_millis(150)),
        ..ScanOptions::default()
    };
    let lib = Library::with_service(&data, ScanService::open(&data, slow).unwrap());
    let mut settings = Settings::default();
    lib.apply_settings(&settings);
    let svc = lib.scan().unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    svc.start(&[id]).unwrap();
    // Mid-run: lower the threshold.
    settings.scanning.classifier.min_side_px = 100;
    lib.apply_settings(&settings);
    let first = svc.wait().unwrap();
    assert_eq!(first.diff.new, FILES.len() as u64);
    assert_eq!(
        classes(svc),
        defaults(),
        "the running scan kept its settings"
    );
    // The next run uses the new ones.
    let next = scan(svc, &[id]);
    assert_eq!(next.diff.reclassified, 2);
    assert_eq!(classes(svc), lowered());
}

/// Task 2.3: extra cache patterns from settings reach the classifier through `scan_options`, for new files and
/// on rescan alike.
#[test]
fn extra_cache_patterns_from_settings_classify_cache_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    let cache = root.join("PhotoAppCache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(
        cache.join("a.jpg"),
        ps_fixtures::media::jpeg(800, 600, 1, 90),
    )
    .unwrap();
    std::fs::write(
        root.join("b.jpg"),
        ps_fixtures::media::jpeg(800, 600, 2, 90),
    )
    .unwrap();
    let data = tmp.path().join("data");
    let mut settings = Settings::default();
    let svc = ScanService::open(&data, scan_options(&settings)).unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    scan(&svc, &[id]);
    let key = format!("PhotoAppCache{}a.jpg", std::path::MAIN_SEPARATOR);
    let class_of = |svc: &ScanService, name: &str| {
        classes(svc)
            .into_iter()
            .find(|(k, _)| k.replace('\\', "/") == name.replace('\\', "/"))
            .and_then(|(_, c)| c)
    };
    assert_eq!(class_of(&svc, &key), Some(PhotoClass::LikelyPhoto));
    drop(svc);

    settings.scanning.classifier.extra_cache_patterns = vec!["photoappcache".into()];
    let opts = scan_options(&settings);
    assert_eq!(
        opts.classifier.extra_cache_patterns,
        vec!["photoappcache".to_string()]
    );
    let svc = ScanService::open(&data, opts).unwrap();
    let summary = scan(&svc, &[id]);
    assert_eq!(summary.diff.reclassified, 1);
    assert_eq!(class_of(&svc, &key), Some(PhotoClass::ThumbnailCache));
    assert_eq!(class_of(&svc, "b.jpg"), Some(PhotoClass::LikelyPhoto));
}

#[test]
fn scenario_settings_hidden_files_setting_honoured() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    let hidden = root.join(".Family").join("2019");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(
        root.join("seen.jpg"),
        ps_fixtures::media::jpeg(800, 600, 1, 90),
    )
    .unwrap();
    std::fs::write(
        root.join(".hidden-file.jpg"),
        ps_fixtures::media::jpeg(800, 600, 2, 90),
    )
    .unwrap();
    for i in 0..3u64 {
        std::fs::write(
            hidden.join(format!("kid{i}.jpg")),
            ps_fixtures::media::jpeg(800, 600, 10 + i, 90),
        )
        .unwrap();
    }
    let data = tmp.path().join("data");
    let lib = Library::new(&data);
    let mut settings = Settings::default();
    settings.scanning.include_hidden = false;
    lib.apply_settings(&settings);
    let svc = lib.scan().unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    let first = scan(svc, &[id]);
    let listed: BTreeSet<String> = classes(svc).into_keys().collect();
    // Hidden folders are not listed; a dot-file directly in a folder is (the setting is about folders).
    assert_eq!(listed, names(&["seen.jpg", ".hidden-file.jpg"]));
    assert_eq!(first.sources[0].hidden_dirs_skipped, 1);

    // Turning it on and rescanning adds the folder's photos.
    settings.scanning.include_hidden = true;
    lib.apply_settings(&settings);
    let next = scan(svc, &[id]);
    assert_eq!(next.diff.new, 3);
    assert_eq!(next.sources[0].hidden_dirs_skipped, 0);
    let photos = classes(svc)
        .into_iter()
        .filter(|(k, c)| k.contains(".Family") && *c == Some(PhotoClass::LikelyPhoto))
        .count();
    assert_eq!(photos, 3);
}

/// Task 2.2: `scanning.preview_cache_gb` caps the 2048 px preview cache; lowering it evicts down to the new cap.
#[test]
fn preview_cache_cap_from_settings_evicts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("source");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..4u64 {
        std::fs::write(
            root.join(format!("p{i}.jpg")),
            ps_fixtures::media::jpeg(1200, 900, 70 + i, 90),
        )
        .unwrap();
    }
    let data = tmp.path().join("data");
    let lib = Library::new(&data);
    let mut settings = Settings::default();
    lib.apply_settings(&settings);
    let svc = lib.scan().unwrap();
    let id = svc.add_source(&root).unwrap().source.id;
    scan(svc, &[id]);
    let ids: Vec<ps_model::ItemId> = svc
        .engine()
        .catalog()
        .read()
        .source_files(id, None, 100)
        .unwrap()
        .into_iter()
        .map(|r| ps_model::ItemId::try_from(r.id).unwrap())
        .collect();
    assert_eq!(ids.len(), 4);
    for i in &ids {
        assert!(matches!(
            lib.preview(*i).unwrap(),
            ps_model::library::Preview::Jpeg(_)
        ));
    }
    let cache = data.join("previews");
    let cached = || -> Vec<u64> {
        std::fs::read_dir(&cache)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
            .map(|e| e.metadata().unwrap().len())
            .collect()
    };
    let sizes = cached();
    assert_eq!(sizes.len(), 4, "the default cap keeps every preview");
    assert_eq!(lib.preview_cache_cap(), 4 * 1024 * 1024 * 1024);

    // A cap that holds about two previews.
    let cap = sizes.iter().max().unwrap() * 2 + 1;
    settings.scanning.preview_cache_gb = cap as f64 / (1024.0 * 1024.0 * 1024.0);
    lib.apply_settings(&settings);
    assert!(lib.preview_cache_cap().abs_diff(cap) <= 1);
    // The next preview reopens the cache at the new cap.
    lib.preview(ids[0]).unwrap();
    let after = cached();
    assert!(after.len() < 4, "{after:?}");
    assert!(after.iter().sum::<u64>() <= cap, "{after:?} over {cap}");
}
