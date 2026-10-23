//! capture-dates through the service (tasks 5.1-5.4; ADR-0069, ADR-0070, ADR-0071): export filing reads the
//! catalog's date columns, folder-name dates in the group filing order, month folders, local `Undated/` days,
//! and existing libraries that keep their layout after the date rules change.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use ps_export::{DateSource, ExecConfig};
use ps_fixtures::media::{ExifSpec, encode_jpeg, exif_tiff, insert_exif, photo_rgb};
use ps_media::{FixedZone, LocalZone, RuleZone};
use ps_model::{CaptureTime, DatePrecision, FileId};
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn opts(zone: impl LocalZone + 'static) -> ScanOptions {
    ScanOptions {
        zone: Some(Arc::new(zone)),
        ..ScanOptions::default()
    }
}

fn at(y: i32, m: u8, d: u8, h: u8, mi: u8) -> SystemTime {
    let s = CaptureTime::from_parts(y, m, d, h, mi, 0).local_seconds;
    SystemTime::UNIX_EPOCH + Duration::from_secs(s as u64)
}

fn write(path: &Path, bytes: &[u8], mtime: SystemTime) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(mtime))
        .unwrap();
}

/// An EXIF-less photo (a distinct picture per seed).
fn stripped(seed: u64, quality: u8) -> Vec<u8> {
    encode_jpeg(&photo_rgb(800, 600, seed), 800, 600, quality)
}

fn scan(svc: &ScanService) -> ps_service::scan::RunSummary {
    svc.start(&[]).unwrap();
    let s = svc.wait().unwrap();
    assert_eq!(s.status, ps_model::RunStatus::Completed, "{s:?}");
    s
}

fn open(data: &Path, o: ScanOptions, src: &Path) -> ScanService {
    let svc = ScanService::open(data, o).unwrap();
    if svc.sources().unwrap().is_empty() {
        svc.add_source(src).unwrap();
    }
    svc
}

/// The one group's member file ids by path suffix; makes `keeper` the keeper.
fn keep(svc: &ScanService, keeper: &str) -> BTreeMap<String, FileId> {
    let groups = svc
        .groups()
        .list(ps_service::groups::GroupOrder::Id, 0, 10)
        .unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    let d = svc.groups().show(groups[0].id).unwrap();
    let ids: BTreeMap<String, FileId> = d
        .members
        .iter()
        .map(|m| (m.path.to_string_lossy().replace('\\', "/"), m.file))
        .collect();
    let (_, k) = ids
        .iter()
        .find(|(p, _)| p.ends_with(keeper))
        .unwrap_or_else(|| panic!("{keeper} in {ids:?}"));
    svc.groups().set_keeper(d.id, *k).unwrap();
    ids
}

fn id_of(ids: &BTreeMap<String, FileId>, suffix: &str) -> FileId {
    *ids.iter().find(|(p, _)| p.ends_with(suffix)).unwrap().1
}

fn plan(svc: &ScanService, library: &Path) -> ps_export::Plan {
    svc.exports()
        .plan(library, profile_settings("apple").unwrap())
        .unwrap()
        .0
}

fn run(svc: &ScanService, library: &Path) -> ps_export::ExportReport {
    let (_, r) = svc
        .exports()
        .run(
            library,
            profile_settings("apple").unwrap(),
            &ExecConfig::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    r
}

#[test]
fn scenario_export_embedded_date_of_a_copy_beats_the_keeper_s_folder_date() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let rgb = photo_rgb(1200, 900, 91);
    let spec = ExifSpec {
        date_time_original: Some("2019:07:03 21:00:00".into()),
        ..ExifSpec::canon_5d()
    };
    let original = insert_exif(&encode_jpeg(&rgb, 1200, 900, 90), &exif_tiff(&spec));
    write(
        &src.join("camera/IMG_0042.JPG"),
        &original,
        at(2019, 7, 3, 21, 0),
    );
    write(
        &src.join("2019-07-04/photo.jpg"),
        &encode_jpeg(&rgb, 1200, 900, 89),
        at(2024, 3, 1, 12, 0),
    );
    let svc = open(&tmp.path().join("data"), opts(FixedZone(0)), &src);
    scan(&svc);
    let ids = keep(&svc, "2019-07-04/photo.jpg");
    let p = plan(&svc, &tmp.path().join("Library"));
    let f = p.files().next().unwrap();
    assert_eq!(f.target.display(), "2019/2019-07-03/photo.jpg");
    assert_eq!(
        f.date_origin.source,
        DateSource::Embedded {
            provenance: ps_model::DateProvenance::DateTimeOriginal
        }
    );
    assert_eq!(f.date_origin.copy, Some(id_of(&ids, "IMG_0042.JPG")));
}

#[test]
fn scenario_export_folder_date_of_a_copy_files_a_stripped_keeper() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let rgb = photo_rgb(1200, 900, 92);
    write(
        &src.join("Downloads/IMG_0001.JPG"),
        &encode_jpeg(&rgb, 1200, 900, 92),
        at(2024, 3, 1, 12, 0),
    );
    write(
        &src.join("2019_07_04 Beach/beach.jpg"),
        &encode_jpeg(&rgb, 1200, 900, 80),
        at(2024, 3, 1, 12, 0),
    );
    let svc = open(&tmp.path().join("data"), opts(FixedZone(0)), &src);
    scan(&svc);
    let ids = keep(&svc, "Downloads/IMG_0001.JPG");
    let p = plan(&svc, &tmp.path().join("Library"));
    let files: Vec<_> = p.files().collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let f = files[0];
    assert_eq!(f.file, id_of(&ids, "IMG_0001.JPG"));
    assert_eq!(f.target.display(), "2019/2019-07-04/IMG_0001.JPG");
    assert_eq!(f.date_origin.source, DateSource::FolderName);
    assert_eq!(f.date_origin.copy, Some(id_of(&ids, "beach.jpg")));
    assert_eq!(f.date_origin.describe(), "folder name of another copy");
    assert_eq!(f.date_precision, DatePrecision::Day);
    assert_eq!(p.totals.dates.folder_name, 1);
}

#[test]
fn scenario_export_month_precision_file_placed_in_a_month_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src.join("2019-07 Italy/IMG_0001.JPG"),
        &stripped(93, 90),
        at(2024, 3, 1, 12, 0),
    );
    let svc = open(&tmp.path().join("data"), opts(FixedZone(0)), &src);
    scan(&svc);
    let library = tmp.path().join("Library");
    let p = plan(&svc, &library);
    let f = p.files().next().unwrap();
    assert_eq!(f.target.display(), "2019/2019-07/IMG_0001.JPG");
    assert_eq!(f.date_origin.source, DateSource::FolderName);
    assert_eq!(f.date_origin.describe(), "folder name");
    assert_eq!(f.date_precision, DatePrecision::Month);
    assert_eq!(run(&svc, &library).copied, 1);
    assert!(library.join("2019/2019-07/IMG_0001.JPG").is_file());
    let m = ps_export::manifest::read_manifest(&library).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].library_path, "2019/2019-07/IMG_0001.JPG");
    assert_eq!(m[0].date_source.as_deref(), Some("folder name"));
    assert_eq!(m[0].date_precision.as_deref(), Some("month"));
}

#[test]
fn scenario_export_undated_day_is_the_local_day() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src.join("Misc/IMG_0001.JPG"),
        &stripped(94, 90),
        at(2019, 7, 5, 3, 30),
    );
    let svc = open(&tmp.path().join("data"), opts(RuleZone::LOS_ANGELES), &src);
    scan(&svc);
    let p = plan(&svc, &tmp.path().join("Library"));
    let f = p.files().next().unwrap();
    assert_eq!(f.target.display(), "Undated/2019/2019-07-04/IMG_0001.JPG");
    assert!(f.date_origin.is_undated());
}

/// Task 5.1: a re-dated row (its date columns changed, its metadata JSON not) is filed by the columns.
#[test]
fn re_dated_row_is_filed_by_the_catalog_columns() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src.join("Misc/IMG_0001.JPG"),
        &stripped(95, 90),
        at(2024, 3, 1, 12, 0),
    );
    let data = tmp.path().join("data");
    let svc = open(&data, opts(FixedZone(0)), &src);
    scan(&svc);
    let library = tmp.path().join("Library");
    assert_eq!(
        plan(&svc, &library)
            .files()
            .next()
            .unwrap()
            .target
            .display(),
        "Undated/2024/2024-03-01/IMG_0001.JPG"
    );
    let ts = CaptureTime::from_parts(2019, 7, 4, 0, 0, 0).local_seconds * 1000;
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&data)).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c.execute(
        "UPDATE file SET capture_ts = ?1, capture_offset_min = NULL, capture_provenance = 'folder-name',
                         capture_undated = 0, capture_precision = 1",
        [ts],
    )
    .unwrap();
    let f = plan(&svc, &library);
    let f = f.files().next().unwrap();
    assert_eq!(f.target.display(), "2019/2019-07-04/IMG_0001.JPG");
    assert_eq!(f.date_origin.source, DateSource::FolderName);
}

// ---- existing libraries keep their layout (task 5.4) ----------------------------------------------------------

/// Every file under `root` with its size, modification time and content hash.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, String)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p);
            } else {
                let hash = blake3::hash(&std::fs::read(&p).unwrap())
                    .to_hex()
                    .to_string();
                out.insert(
                    p.strip_prefix(root).unwrap().to_path_buf(),
                    (m.len(), m.modified().unwrap(), hash),
                );
            }
        }
    }
    out
}

/// Rewrite the rows without an embedded date as the previous date rules left them (version 1).
fn make_old_catalog(data: &Path) {
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(data)).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c.execute_batch(
        "UPDATE file SET
            capture_provenance = 'file-mtime',
            capture_ts = (mtime_ns / 1000000000) * 1000,
            capture_offset_min = 0,
            capture_undated = 1,
            capture_precision = 0,
            date_version = 1,
            metadata = json_set(metadata, '$.capture_date', json_object(
                'value', json_object('local_seconds', mtime_ns / 1000000000, 'millis', 0, 'offset_minutes', 0),
                'provenance', 'file_mtime', 'undated', json('true')))
         WHERE metadata IS NOT NULL AND capture_provenance IN ('folder-name', 'file-mtime');",
    )
    .unwrap();
}

struct OldLibrary {
    tmp: tempfile::TempDir,
    src: PathBuf,
    data: PathBuf,
    library: PathBuf,
}

/// 20 EXIF-less photos in `2019-07-04 Beach/`, exported under the previous date rules (to `Undated/`), then the
/// catalog re-dated by a rescan with the current rules.
fn exported_under_earlier_rules() -> (
    OldLibrary,
    ScanService,
    BTreeMap<PathBuf, (u64, SystemTime, String)>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    for i in 0..20u64 {
        write(
            &src.join(format!("2019-07-04 Beach/IMG_{i:04}.JPG")),
            &stripped(1_000 + i * 7_919, 88),
            at(2024, 3, 1, 12, 0),
        );
    }
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");
    {
        let svc = open(&data, opts(FixedZone(0)), &src);
        scan(&svc);
    }
    make_old_catalog(&data);
    let svc = open(&data, opts(FixedZone(0)), &src);
    let first = plan(&svc, &library);
    assert_eq!(first.totals.files, 20);
    assert!(
        first
            .files()
            .all(|f| f.target.display().starts_with("Undated/2024/2024-03-01/")),
        "the earlier rules file them by their file time"
    );
    assert_eq!(run(&svc, &library).copied, 20);
    let before = snapshot(&library);
    // The next rescan re-dates the catalog: every photo now has its folder date.
    let summary = scan(&svc);
    assert_eq!(summary.diff.redated, 20);
    (
        OldLibrary {
            tmp,
            src,
            data,
            library,
        },
        svc,
        before,
    )
}

#[test]
fn scenario_export_re_export_after_date_rules_change_copies_nothing() {
    let (o, svc, before) = exported_under_earlier_rules();
    let p = plan(&svc, &o.library);
    assert_eq!(p.totals.files, 0, "{:?}", p.totals);
    assert_eq!(p.totals.adopted, 0);
    assert_eq!(p.totals.already_exported, 20);
    assert_eq!(p.totals.filed_under_earlier_rules, 20);
    assert!(p.warnings.is_empty(), "this catalog wrote the library");
    let r = run(&svc, &o.library);
    assert_eq!((r.copied, r.adopted), (0, 0));
    assert_eq!(
        snapshot(&o.library),
        before,
        "nothing moved, added or changed"
    );
    let _ = (&o.tmp, &o.src, &o.data);
}

#[test]
fn scenario_export_new_keeper_uses_the_current_rules_beside_old_copies() {
    let (o, svc, before) = exported_under_earlier_rules();
    write(
        &o.src.join("2019-07-04/IMG_9999.JPG"),
        &stripped(299, 88),
        at(2024, 3, 2, 12, 0),
    );
    scan(&svc);
    let p = plan(&svc, &o.library);
    let files: Vec<_> = p.files().collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].target.display(), "2019/2019-07-04/IMG_9999.JPG");
    assert_eq!(p.totals.filed_under_earlier_rules, 20);
    assert_eq!(run(&svc, &o.library).copied, 1);
    let after = snapshot(&o.library);
    let added: Vec<_> = after.keys().filter(|k| !before.contains_key(*k)).collect();
    // The new keeper and its sidecar (the Apple profile writes one), nothing else.
    assert_eq!(
        added,
        vec![
            Path::new("2019/2019-07-04/IMG_9999.JPG"),
            Path::new("2019/2019-07-04/IMG_9999.xmp")
        ]
    );
    for (k, v) in &before {
        if k.starts_with(".photosquirrel") {
            continue;
        }
        assert_eq!(after.get(k), Some(v), "{} unchanged", k.display());
    }
}

// ---- gate review: an unreadable file during the analysis refresh keeps its analysis (task 8.1) ----------------

/// Every file row's stage by relative path.
fn stages(data: &Path) -> BTreeMap<String, i64> {
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(data)).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let mut q = c.prepare("SELECT rel_path, stage FROM file").unwrap();
    q.query_map([], |r| {
        let p: Vec<u8> = r.get(0)?;
        Ok((String::from_utf8_lossy(&p).replace('\\', "/"), r.get(1)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn scenario_incremental_rescan_unreadable_file_keeps_its_analysis() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");
    let rgb = photo_rgb(1200, 900, 77);
    write(
        &src.join("best/IMG_0001.JPG"),
        &encode_jpeg(&rgb, 1200, 900, 95),
        at(2024, 3, 1, 12, 0),
    );
    write(
        &src.join("copies/IMG_0001_small.JPG"),
        &encode_jpeg(&rgb, 1200, 900, 60),
        at(2024, 3, 1, 12, 0),
    );
    for i in 2..4u64 {
        write(
            &src.join(format!("other/IMG_{i:04}.JPG")),
            &stripped(4_000 + i * 31, 90),
            at(2024, 3, 1, 12, 0),
        );
    }
    {
        let svc = open(&data, opts(FixedZone(0)), &src);
        scan(&svc);
        keep(&svc, "best/IMG_0001.JPG");
        assert_eq!(run(&svc, &library).copied, 3);
    }
    let before = stages(&data);
    // Rows as if probed before catalog migration 13.
    {
        let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&data)).unwrap();
        c.busy_timeout(Duration::from_secs(10)).unwrap();
        c.execute(
            "UPDATE file SET class_facts = NULL, classifier_hash = NULL",
            [],
        )
        .unwrap();
    }
    // The keeper's source becomes unreadable, then the refresh runs.
    let (fs, instr) = ps_scan::SourceFs::instrumented();
    let svc = open(
        &data,
        ScanOptions {
            fs,
            refresh_analysis: true,
            ..opts(FixedZone(0))
        },
        &src,
    );
    let root = svc.sources().unwrap()[0].root.clone();
    instr.deny_open(&root.join("best").join("IMG_0001.JPG"));
    let summary = scan(&svc);
    assert_eq!(summary.diff.needs_refresh, 4, "{:?}", summary.diff);
    // The summary counts what the refresh did and what is left (task 8.4).
    assert_eq!(summary.diff.refreshed, 3, "{:?}", summary.diff);
    let o = svc.run_overview(summary.run).unwrap();
    assert_eq!((o.refreshed, o.needs_refresh), (3, 1), "{o:?}");
    assert!(
        instr
            .opens()
            .iter()
            .any(|o| !o.ok && o.path.ends_with("IMG_0001.JPG")),
        "the keeper's open failed"
    );
    assert_eq!(stages(&data), before, "every row keeps its stage");
    let groups = svc
        .groups()
        .list(ps_service::groups::GroupOrder::Id, 0, 10)
        .unwrap();
    assert_eq!(groups.len(), 1, "the duplicate group survives: {groups:?}");
    let d = svc.groups().show(groups[0].id).unwrap();
    assert_eq!(d.members.len(), 2);
    let p = plan(&svc, &library);
    assert_eq!(p.totals.files, 0, "nothing new to copy: {:?}", p.totals);
    assert_eq!(p.totals.already_exported, 3);
}

// ---- gate review: duplicate review cards show the catalog date at its precision (task 8.2) -------------------

#[test]
fn review_cards_show_the_catalog_date_at_its_precision() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let rgb = photo_rgb(1200, 900, 93);
    write(
        &src.join("2019-07 Italy/IMG_0001.JPG"),
        &encode_jpeg(&rgb, 1200, 900, 92),
        at(2024, 3, 1, 12, 0),
    );
    write(
        &src.join("2019_07_04 Beach/beach.jpg"),
        &encode_jpeg(&rgb, 1200, 900, 80),
        at(2024, 3, 1, 12, 0),
    );
    let data = tmp.path().join("data");
    let svc = open(&data, opts(FixedZone(0)), &src);
    scan(&svc);
    let ids = keep(&svc, "IMG_0001.JPG");
    let store = ps_service::review::ReviewStore::open(&data);
    let captured = |name: &str| {
        let d = ps_service::review::show(&svc, &store, id_of(&ids, "IMG_0001.JPG")).unwrap();
        d.card(id_of(&ids, name)).unwrap().captured.clone()
    };
    assert_eq!(captured("IMG_0001.JPG").as_deref(), Some("July 2019"));
    assert_eq!(captured("beach.jpg").as_deref(), Some("4 July 2019"));
    // A re-dated row (columns changed, metadata JSON not) shows the columns.
    let ts = CaptureTime::from_parts(2019, 7, 5, 0, 0, 0).local_seconds * 1000;
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&data)).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c.execute(
        "UPDATE file SET capture_ts = ?1, capture_precision = 1 WHERE capture_precision = 1",
        [ts],
    )
    .unwrap();
    assert_eq!(captured("beach.jpg").as_deref(), Some("5 July 2019"));
}
