//! A better copy that is not in the library (ADR-0093), end to end through the service: a resized JPEG is
//! exported from one source, its HEIC original on a second source is decoded (fake decoder) and becomes the
//! group's keeper. The HEIC is superseded: not accounted for, never planned for cleanup, and the inspector
//! does not call the JPEG its best copy. After `--upgrade` it is exported and cleanup may consider the lesser
//! copies as usual. This is the gate reviewer's blocker repro on fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::{Disposition, SourceId};
use ps_service::export::profile_settings;
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

#[test]
fn scenario_cleanup_cleanup_never_moves_a_better_copy_that_is_not_in_the_library() {
    let tmp = tempfile::tempdir().unwrap();
    let (w, h) = (3000, 2000);
    let rgb = ps_fixtures::media::photo_rgb(w, h, 4243);
    let full = ps_media::RgbImage {
        width: w,
        height: h,
        data: rgb,
    };
    let small = ps_media::decode::resize_to_long_edge(&full, 800);
    assert_eq!((small.width, small.height), (800, 533));
    let jpeg = ps_fixtures::media::encode_jpeg(&small.data, small.width, small.height, 85);
    let a = tmp.path().join("drive-a");
    let b = tmp.path().join("drive-b");
    write(&a, "Web/IMG_0001.jpg", &jpeg);
    // The sample declaring the original's 3000x2000 (the fake decoder returns its pixels).
    let heic = ps_fixtures::heif::heif_with_size(HEIC, w, h).unwrap();
    write(&b, "DCIM/IMG_0001.HEIC", &heic);
    let fake = FakeHeif::new();
    fake.give(&heic, full);
    let decoders = Decoders::with_heif(Arc::new(fake));
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");
    let settings = profile_settings("google").unwrap();
    let svc = ScanService::open(&data, ScanOptions::default())
        .unwrap()
        .with_decoders(decoders.clone());

    // 1. Source A's JPEG is exported.
    svc.add_source(&a).unwrap();
    scan(&svc);
    let run = |upgrade: bool| {
        svc.exports()
            .run(
                &library,
                settings.with_upgrade(upgrade),
                &Default::default(),
                &AtomicBool::new(false),
            )
            .unwrap()
            .1
    };
    assert_eq!(run(false).copied, 1);

    // 2. Source B's HEIC original is decoded and becomes the keeper of the JPEG's group.
    svc.add_source(&b).unwrap();
    scan(&svc);
    let groups = svc.engine().catalog().read().group_memberships().unwrap();
    assert_eq!(groups.len(), 1, "one group: {groups:?}");
    let (files, d, _) = svc.exports().dispositions(&library, false).unwrap();
    let of = |src: i64| {
        files
            .iter()
            .zip(&d)
            .find(|(f, _)| f.source == SourceId(src))
            .map(|(f, x)| (f.file, x.clone()))
            .unwrap()
    };
    let (heic, hd) = of(2);
    let (jpg, jd) = of(1);
    assert_eq!(groups[0].2.first(), Some(&heic), "the HEIC is the keeper");
    assert_eq!(hd.disposition, Disposition::Superseded, "{hd:?}");
    assert_eq!(jd.disposition, Disposition::Exported, "{jd:?}");

    // 3. The report does not call source B accounted for.
    let c = svc.exports().completion(Some(&library), false).unwrap();
    let sb = c
        .report
        .sources
        .iter()
        .find(|s| s.source == SourceId(2))
        .unwrap();
    assert!(!sb.counts.fully_accounted(), "{:?}", sb.counts);
    assert_eq!(sb.counts.files_of(Disposition::Superseded), 1);
    assert_eq!(sb.counts.files_of(Disposition::Covered), 0);
    assert_eq!(sb.badge.label(), "1 better copy not in the library yet");

    // 4. A cleanup plan of source B moves nothing; source A's JPEG (in the library itself) may be moved.
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    assert!(plan.moves.is_empty(), "{:?}", plan.moves);
    assert!(plan.summary.is_none(), "nothing eligible, no run stored");
    let plan_a = svc.cleanups().plan(SourceId(1), None, None).unwrap();
    assert_eq!(
        plan_a.moves.iter().map(|m| m.file).collect::<Vec<_>>(),
        vec![jpg]
    );
    svc.cleanups()
        .discard(plan_a.summary.unwrap().run.id)
        .unwrap();

    // 5. `--upgrade` copies the HEIC; it is exported, the JPEG covered, and cleanup may consider both.
    assert_eq!(run(true).copied, 1);
    let (files, d, _) = svc.exports().dispositions(&library, false).unwrap();
    let by: std::collections::HashMap<_, _> = files.iter().map(|f| f.file).zip(&d).collect();
    assert_eq!(by[&heic].disposition, Disposition::Exported);
    assert_eq!(by[&jpg].disposition, Disposition::Covered);
    let c = svc.exports().completion(Some(&library), false).unwrap();
    assert!(c.report.sources.iter().all(|s| s.counts.fully_accounted()));
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    assert_eq!(
        plan.moves.iter().map(|m| m.file).collect::<Vec<_>>(),
        vec![heic]
    );
    svc.cleanups().discard(plan.summary.unwrap().run.id).unwrap();
    let plan = svc.cleanups().plan(SourceId(1), None, None).unwrap();
    assert_eq!(
        plan.moves.iter().map(|m| m.file).collect::<Vec<_>>(),
        vec![jpg]
    );
    assert_eq!(plan.moves[0].disposition, Disposition::Covered);
    // Nothing was moved by planning.
    assert!(a.join("Web/IMG_0001.jpg").exists());
    assert!(b.join("DCIM/IMG_0001.HEIC").exists());
}

/// The inspector's words for a superseded HEIC: never "its best copy" for the lesser JPEG.
#[test]
fn scenario_photo_inspector_better_copy_not_in_the_library_is_explained() {
    let why = ps_service::library::disposition_reason(
        Disposition::Superseded,
        None,
        None,
        None,
    );
    assert!(!why.contains("best copy"), "{why}");
    assert!(why.contains("not in the library yet") && why.contains("--upgrade"), "{why}");
    let p = ps_model::RelPath::from_bytes(b"2019/IMG_0001.jpg".to_vec()).unwrap();
    let why = ps_service::library::disposition_reason(Disposition::Superseded, None, Some(&p), None);
    assert!(!why.contains("best copy"), "{why}");
    assert!(why.contains("2019/IMG_0001.jpg") && why.contains("lesser copy"), "{why}");
}
