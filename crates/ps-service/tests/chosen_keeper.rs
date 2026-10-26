//! Cleanup safety around a keeper the user chose (ADR-0096), end to end through the service with the fake HEIC
//! decoder: the gate re-review's two repros on fixtures.
//!
//! - A resized 800x533 JPEG is exported from source A; its 3000x2000 HEIC original on source B is decoded and
//!   becomes the keeper (superseded, ADR-0093). The user makes the JPEG the keeper: the HEIC counts as accounted
//!   for by that choice, but it ranks above the JPEG automatically, so cleanup never plans it ("better than the
//!   keeper you chose") and the inspector says so instead of calling the JPEG its best copy.
//! - A plan stored without that rule (as an older version stored it, here injected through the catalog) still
//!   names the HEIC; after the keeper override is cleared the HEIC is superseded, and the run re-derives every
//!   disposition and skips it ("no longer safe"), also after `select` widened the plan back to every file.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ps_export::quarantine::{RunProgress, SkipReason, Stat};
use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::{Disposition, FileId, MoveState, SourceId};
use ps_service::cleanup::{RunOptions, Selection};
use ps_service::export::profile_settings;
use ps_service::library::Library;
use ps_service::scan::{ScanOptions, ScanService};

const HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
}

fn scan(svc: &ScanService) {
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
}

struct Setup {
    _tmp: tempfile::TempDir,
    lib: Library,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
    library: std::path::PathBuf,
    heic: FileId,
    jpg: FileId,
    other: FileId,
    group: FileId,
}

/// Source A's JPEG exported; source B's HEIC original decoded (the group's keeper, superseded) and an unrelated
/// photo of source B exported.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let (w, h) = (3000, 2000);
    let full = ps_media::RgbImage {
        width: w,
        height: h,
        data: ps_fixtures::media::photo_rgb(w, h, 4243),
    };
    let small = ps_media::decode::resize_to_long_edge(&full, 800);
    let jpeg = ps_fixtures::media::encode_jpeg(&small.data, small.width, small.height, 85);
    let other =
        ps_fixtures::media::encode_jpeg(&ps_fixtures::media::photo_rgb(900, 600, 77), 900, 600, 90);
    let a = tmp.path().join("drive-a");
    let b = tmp.path().join("drive-b");
    write(&a, "Web/IMG_0001.jpg", &jpeg);
    let heic = ps_fixtures::heif::heif_with_size(HEIC, w, h).unwrap();
    write(&b, "DCIM/IMG_0001.HEIC", &heic);
    write(&b, "DCIM/IMG_0002.jpg", &other);
    let fake = FakeHeif::new();
    fake.give(&heic, full);
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");
    let svc = ScanService::open(&data, ScanOptions::default())
        .unwrap()
        .with_decoders(Decoders::with_heif(Arc::new(fake)));
    let lib = Library::with_service(&data, svc);
    let svc = lib.scan().unwrap();
    let export = || {
        svc.exports()
            .run(
                &library,
                profile_settings("google").unwrap(),
                &Default::default(),
                &AtomicBool::new(false),
            )
            .unwrap()
            .1
            .copied
    };
    svc.add_source(&a).unwrap();
    scan(svc);
    assert_eq!(export(), 1);
    svc.add_source(&b).unwrap();
    scan(svc);
    // The new photo is copied; the group in the library through the JPEG is not (ADR-0092).
    assert_eq!(export(), 1);
    let (files, _, _) = svc.exports().dispositions(&library, false).unwrap();
    let id = |name: &str| {
        files
            .iter()
            .find(|f| f.rel_path.display().ends_with(name))
            .unwrap()
            .file
    };
    let (heic, jpg, other) = (id("IMG_0001.HEIC"), id("IMG_0001.jpg"), id("IMG_0002.jpg"));
    let groups = svc.engine().catalog().read().group_memberships().unwrap();
    let group = groups
        .iter()
        .find(|g| g.1.contains(&heic))
        .map(|g| g.0)
        .unwrap();
    assert_eq!(
        groups.iter().find(|g| g.0 == group).unwrap().2.first(),
        Some(&heic)
    );
    Setup {
        _tmp: tmp,
        lib,
        a,
        b,
        library,
        heic,
        jpg,
        other,
        group,
    }
}

fn disposition(s: &Setup, file: FileId) -> ps_export::FileDisposition {
    let (files, d, _) = s
        .lib
        .scan()
        .unwrap()
        .exports()
        .dispositions(&s.library, false)
        .unwrap();
    files
        .iter()
        .zip(d)
        .find(|(f, _)| f.file == file)
        .map(|(_, x)| x)
        .unwrap()
}

#[test]
fn scenario_cleanup_cleanup_keeps_a_copy_better_than_the_keeper_you_chose() {
    let s = setup();
    let svc = s.lib.scan().unwrap();
    assert_eq!(disposition(&s, s.heic).disposition, Disposition::Superseded);
    svc.groups().set_keeper(s.group, s.jpg).unwrap();
    // The user's choice is respected for accounting: the HEIC is covered by the JPEG they chose ...
    let hd = disposition(&s, s.heic);
    assert_eq!(hd.disposition, Disposition::Covered, "{hd:?}");
    assert!(hd.better_than_keeper, "{hd:?}");
    assert_eq!(disposition(&s, s.jpg).disposition, Disposition::Exported);
    let c = svc.exports().completion(Some(&s.library), false).unwrap();
    assert!(
        c.report.sources.iter().all(|x| x.counts.fully_accounted()),
        "{:?}",
        c.report.sources
    );
    // ... but it ranks above that keeper automatically, so cleanup keeps it.
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    assert_eq!(
        plan.moves.iter().map(|m| m.file).collect::<Vec<_>>(),
        vec![s.other],
        "only the unrelated exported photo is planned"
    );
    assert_eq!(plan.held.len(), 1, "{:?}", plan.held);
    assert_eq!(plan.held[0].file, s.heic);
    assert_eq!(plan.held[0].reason, SkipReason::BetterThanKeeper);
    assert!(
        plan.held[0]
            .text
            .contains("better than the keeper you chose"),
        "{}",
        plan.held[0].text
    );
    // A folder plan of the HEIC's folder plans the photo beside it, never the HEIC.
    let dcim = ps_model::RelPath::from_bytes(b"DCIM".to_vec()).unwrap();
    svc.cleanups()
        .discard(plan.summary.unwrap().run.id)
        .unwrap();
    let plan = svc.cleanups().plan(SourceId(2), Some(dcim), None).unwrap();
    assert!(plan.moves.iter().all(|m| m.file != s.heic));
    svc.cleanups()
        .discard(plan.summary.unwrap().run.id)
        .unwrap();
    // Source A's JPEG (the keeper, in the library itself) may be cleaned up as before.
    let plan_a = svc.cleanups().plan(SourceId(1), None, None).unwrap();
    assert_eq!(
        plan_a.moves.iter().map(|m| m.file).collect::<Vec<_>>(),
        vec![s.jpg]
    );
    assert!(plan_a.held.is_empty());
    assert!(s.b.join("DCIM/IMG_0001.HEIC").exists());
}

#[test]
fn scenario_photo_inspector_a_copy_better_than_the_keeper_you_chose_is_explained() {
    let s = setup();
    let svc = s.lib.scan().unwrap();
    svc.groups().set_keeper(s.group, s.jpg).unwrap();
    s.lib.forget_dispositions();
    let d = s.lib.disposition_of(s.heic).unwrap().unwrap();
    assert_eq!(d.disposition, Disposition::Covered);
    assert!(d.better_than_keeper);
    assert!(!d.reason.contains("best copy"), "{}", d.reason);
    assert!(
        d.reason.contains("you chose") && d.reason.contains("IMG_0001.jpg"),
        "{}",
        d.reason
    );
    assert!(
        d.reason.contains("better quality and is kept"),
        "{}",
        d.reason
    );
    // Other covered files keep their words.
    let plain = ps_service::library::disposition_reason(Disposition::Covered, None, None, None);
    assert_eq!(plain, "a better copy is in the library");
}

/// A plan stored as an older version did, naming the HEIC as covered by the JPEG's library copy.
fn stale_plan(s: &Setup) -> i64 {
    let svc = s.lib.scan().unwrap();
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    let rec = plan.summary.as_ref().unwrap().run.clone();
    svc.cleanups().discard(rec.id).unwrap();
    let (files, d, _) = svc.exports().dispositions(&s.library, false).unwrap();
    let mut stats = std::collections::HashMap::new();
    svc.engine()
        .catalog()
        .read()
        .for_each_file(|f| {
            stats.insert(f.id, f.stat);
        })
        .unwrap();
    let rows: Vec<(ps_export::FileFacts, ps_export::FileDisposition, Stat)> = files
        .into_iter()
        .zip(d)
        .filter(|(f, _)| f.file == s.heic || f.file == s.other)
        .map(|(f, mut x)| {
            // What the rule of ADR-0092 alone said: covered, eligible.
            x.better_than_keeper = false;
            let st = stats[&f.file];
            (
                f,
                x,
                Stat {
                    size: st.size,
                    mtime_ns: st.mtime_ns,
                    file_index: st.file_index,
                },
            )
        })
        .collect();
    assert_eq!(rows.len(), 2);
    svc.engine()
        .catalog()
        .writer()
        .cleanup_create_run(ps_catalog::NewCleanupRun {
            source: rec.source,
            source_added_at_ms: rec.source_added_at_ms.unwrap(),
            root: rec.root.clone(),
            library_id: rec.library_id,
            folder: None,
            uid: "stale-plan".into(),
            moves: Box::new(move |id| {
                ps_export::quarantine::plan_moves(id, None, rows.iter().map(|(f, d, s)| (f, d, *s)))
            }),
        })
        .unwrap()
}

#[test]
fn scenario_cleanup_cleanup_run_skips_a_file_that_became_unsafe_after_planning() {
    let s = setup();
    let svc = s.lib.scan().unwrap();
    svc.groups().set_keeper(s.group, s.jpg).unwrap();
    let run = stale_plan(&s);
    let (_, moves) = svc.cleanups().run_detail(run).unwrap();
    assert_eq!(moves.len(), 2, "{moves:?}");
    assert!(
        moves
            .iter()
            .any(|m| m.file == s.heic && m.state == MoveState::Planned)
    );
    // `select` narrows the old plan to the other photo, then widens it back to every file.
    svc.cleanups()
        .select(
            run,
            &Selection {
                files: vec![s.other],
                ..Selection::default()
            },
        )
        .unwrap();
    let summary = svc.cleanups().select(run, &Selection::all()).unwrap();
    assert_eq!(summary.counts.planned, 2);
    // The keeper override is cleared: the HEIC is the keeper again, better than the library's JPEG, and
    // superseded (not accounted for).
    svc.groups().clear_keeper(s.group).unwrap();
    assert_eq!(disposition(&s, s.heic).disposition, Disposition::Superseded);
    let heic_path = s.b.join("DCIM/IMG_0001.HEIC");
    let before = std::fs::read(&heic_path).unwrap();
    let report = svc
        .cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    assert_eq!(report.moved, 1, "{report:?}");
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    let sk = &report.skipped[0];
    assert_eq!(sk.file, s.heic);
    assert!(
        matches!(sk.reason, SkipReason::NoLongerSafe(_)),
        "{:?}",
        sk.reason
    );
    assert!(
        sk.text.starts_with("no longer safe") && sk.text.contains("not in the library"),
        "{}",
        sk.text
    );
    let json = serde_json::to_value(sk).unwrap();
    assert_eq!(json["reason"]["kind"], "no_longer_safe", "{json}");
    // The HEIC is untouched at its path; the run's catalog row says why.
    assert_eq!(std::fs::read(&heic_path).unwrap(), before);
    let (_, moves) = svc.cleanups().run_detail(run).unwrap();
    let m = moves.iter().find(|m| m.file == s.heic).unwrap();
    assert_eq!(m.state, MoveState::Skipped);
    assert!(
        m.note
            .as_deref()
            .unwrap_or("")
            .starts_with("no longer safe"),
        "{m:?}"
    );
    assert!(
        !s.b.join("DCIM/IMG_0002.jpg").exists(),
        "the safe photo moved"
    );
    assert!(s.a.join("Web/IMG_0001.jpg").exists());
}
