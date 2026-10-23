//! capture-dates scanning (tasks 4.1-4.5): the run's zone, re-dating unchanged rows from stored facts
//! (ADR-0071), resumable after a cancel, folder dates that never touch grouping, and the opt-in analysis
//! refresh of rows probed before class facts were stored.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::*;
use ps_catalog::FileRecord;
use ps_media::{FixedZone, RuleZone};
use ps_model::{
    CaptureTime, ClassifierSettings, DATE_LOGIC_VERSION, DatePrecision, DateProvenance, PhotoClass,
    SourceId, Stage,
};
use ps_scan::redate::{StaleDate, redate};
use ps_scan::{Control, ScanOptions};

fn utc_opts() -> ScanOptions {
    ScanOptions {
        zone: Some(Arc::new(FixedZone(0))),
        ..ScanOptions::default()
    }
}

fn set_mtime(path: &Path, t: SystemTime) {
    let f = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for mtime");
    f.set_modified(t).expect("set mtime");
}

fn at(y: i32, m: u8, d: u8) -> SystemTime {
    let s = CaptureTime::from_parts(y, m, d, 12, 0, 0).local_seconds;
    SystemTime::UNIX_EPOCH + Duration::from_secs(s as u64)
}

/// EXIF-less photos in dated folders, one in an undated folder and one with an EXIF date.
const DATED: &[&str] = &[
    "2019-07-04 Beach/IMG_0001.JPG",
    "2019-07-04 Beach/IMG_0002.JPG",
    "2019_07_05/IMG_0003.JPG",
    "2019-07 Italy/IMG_0004.JPG",
    "2019/08/IMG_0005.JPG",
];
const UNDATED: &str = "Misc/IMG_0006.JPG";
const EXIF: &str = "2019-07-04 Beach/IMG_0007.JPG";

fn write_dated_source(root: &Path) {
    for (i, rel) in DATED.iter().chain([&UNDATED]).enumerate() {
        let p = rel.split('/').fold(root.to_path_buf(), |p, c| p.join(c));
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(&p, ps_fixtures::media::jpeg(640, 480, 300 + i as u64, 85)).expect("write");
        set_mtime(&p, at(2024, 3, 1));
    }
    let spec = ps_fixtures::media::ExifSpec {
        date_time_original: Some("2018:01:02 10:00:00".into()),
        ..ps_fixtures::media::ExifSpec::canon_5d()
    };
    let p = root.join("2019-07-04 Beach").join("IMG_0007.JPG");
    std::fs::write(
        &p,
        ps_fixtures::media::jpeg_with_exif(640, 480, 399, 85, &spec),
    )
    .expect("write");
    set_mtime(&p, at(2024, 3, 1));
}

/// Rewrite every row without an embedded date as the previous date rules left it (ADR-0071's version 1): its
/// file time read as UTC, undated, version 1, and a metadata JSON saying so.
fn make_old_catalog(data: &Path) {
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(data)).expect("open catalog");
    c.busy_timeout(Duration::from_secs(10)).expect("busy");
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
    .expect("old rules");
}

fn rows(env: &Env, id: SourceId) -> BTreeMap<String, FileRecord> {
    env.files(id)
}

struct Dated {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: Env,
    id: SourceId,
}

fn dated(opts: ScanOptions) -> Dated {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("source");
    write_dated_source(&root);
    let env = env_with(opts);
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    Dated {
        _tmp: tmp,
        root,
        env,
        id,
    }
}

#[test]
fn new_files_are_dated_by_folder_names_with_the_run_zone() {
    let _g = shared();
    let d = dated(ScanOptions {
        zone: Some(Arc::new(RuleZone::LOS_ANGELES)),
        ..ScanOptions::default()
    });
    let r = rows(&d.env, d.id);
    let beach = &r["2019-07-04 Beach/IMG_0001.JPG"];
    assert_eq!(beach.capture_provenance, Some(DateProvenance::FolderName));
    assert_eq!(beach.capture_precision, DatePrecision::Day);
    assert_eq!(beach.date_version, DATE_LOGIC_VERSION);
    let italy = r["2019-07 Italy/IMG_0004.JPG"]
        .capture_date()
        .expect("date");
    assert_eq!(
        (italy.value.ymd(), italy.precision),
        ((2019, 7, 1), DatePrecision::Month)
    );
    // 2024-03-01 12:00 UTC is 04:00 in Los Angeles (standard time).
    let misc = r[UNDATED].capture_date().expect("date");
    assert_eq!(misc.provenance, DateProvenance::FileMtime);
    assert_eq!(misc.value.display(), "2024-03-01 04:00:00");
    assert_eq!(misc.value.offset_minutes, Some(-480));
    let exif = r[EXIF].capture_date().expect("date");
    assert_eq!(exif.provenance, DateProvenance::DateTimeOriginal);
    assert_eq!(exif.value.display(), "2018-01-02 10:00:00");
}

#[test]
fn scenario_incremental_rescan_old_catalog_re_dated_on_rescan() {
    let _g = shared();
    let mut d = dated(utc_opts());
    let exif_before = rows(&d.env, d.id)[EXIF].clone();
    make_old_catalog(&d.env.dir);
    let old = rows(&d.env, d.id);
    for rel in DATED {
        assert_eq!(
            old[*rel].capture_provenance,
            Some(DateProvenance::FileMtime),
            "{rel}"
        );
        assert_eq!(old[*rel].date_version, 1);
    }
    d.env.reopen(utc_opts());
    let summary = d.env.scan(&[d.id]);
    completed(&summary);
    assert_eq!(
        summary.diff.redated,
        DATED.len() as u64,
        "{:?}",
        summary.diff
    );
    assert_eq!(summary.diff.unchanged, DATED.len() as u64 + 2);
    assert_eq!(
        d.env.instr.total_bytes(),
        0,
        "re-dating reads no file content"
    );
    assert_eq!(d.env.instr.open_count(), 0, "re-dating opens no file");
    let r = rows(&d.env, d.id);
    for rel in DATED {
        let c = r[*rel].capture_date().expect("date");
        assert_eq!(c.provenance, DateProvenance::FolderName, "{rel}");
        assert!(!c.undated);
        assert_eq!(r[*rel].date_version, DATE_LOGIC_VERSION);
    }
    assert_eq!(
        r["2019-07-04 Beach/IMG_0002.JPG"].capture_precision,
        DatePrecision::Day
    );
    assert_eq!(
        r["2019/08/IMG_0005.JPG"].capture_precision,
        DatePrecision::Month
    );
    // The undated file keeps its (UTC) file time and is brought to the current version; the EXIF file is
    // untouched.
    assert_eq!(
        r[UNDATED].capture_provenance,
        Some(DateProvenance::FileMtime)
    );
    assert_eq!(r[UNDATED].date_version, DATE_LOGIC_VERSION);
    assert_eq!(normalized(&r[EXIF]), normalized(&exif_before));
    assert_eq!(r[EXIF].date_version, DATE_LOGIC_VERSION);
    // Re-dating changes no grouping input: the (full, after the reopen) regroup writes nothing.
    let g = summary.grouping.expect("regroup");
    assert_eq!((g.groups_written, g.groups_removed), (0, 0), "{g:?}");
    let _ = &d.root;
}

#[test]
fn scenario_incremental_rescan_re_dating_happens_once() {
    let _g = shared();
    let mut d = dated(utc_opts());
    make_old_catalog(&d.env.dir);
    d.env.reopen(utc_opts());
    assert_eq!(d.env.scan(&[d.id]).diff.redated, DATED.len() as u64);
    let after_first = rows(&d.env, d.id);
    d.env.reopen(utc_opts());
    let again = d.env.scan(&[d.id]);
    completed(&again);
    assert_eq!(again.diff.redated, 0);
    assert_eq!(d.env.instr.total_bytes(), 0);
    let after_second = rows(&d.env, d.id);
    for (p, r) in &after_second {
        assert_eq!(r.capture_date(), after_first[p].capture_date(), "{p}");
    }
}

/// Task 4.3: a cancel during the re-date pass leaves the unwritten rows at version 1; the next rescan
/// finishes them.
#[test]
fn cancel_during_re_dating_leaves_the_rest_for_the_next_rescan() {
    let _g = shared();
    let mut d = dated(utc_opts());
    make_old_catalog(&d.env.dir);
    let stale: Vec<StaleDate> = rows(&d.env, d.id)
        .into_values()
        .filter(|r| r.date_version < DATE_LOGIC_VERSION)
        .map(|r| StaleDate {
            id: r.id,
            path: r.key.path.clone(),
            current: r.capture_date(),
        })
        .collect();
    assert_eq!(stale.len(), DATED.len() + 1);
    let ctl = Control::new();
    let writer = d.env.s().catalog().writer().clone();
    let mut chunks = 0;
    let out = redate(
        d.id,
        &stale,
        d.env.readers(),
        &FixedZone(0),
        &ctl,
        2,
        |cmd| {
            writer.submit(cmd).expect("submit");
            chunks += 1;
            ctl.cancel();
        },
    )
    .expect("redate");
    writer.flush().expect("flush");
    assert!(out.cancelled);
    assert_eq!((chunks, out.written), (1, 2));
    let r = rows(&d.env, d.id);
    let current = r
        .values()
        .filter(|r| r.date_version == DATE_LOGIC_VERSION)
        .count();
    assert_eq!(current, 2 + 1, "the first chunk and the EXIF row");
    d.env.reopen(utc_opts());
    let summary = d.env.scan(&[d.id]);
    completed(&summary);
    let r = rows(&d.env, d.id);
    assert!(r.values().all(|r| r.date_version == DATE_LOGIC_VERSION));
    for rel in DATED {
        assert_eq!(
            r[*rel].capture_provenance,
            Some(DateProvenance::FolderName),
            "{rel}"
        );
    }
    assert_eq!(d.env.instr.total_bytes(), 0);
}

// ---- folder dates and grouping (task 4.1) -------------------------------------------------------------------

/// The corpus with every EXIF-stripped copy moved one folder down, into `<sub>/`.
fn corpus_with_stripped_in(dst: &Path, sub: &str) -> Vec<(String, String)> {
    let c = corpus();
    copy_tree(&c.root, dst);
    let mut moved = Vec::new();
    for f in &c.truth.files {
        if f.category != ps_fixtures::Category::ExifStripped {
            continue;
        }
        let (dir, name) = f.path.rsplit_once('/').expect("in a drive");
        let to = format!("{dir}/{sub}/{name}");
        let from_abs = dst.join(&f.path);
        let to_abs = dst.join(&to);
        std::fs::create_dir_all(to_abs.parent().expect("parent")).expect("mkdir");
        std::fs::rename(&from_abs, &to_abs).expect("move inside the test copy");
        moved.push((f.path.clone(), to));
    }
    moved
}

/// Groups as sorted member paths (with `<drive>/` prefixes) and keepers, `sub` folders renamed to `as_sub`.
fn groups_of(
    env: &Env,
    ids: &[(String, SourceId)],
    sub: &str,
    as_sub: &str,
) -> Vec<(Vec<String>, String)> {
    let r = env.readers();
    let path_of = |f: ps_model::FileId| {
        let rec = r.file_by_id(f).expect("file").expect("present");
        let drive = &ids
            .iter()
            .find(|(_, s)| *s == rec.key.source)
            .expect("source")
            .0;
        format!("{drive}/{}", rec.key.path.display())
            .replace(&format!("/{sub}/"), &format!("/{as_sub}/"))
    };
    let (n, _, _) = r.group_totals().expect("totals");
    let mut out: Vec<(Vec<String>, String)> = r
        .groups(ps_catalog::GroupOrder::Id, 0, n)
        .expect("groups")
        .iter()
        .map(|s| {
            let g = r.group(s.id).expect("group").expect("present");
            let mut m: Vec<String> = g.members.iter().map(|m| path_of(m.file)).collect();
            m.sort();
            (m, path_of(g.keeper))
        })
        .collect();
    out.sort();
    out
}

/// Scan a corpus copy drive by drive, so file ids (the last tie-break between byte-identical copies at equal
/// depth) follow the same order in both trees; drives scanned in parallel interleave their ids by timing.
fn scan_corpus_copy(root: &Path) -> (Env, Vec<(String, SourceId)>) {
    let env = env_with(utc_opts());
    let mut ids = Vec::new();
    for drive in &corpus().truth.drives {
        let id = env.add(&root.join(drive));
        completed(&env.scan(&[id]));
        ids.push((drive.clone(), id));
    }
    (env, ids)
}

#[test]
fn scenario_media_metadata_folder_date_does_not_change_duplicate_groups() {
    let _g = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    // A dated folder before every corpus file time, so the plausibility rule keeps every folder date.
    let dated_sub = "2000-01-01 Moved";
    let plain = tmp.path().join("plain");
    let dated = tmp.path().join("dated");
    let moved = corpus_with_stripped_in(&plain, "Moved");
    assert!(!moved.is_empty(), "the corpus has stripped copies");
    corpus_with_stripped_in(&dated, dated_sub);

    let (a, a_ids) = scan_corpus_copy(&plain);
    let (b, b_ids) = scan_corpus_copy(&dated);
    // Every analysed stripped copy in the dated tree got its folder date; in the plain tree none of the moved
    // copies did (other EXIF-less files of the corpus sit in dated folders in both trees).
    let moved_dated = |env: &Env, ids: &[(String, SourceId)], sub: &str| {
        let all: Vec<(String, FileRecord)> = ids
            .iter()
            .flat_map(|(_, s)| env.files(*s).into_iter())
            .filter(|(p, r)| p.contains(&format!("/{sub}/")) && r.stage == Stage::Thumbed)
            .collect();
        let dated = all
            .iter()
            .filter(|(_, r)| r.capture_provenance == Some(DateProvenance::FolderName))
            .count();
        (all.len(), dated)
    };
    let (n, dated_a) = moved_dated(&a, &a_ids, "Moved");
    assert!(n > 0, "stripped copies analysed");
    assert_eq!(dated_a, 0);
    assert_eq!(moved_dated(&b, &b_ids, dated_sub), (n, n));

    let ga = groups_of(&a, &a_ids, "Moved", "Moved");
    let gb = groups_of(&b, &b_ids, dated_sub, "Moved");
    assert!(!ga.is_empty());
    let only_a: Vec<_> = ga.iter().filter(|g| !gb.contains(g)).collect();
    let only_b: Vec<_> = gb.iter().filter(|g| !ga.contains(g)).collect();
    assert!(
        only_a.is_empty() && only_b.is_empty(),
        "the same groups and keepers with and without folder dates:\nwithout: {only_a:#?}\nwith: {only_b:#?}"
    );
}

// ---- the opt-in analysis refresh (task 4.5) -----------------------------------------------------------------

/// 50 plain JPEGs: 20 photos larger than the probe window, 20 junk at 150 px short side, 10 junk icons.
fn write_old_rows_source(root: &Path) -> Vec<(String, u32)> {
    std::fs::create_dir_all(root).expect("mkdir");
    let mut out = Vec::new();
    for i in 0..50u64 {
        let (name, w, h) = match i {
            0..20 => (format!("photo_{i:02}.jpg"), 1800, 1350),
            20..40 => (format!("narrow_{i:02}.jpg"), 150, 1000),
            _ => (format!("icon_{i:02}.jpg"), 64, 64),
        };
        std::fs::write(
            root.join(&name),
            ps_fixtures::media::jpeg(w, h, 500 + i, 97),
        )
        .expect("write");
        out.push((name, w.min(h)));
    }
    out
}

/// Rows as if probed before catalog migration 13: no class facts, no classifier hash.
fn drop_class_facts(data: &Path) {
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(data)).expect("open catalog");
    c.busy_timeout(Duration::from_secs(10)).expect("busy");
    let n = c
        .execute(
            "UPDATE file SET class_facts = NULL, classifier_hash = NULL",
            [],
        )
        .expect("drop facts");
    assert_eq!(n, 50);
}

fn facts_count(env: &Env) -> i64 {
    env.readers()
        .with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM file WHERE class_facts IS NOT NULL AND classifier_hash IS NOT NULL",
                [],
                |r| r.get(0),
            )?)
        })
        .expect("count")
}

struct OldRows {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: Env,
    id: SourceId,
    files: Vec<(String, u32)>,
}

fn old_rows() -> OldRows {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("old");
    let files = write_old_rows_source(&root);
    let mut env = env_with(utc_opts());
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    drop_class_facts(&env.dir);
    env.reopen(utc_opts());
    OldRows {
        _tmp: tmp,
        root,
        env,
        id,
        files,
    }
}

#[test]
fn scenario_incremental_rescan_files_without_facts_counted_not_read() {
    let _g = shared();
    let o = old_rows();
    let summary = o.env.scan(&[o.id]);
    completed(&summary);
    assert_eq!(summary.diff.needs_refresh, 50, "{:?}", summary.diff);
    assert_eq!(o.env.instr.total_bytes(), 0);
    assert_eq!(o.env.instr.open_count(), 0);
    assert_eq!(facts_count(&o.env), 0, "nothing re-read, nothing stored");
    let _ = &o.root;
}

#[test]
fn scenario_incremental_rescan_refresh_re_classifies_old_rows() {
    let _g = shared();
    let mut o = old_rows();
    let before = o.env.files(o.id);
    let narrow_before: Vec<_> = before
        .iter()
        .filter(|(p, _)| p.starts_with("narrow_"))
        .map(|(_, r)| r.class)
        .collect();
    assert!(narrow_before.iter().all(|c| *c == Some(PhotoClass::Junk)));
    // The refresh as `photosquirrel-cli scan --refresh-analysis` runs it, after lowering the minimum side.
    o.env.reopen(ScanOptions {
        classifier: ClassifierSettings {
            min_side_px: 100,
            min_pixels: 100_000,
            ..ClassifierSettings::default()
        },
        refresh_analysis: true,
        ..utc_opts()
    });
    let summary = o.env.scan(&[o.id]);
    completed(&summary);
    assert_eq!(summary.diff.needs_refresh, 50);
    let root = o.env.root(o.id);
    let per_file = o.env.instr.bytes_per_file();
    for (name, _) in &o.files {
        let n = per_file.get(&root.join(name)).copied().unwrap_or(0);
        assert!(n > 0, "{name} read for the refresh");
        assert!(n <= ps_media::PROBE_LIMIT as u64, "{name}: {n} bytes");
    }
    // The large photos were read only up to the probe limit.
    let photo = root.join("photo_00.jpg");
    assert!(std::fs::metadata(&photo).expect("meta").len() > ps_media::PROBE_LIMIT as u64);
    assert_eq!(per_file[&photo], ps_media::PROBE_LIMIT as u64);
    assert_eq!(
        facts_count(&o.env),
        50,
        "every refreshed file has stored facts"
    );
    let after = o.env.files(o.id);
    for (name, side) in &o.files {
        let r = &after[name];
        if (100..200).contains(side) {
            assert_eq!(
                r.class,
                Some(PhotoClass::LikelyPhoto),
                "{name} re-classified"
            );
            assert_eq!(r.stage, Stage::Thumbed, "{name} analysed after waking");
        } else if *side < 100 {
            assert_eq!(r.class, Some(PhotoClass::Junk), "{name}");
        } else {
            assert_eq!(r.class, before[name].class, "{name}");
            assert_eq!(r.stage, Stage::Thumbed);
            assert_eq!(
                normalized(r),
                normalized(&before[name]),
                "{name} keeps its analysis"
            );
        }
    }
    // A second rescan has nothing left to refresh and reads nothing.
    o.env.reopen(ScanOptions {
        refresh_analysis: true,
        ..utc_opts()
    });
    let again = o.env.scan(&[o.id]);
    assert_eq!(again.diff.needs_refresh, 0);
}

/// Gate review (task 8.3): a pause parks the re-date pass between chunks, idle, and a resume finishes it.
#[test]
fn pause_parks_the_re_date_pass_between_chunks() {
    let _g = shared();
    let d = dated(utc_opts());
    make_old_catalog(&d.env.dir);
    let stale: Vec<StaleDate> = rows(&d.env, d.id)
        .into_values()
        .filter(|r| r.date_version < DATE_LOGIC_VERSION)
        .map(|r| StaleDate {
            id: r.id,
            path: r.key.path.clone(),
            current: r.capture_date(),
        })
        .collect();
    assert_eq!(stale.len(), DATED.len() + 1);
    let ctl = Control::new();
    let writer = d.env.s().catalog().writer().clone();
    let chunks = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        let pass = sc.spawn(|| {
            redate(
                d.id,
                &stale,
                d.env.readers(),
                &FixedZone(0),
                &ctl,
                2,
                |cmd| {
                    writer.submit(cmd).expect("submit");
                    if chunks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        ctl.pause();
                    }
                },
            )
            .expect("redate")
        });
        wait_until(Duration::from_secs(10), "the pass parks", || {
            chunks.load(std::sync::atomic::Ordering::SeqCst) == 1 && ctl.busy_count() == 0
        });
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            chunks.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "nothing re-dated while paused"
        );
        assert_eq!(ctl.busy_count(), 0, "a parked pass is idle");
        ctl.resume();
        let out = pass.join().expect("pass");
        assert!(!out.cancelled);
        assert_eq!(out.written, stale.len() as u64);
    });
    assert_eq!(
        chunks.load(std::sync::atomic::Ordering::SeqCst),
        stale.len().div_ceil(2)
    );
}

/// Gate review (task 8.3): a cancelled refresh run, resumed by a scanner whose options do not ask for the
/// refresh (`photosquirrel-cli scan resume`), still refreshes: the run remembers it (migration 18, ADR-0082).
#[test]
fn resumed_refresh_run_still_refreshes() {
    let _g = shared();
    let mut o = old_rows();
    o.env.reopen(ScanOptions {
        refresh_analysis: true,
        read_delay: Some(Duration::from_millis(40)),
        ..utc_opts()
    });
    let run = o.env.s().start_scan(&[o.id]).expect("start");
    wait_until(Duration::from_secs(20), "some files refreshed", || {
        facts_count(&o.env) > 0
    });
    o.env.s().cancel().expect("cancel");
    o.env.s().wait().expect("summary");
    let partial = facts_count(&o.env);
    assert!(partial < 50, "cancelled before the end: {partial}");
    let rec = o.env.readers().run(run).expect("run").expect("record");
    assert!(rec.refresh_analysis, "the run records the refresh");
    // Resume without the option.
    o.env.reopen(utc_opts());
    o.env.s().resume_run(run).expect("resume");
    let summary = o.env.s().wait().expect("summary");
    completed(&summary);
    assert_eq!(
        facts_count(&o.env),
        50,
        "the resumed run refreshed the rest"
    );
    // A plain scan does not record a refresh.
    let plain = o.env.s().start_scan(&[o.id]).expect("start");
    o.env.s().wait().expect("summary");
    assert!(
        !o.env
            .readers()
            .run(plain)
            .expect("run")
            .expect("record")
            .refresh_analysis
    );
}
