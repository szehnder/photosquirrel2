//! `cleanup` scenario of burst-frame-identity (ADR-0105) through the service: a cleanup made on the strength of a
//! merged burst, then the rescan that splits the frames. A test binary of its own: it switches the process-wide
//! frame-identity hook.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ps_export::ExecConfig;
use ps_export::quarantine::RunProgress;
use ps_fixtures::dng::{converted_burst_frame, converted_burst_frame_with};
use ps_model::{Disposition, FileId, RelPath, SourceId};
use ps_service::cleanup::RunOptions;
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// Frames 34 and 35 of a converted burst as on the owner's card (see the ps-scan test of the same name): CR2 and
/// JPEG in `DCIM/101CANON`, DNG and a JPEG copy in `2020/2020-04-20`. Frame 34's DNG shows frame 35 as the
/// converter rendered it and is the larger file, so without frame identity it joins frame 35 as its keeper.
fn card(root: &Path) {
    let frames = [
        converted_burst_frame_with(3, 0, 3, 960, 1),
        converted_burst_frame(3, 1, 140, 960),
    ];
    for (i, f) in frames.iter().enumerate() {
        let dng = if i == 0 {
            ps_fixtures::media::pad_to(&f.dng, f.dng.len() + 4096)
        } else {
            f.dng.clone()
        };
        write(&root.join(format!("DCIM/101CANON/{}.CR2", f.stem)), &f.cr2);
        write(&root.join(format!("DCIM/101CANON/{}.JPG", f.stem)), &f.jpeg);
        write(&root.join(format!("2020/2020-04-20/{}.DNG", f.stem)), &dng);
        write(
            &root.join(format!("2020/2020-04-20/{}.JPG", f.stem)),
            &f.jpeg,
        );
    }
}

/// One test at a time: the frame-identity hook is process-wide.
static HOOK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hook() -> std::sync::MutexGuard<'static, ()> {
    HOOK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn scratch() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(base).unwrap();
    tempfile::Builder::new()
        .prefix("ps-frame-identity-")
        .tempdir_in(base)
        .unwrap()
}

fn rel(s: &str) -> RelPath {
    RelPath::from_bytes(s.as_bytes().to_vec()).unwrap()
}

/// File id by path relative to the source.
fn ids(svc: &ScanService, library: &Path) -> BTreeMap<String, FileId> {
    let (files, _, _) = svc.exports().dispositions(library, false).unwrap();
    files
        .iter()
        .map(|f| (f.rel_path.display().into_owned(), f.file))
        .collect()
}

fn export(svc: &ScanService, library: &Path) -> ps_export::ExportReport {
    let (_, report) = svc
        .exports()
        .run(
            library,
            profile_settings("lightroom").unwrap(),
            &ExecConfig::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    report
}

/// Files in the library (relative paths).
fn library_files(library: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![library.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(library).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn scenario_cleanup_quarantined_frame_of_a_split_burst_is_pending() {
    let _g = hook();
    let tmp = scratch();
    let root = tmp.path().join("card");
    card(&root);
    let root = root
        .canonicalize()
        .map(ps_model::path::simplify_canonical)
        .unwrap();
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");

    // The build before ADR-0105: scan, export, clean up DCIM. Frame 35's CR2 is quarantined as covered by
    // frame 34's DNG.
    ps_model::frame::set_frame_identity_enabled(false);
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let src = svc.add_source(&root).unwrap().source.id;
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    export(&svc, &library);
    let id = ids(&svc, &library);
    let plan = svc.cleanups().plan(src, Some(rel("DCIM")), None).unwrap();
    let cr2 = id["DCIM/101CANON/IMG_0035.CR2"];
    let mv = plan
        .moves
        .iter()
        .find(|m| m.file == cr2)
        .expect("frame 35's CR2 planned")
        .clone();
    assert!(
        {
            let p = mv.covering_library_path.display();
            p.contains("IMG_0034") && p.ends_with(".DNG")
        },
        "covered by frame 34's DNG: {}",
        mv.covering_library_path
    );
    let run = plan.summary.as_ref().unwrap().run.id;
    let r = svc
        .cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    assert_eq!(r.moved, plan.moves.len() as u64, "{:?}", r.skipped);
    let q = mv.quarantine_rel_path.under(&root);
    assert!(q.is_file());
    drop(svc);
    ps_model::frame::set_frame_identity_enabled(true);

    // Upgraded (migration 23 leaves RAW rows at metadata version 0), then rescanned: the frames split.
    {
        let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&data)).unwrap();
        c.execute_batch(&ps_catalog::db::frame_identity_reset_sql())
            .unwrap();
    }
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    svc.start(&[]).unwrap();
    let summary = svc.wait().unwrap();
    assert!(summary.diff.frame_reread >= 2, "{:?}", summary.diff);
    assert!(
        summary
            .grouping
            .as_ref()
            .is_some_and(|g| g.groups_split >= 1),
        "{:?}",
        summary.grouping
    );

    // Frame 35's CR2: pending, with its quarantine path, the group change and how to restore it.
    let (files, d, _) = svc.exports().dispositions(&library, false).unwrap();
    let (_, x) = files.iter().zip(&d).find(|(f, _)| f.file == cr2).unwrap();
    assert_eq!(x.disposition, Disposition::Pending);
    assert!(x.regrouped, "{x:?}");
    assert_eq!(x.quarantine_path.as_ref(), Some(&mv.quarantine_rel_path));
    let why = ps_service::library::file_disposition_reason(x, None);
    assert!(
        why.contains(&mv.quarantine_rel_path.display().into_owned())
            && why.contains("group changed")
            && why.contains("cleanup undo"),
        "{why}"
    );
    let source_report = |svc: &ScanService| {
        svc.exports()
            .completion(None, false)
            .unwrap()
            .report
            .sources
            .into_iter()
            .find(|s| s.source == SourceId(src.0))
            .unwrap()
    };
    let s = source_report(&svc);
    assert!(!s.counts.fully_accounted(), "{:?}", s.counts);
    assert!(s.counts.pending_regrouped >= 1, "{:?}", s.counts);
    assert!(
        s.badge.label().contains("group changed"),
        "{}",
        s.badge.label()
    );
    assert!(q.is_file(), "nothing moved back by itself");

    // The next export copies exactly the split frame's keeper set (its DNG and the JPEG beside it); then the
    // drive is accounted for again.
    let before = library_files(&library);
    let report = export(&svc, &library);
    let after = library_files(&library);
    // The lightroom profile writes an XMP sidecar beside the RAW keeper.
    let new: Vec<_> = after
        .iter()
        .filter(|p| !before.contains(p))
        .filter(|p| p.extension().is_none_or(|e| e != "xmp"))
        .collect();
    assert!(report.copied >= 2, "{report:?}");
    assert_eq!(new.len(), 2, "{new:?}");
    assert!(
        new.iter()
            .any(|p| p.to_string_lossy().ends_with("IMG_0035.DNG"))
            && new
                .iter()
                .any(|p| p.to_string_lossy().ends_with("IMG_0035.JPG")),
        "{new:?}"
    );
    assert!(
        source_report(&svc).counts.fully_accounted(),
        "{:?}",
        source_report(&svc).counts
    );
}

#[test]
fn scenario_cleanup_stale_groups_refused_before_the_rescan() {
    let _g = hook();
    let tmp = scratch();
    let root = tmp.path().join("card");
    card(&root);
    let root = root
        .canonicalize()
        .map(ps_model::path::simplify_canonical)
        .unwrap();
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");

    // The build before ADR-0105: scan, export, and a cleanup plan of DCIM that counts frame 35's CR2 as covered
    // by frame 34's DNG. Not run yet.
    ps_model::frame::set_frame_identity_enabled(false);
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let src = svc.add_source(&root).unwrap().source.id;
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    export(&svc, &library);
    let id = ids(&svc, &library);
    let cr2 = id["DCIM/101CANON/IMG_0035.CR2"];
    let old_plan = svc.cleanups().plan(src, Some(rel("DCIM")), None).unwrap();
    assert!(old_plan.moves.iter().any(|m| m.file == cr2));
    let old_run = old_plan.summary.as_ref().unwrap().run.id;
    drop(svc);
    ps_model::frame::set_frame_identity_enabled(true);

    // Upgraded (migration 23), not rescanned yet: the report flags the source, plan and run are refused.
    {
        let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&data)).unwrap();
        c.execute_batch(&ps_catalog::db::frame_identity_reset_sql())
            .unwrap();
    }
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let source_report = |svc: &ScanService| {
        svc.exports()
            .completion(None, false)
            .unwrap()
            .report
            .sources
            .into_iter()
            .find(|s| s.source == SourceId(src.0))
            .unwrap()
    };
    assert!(source_report(&svc).rescan_needed > 0);
    let refused = |e: ps_service::cleanup::CleanupError| {
        assert!(
            matches!(e, ps_service::cleanup::CleanupError::RescanFirst { files } if files > 0),
            "{e}"
        );
        assert!(e.to_string().contains("rescan"), "{e}");
    };
    refused(
        svc.cleanups()
            .plan(src, Some(rel("DCIM")), None)
            .unwrap_err(),
    );
    refused(
        svc.cleanups()
            .run(
                old_run,
                RunOptions::default(),
                &AtomicBool::new(false),
                &RunProgress::default(),
            )
            .unwrap_err(),
    );
    assert!(
        root.join("DCIM/101CANON/IMG_0035.CR2").is_file(),
        "nothing moved"
    );

    // After the rescan the report is confirmed again and a new plan never counts frame 35's CR2 as covered by a
    // copy of frame 34.
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    assert_eq!(source_report(&svc).rescan_needed, 0);
    svc.cleanups().discard(old_run).unwrap();
    let plan = svc.cleanups().plan(src, Some(rel("DCIM")), None).unwrap();
    for m in plan.moves.iter().filter(|m| m.file == cr2) {
        assert!(
            !m.covering_library_path.display().contains("IMG_0034"),
            "{}",
            m.covering_library_path
        );
    }
}

/// ADR-0108: a quarantined file removed from the quarantine folder by hand is marked current quietly, not reported
/// unreadable on every rescan.
#[test]
fn a_quarantined_file_gone_from_the_quarantine_folder_is_not_an_error() {
    let _g = hook();
    let tmp = scratch();
    let root = tmp.path().join("card");
    card(&root);
    let root = root
        .canonicalize()
        .map(ps_model::path::simplify_canonical)
        .unwrap();
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");
    ps_model::frame::set_frame_identity_enabled(false);
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let src = svc.add_source(&root).unwrap().source.id;
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    export(&svc, &library);
    let plan = svc.cleanups().plan(src, Some(rel("DCIM")), None).unwrap();
    let run = plan.summary.as_ref().unwrap().run.id;
    svc.cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    drop(svc);
    ps_model::frame::set_frame_identity_enabled(true);
    let mut removed = 0;
    for m in &plan.moves {
        let q = m.quarantine_rel_path.under(&root);
        if q.is_file() {
            fs::remove_file(&q).unwrap();
            removed += 1;
        }
    }
    assert!(removed > 0);
    let catalog = ps_catalog::db::catalog_path(&data);
    {
        let c = rusqlite::Connection::open(&catalog).unwrap();
        c.execute_batch(&ps_catalog::db::frame_identity_reset_sql())
            .unwrap();
    }
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    for _ in 0..2 {
        svc.start(&[]).unwrap();
        let s = svc.wait().unwrap();
        assert_eq!(s.errors, 0, "{s:?}");
    }
    drop(svc);
    let c = rusqlite::Connection::open(&catalog).unwrap();
    let stale: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM file WHERE meta_version < 1 AND stage = 3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
}
