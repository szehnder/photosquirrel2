//! Group edits while a cleanup run is moving files (ADR-0099), end to end through the service on a hand-made
//! source in a temp folder: a 1600x1067 JPEG (the keeper, exported), its 800x533 resize (covered by the keeper's
//! library copy) and an unrelated photo (exported), all planned.
//!
//! The gate review's repro: right after the run's first rename, the resize is separated from its group. It is
//! then its own photo with no copy in the library, and must stay where it is. The service refuses the edit
//! while the run holds the slot ("a cleanup is running"); the same edit written past that check, straight to
//! the catalog as a rescan's regroup would, moves the constraints generation on, and the run derives the
//! dispositions of its remaining files again and skips the resize.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use ps_export::quarantine::{QuarantineHooks, RunProgress, SkipReason};
use ps_model::{CleanupMove, Disposition, FileId, MoveState, SourceId};
use ps_service::cleanup::RunOptions;
use ps_service::export::profile_settings;
use ps_service::groups::GroupError;
use ps_service::ops::Busy;
use ps_service::scan::{ScanOptions, ScanService};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
}

struct Setup {
    _tmp: tempfile::TempDir,
    svc: ScanService,
    drive: PathBuf,
    library: PathBuf,
    full: FileId,
    small: FileId,
    other: FileId,
    group: FileId,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let (w, h) = (1600, 1067);
    let full = ps_media::RgbImage {
        width: w,
        height: h,
        data: ps_fixtures::media::photo_rgb(w, h, 9091),
    };
    let small = ps_media::decode::resize_to_long_edge(&full, 800);
    let drive = tmp.path().join("drive-a");
    write(
        &drive,
        "A/IMG_0100.jpg",
        &ps_fixtures::media::encode_jpeg(
            &ps_fixtures::media::photo_rgb(900, 600, 77),
            900,
            600,
            90,
        ),
    );
    write(
        &drive,
        "B/IMG_0001.jpg",
        &ps_fixtures::media::encode_jpeg(&full.data, w, h, 92),
    );
    write(
        &drive,
        "C/IMG_0001_small.jpg",
        &ps_fixtures::media::encode_jpeg(&small.data, small.width, small.height, 85),
    );
    let library = tmp.path().join("Library");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&drive).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let (_, report) = svc
        .exports()
        .run(
            &library,
            profile_settings("google").unwrap(),
            &Default::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(report.copied, 2, "{report:?}");
    let (files, d, _) = svc.exports().dispositions(&library, false).unwrap();
    let id = |name: &str| {
        files
            .iter()
            .find(|f| f.rel_path.display().ends_with(name))
            .unwrap()
            .file
    };
    let (full, small, other) = (
        id("IMG_0001.jpg"),
        id("IMG_0001_small.jpg"),
        id("IMG_0100.jpg"),
    );
    let disp = |f: FileId| {
        files
            .iter()
            .zip(&d)
            .find(|(x, _)| x.file == f)
            .unwrap()
            .1
            .disposition
    };
    assert_eq!(disp(full), Disposition::Exported);
    assert_eq!(disp(small), Disposition::Covered);
    assert_eq!(disp(other), Disposition::Exported);
    let group = svc
        .engine()
        .catalog()
        .read()
        .group_of_file(small)
        .unwrap()
        .expect("the resize is grouped with its original");
    Setup {
        _tmp: tmp,
        svc,
        drive,
        library,
        full,
        small,
        other,
        group,
    }
}

/// After the first rename: the edit through the service (refused), then the same separation written straight to
/// the catalog and a regroup.
struct SeparateAfterFirstMove<'a> {
    s: &'a Setup,
    refused: Mutex<Option<String>>,
}

impl QuarantineHooks for SeparateAfterFirstMove<'_> {
    fn after_rename(&self, _m: &CleanupMove) {
        let mut refused = self.refused.lock().unwrap();
        if refused.is_some() {
            return;
        }
        let s = self.s;
        match s.svc.groups().separate(s.group, s.small) {
            Err(GroupError::Busy(b @ Busy::Cleanup(_))) => *refused = Some(b.to_string()),
            other => panic!("the edit is refused during the run: {other:?}"),
        }
        let r = s.svc.engine().catalog().read();
        let content = |f: FileId| r.file_by_id(f).unwrap().unwrap().identity.unwrap();
        s.svc
            .engine()
            .catalog()
            .writer()
            .set_separations(vec![(content(s.small), content(s.full))], true)
            .unwrap();
        s.svc.engine().regroup().unwrap();
    }
}

#[test]
fn scenario_cleanup_group_edit_during_a_cleanup_run_is_respected() {
    let s = setup();
    let plan = s.svc.cleanups().plan(SourceId(1), None, None).unwrap();
    let order: Vec<FileId> = plan.moves.iter().map(|m| m.file).collect();
    assert_eq!(order, vec![s.other, s.full, s.small], "{:?}", plan.moves);
    let run = plan.summary.unwrap().run.id;
    let small_path = s.drive.join("C/IMG_0001_small.jpg");
    let before = std::fs::read(&small_path).unwrap();
    let hooks = SeparateAfterFirstMove {
        s: &s,
        refused: Mutex::new(None),
    };
    let report = s
        .svc
        .cleanups()
        .run_with_hooks(
            run,
            RunOptions::default(),
            &hooks,
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    let refused = hooks.refused.lock().unwrap().clone().expect("refused");
    assert!(refused.contains("a cleanup is running"), "{refused}");
    // The separation reached the catalog: the resize is its own photo now, not in the library.
    let (files, d, _) = s.svc.exports().dispositions(&s.library, false).unwrap();
    let now = files
        .iter()
        .zip(&d)
        .find(|(f, _)| f.file == s.small)
        .unwrap()
        .1;
    assert_eq!(now.disposition, Disposition::Pending);
    // The original and the other photo moved; the resize was checked again and kept.
    assert_eq!(report.moved, 2, "{report:?}");
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    let sk = &report.skipped[0];
    assert_eq!(sk.file, s.small);
    assert!(
        matches!(sk.reason, SkipReason::NoLongerSafe(_)),
        "{:?}",
        sk.reason
    );
    assert_eq!(std::fs::read(&small_path).unwrap(), before, "in place");
    let (_, moves) = s.svc.cleanups().run_detail(run).unwrap();
    let m = moves.iter().find(|m| m.file == s.small).unwrap();
    assert_eq!(m.state, MoveState::Skipped);
    // Edits are accepted again once the run ended.
    assert!(s.svc.cleanup_running().is_none());
    s.svc.groups().regroup().unwrap();
}

/// A cleanup run does not start while an edit is being saved, and an edit is refused while a run holds the slot.
#[test]
fn edits_and_cleanup_runs_exclude_each_other() {
    let s = setup();
    let plan = s.svc.cleanups().plan(SourceId(1), None, None).unwrap();
    let run = plan.summary.unwrap().run.id;
    let edit = s.svc.edit().unwrap();
    let err = s
        .svc
        .cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("being saved"), "{err}");
    drop(edit);
    let slot = s
        .svc
        .begin(ps_service::ops::Operation::CleanupRun(run))
        .unwrap();
    assert_eq!(s.svc.cleanup_running(), Some(run));
    let err = s.svc.groups().clear_keeper(s.group).unwrap_err();
    assert!(err.to_string().contains("a cleanup is running"), "{err}");
    drop(slot);
    assert_eq!(s.svc.cleanup_running(), None);
    s.svc.groups().clear_keeper(s.group).unwrap();
}
