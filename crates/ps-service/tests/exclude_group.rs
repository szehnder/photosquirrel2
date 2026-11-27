//! Excluding a whole duplicate group from the review ("keep none", exclude-group-in-review tasks 1.1-1.2): every
//! copy is excluded by the user, the group stays out of the export and stays excluded when a rescan changes its
//! keeper; Include again and Undo put it back. Nothing on disk changes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use ps_export::ExecConfig;
use ps_fixtures::jpeg::encode_jpeg;
use ps_fixtures::synth::Rgb;
use ps_fixtures::{DirWriter, Params, Preset};
use ps_model::{Disposition, ExclusionReason, FileId};
use ps_service::export::profile_settings;
use ps_service::review::{self, ReviewFilter, ReviewSort, ReviewStore};
use ps_service::scan::{ScanOptions, ScanService};

fn scratch() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(base).unwrap();
    tempfile::Builder::new()
        .prefix("ps-exclude-group-")
        .tempdir_in(base)
        .unwrap()
}

fn tiny() -> Params {
    Params {
        standalone: 10,
        sets_per_kind: 2,
        burst_sets: 2,
        converted_raw_bursts: 0,
        videos: 1,
        scans: 1,
        icons: 2,
        ui_graphics: 2,
        tiny_web: 2,
        junk_videos: 1,
        raw_share: 0.25,
        dim_scale: (1, 2),
        twelve_mp: 0,
        backups: (1, 2),
    }
}

fn corpus(dir: &Path) -> PathBuf {
    let root = dir.join("corpus");
    let w = DirWriter::new(&root).expect("writer");
    ps_fixtures::generate_with(11, Preset::Small, tiny(), &w).expect("generate");
    root
}

fn scanned(dir: &Path, sources: &[PathBuf]) -> ScanService {
    let svc = ScanService::open(&dir.join("data"), ScanOptions::default()).expect("open");
    for s in sources {
        svc.add_source(s).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    svc
}

fn rescan(svc: &ScanService) {
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
}

/// Size and modification time of every file under `root`.
fn stamps(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = fs::metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p);
            } else {
                out.insert(p, (m.len(), m.modified().unwrap()));
            }
        }
    }
    out
}

/// The largest group of the list (most copies).
fn biggest_group(svc: &ScanService, store: &ReviewStore) -> review::ReviewRow {
    review::list(svc, store, ReviewFilter::default(), ReviewSort::MemberCount)
        .expect("list")
        .rows
        .into_iter()
        .next()
        .expect("a group")
}

fn excluded_by_user(svc: &ScanService, files: &[FileId]) -> Vec<FileId> {
    let d = svc
        .engine()
        .catalog()
        .read()
        .user_decisions_of(files)
        .expect("decisions");
    files
        .iter()
        .copied()
        .filter(|f| {
            d.get(f)
                .is_some_and(|x| x.decision == ps_catalog::UserDecision::Exclude)
        })
        .collect()
}

#[test]
fn scenario_duplicate_review_excluded_group_is_not_exported() {
    let tmp = scratch();
    let root = corpus(tmp.path());
    let drives: Vec<PathBuf> = ["drive-a", "drive-b", "drive-c"]
        .iter()
        .map(|d| root.join(d))
        .collect();
    let svc = scanned(tmp.path(), &drives);
    let store = ReviewStore::in_memory();
    let row = biggest_group(&svc, &store);
    assert!(row.member_count >= 2, "a group with copies");
    let before = stamps(&root);

    let done =
        review::exclude_group(&svc, &store, row.id, Some("not ours".into())).expect("exclude");
    assert!(!done.was_reviewed);
    let d = review::show(&svc, &store, row.id).expect("show");
    let members: Vec<FileId> = d.members().iter().map(|m| m.file).collect();
    assert!(d.reviewed, "excluding marks the group reviewed");
    assert!(d.keeper_excluded());
    assert_eq!(d.excluded, members);
    assert_eq!(d.kept(), 0);
    let listed = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::MemberCount,
    )
    .expect("list")
    .rows
    .into_iter()
    .find(|r| r.id == row.id)
    .expect("still listed");
    assert!(listed.keeper_excluded && listed.reviewed);
    assert_eq!(listed.excluded, row.member_count);

    let library = tmp.path().join("library");
    let (_, report) = svc
        .exports()
        .run(
            &library,
            profile_settings("google").unwrap(),
            &ExecConfig::default(),
            &AtomicBool::new(false),
        )
        .expect("export");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let (_, dispositions, _) = svc.exports().dispositions(&library, false).expect("report");
    for f in &members {
        let x = dispositions.iter().find(|x| x.file == *f).expect("file");
        assert_eq!(x.disposition, Disposition::Excluded, "{f:?}");
        assert_eq!(x.reason, Some(ExclusionReason::UserExcluded));
        assert!(x.library_path.is_none(), "no copy in the library");
    }
    assert_eq!(stamps(&root), before, "no source file changed");
}

/// One photo: a 1600x1200 original and an 800x600 resized copy of it, as JPEG.
fn photo(seed: u64) -> (Vec<u8>, Vec<u8>) {
    let big = Rgb {
        width: 1600,
        height: 1200,
        data: ps_fixtures::media::photo_rgb(1600, 1200, seed),
    };
    let small = big.downscale(800, 600);
    (encode_jpeg(&big, 92), encode_jpeg(&small, 85))
}

#[test]
fn scenario_duplicate_review_excluded_group_stays_out_after_its_keeper_changes() {
    let tmp = scratch();
    let (big, small) = photo(7);
    let a = tmp.path().join("drive-a");
    fs::create_dir_all(&a).unwrap();
    fs::write(a.join("small.jpg"), &small).unwrap();
    fs::write(a.join("small copy.jpg"), &small).unwrap();
    let svc = scanned(tmp.path(), std::slice::from_ref(&a));
    let store = ReviewStore::in_memory();
    let row = biggest_group(&svc, &store);
    assert_eq!(row.member_count, 2);
    review::exclude_group(&svc, &store, row.id, None).expect("exclude");
    let old: Vec<FileId> = review::show(&svc, &store, row.id)
        .expect("show")
        .members()
        .iter()
        .map(|m| m.file)
        .collect();

    // The original arrives on another drive and becomes the keeper.
    let b = tmp.path().join("drive-b");
    fs::create_dir_all(&b).unwrap();
    fs::write(b.join("original.jpg"), &big).unwrap();
    svc.add_source(&b).expect("add");
    rescan(&svc);
    let row = biggest_group(&svc, &store);
    assert_eq!(row.member_count, 3, "the original joined the group");
    let d = review::show(&svc, &store, row.id).expect("show");
    assert!(
        !old.contains(&d.group.keeper),
        "the original is the keeper now"
    );
    assert_eq!(
        excluded_by_user(&svc, &old),
        old,
        "every old copy stays excluded"
    );
    assert!(!d.keeper_excluded());
    assert_eq!(d.kept(), 1);
}

#[test]
fn include_again_and_undo_put_the_group_back() {
    let tmp = scratch();
    let (_, small) = photo(9);
    let a = tmp.path().join("drive-a");
    fs::create_dir_all(&a).unwrap();
    fs::write(a.join("one.jpg"), &small).unwrap();
    fs::write(a.join("two.jpg"), &small).unwrap();
    fs::write(a.join("three.jpg"), &small).unwrap();
    let svc = scanned(tmp.path(), std::slice::from_ref(&a));
    let store = ReviewStore::in_memory();
    let row = biggest_group(&svc, &store);
    let members: Vec<FileId> = review::show(&svc, &store, row.id)
        .unwrap()
        .members()
        .iter()
        .map(|m| m.file)
        .collect();
    let keeper = row.keeper;

    // Undo restores every decision and the review state.
    let x = review::exclude_group(&svc, &store, row.id, None).expect("exclude");
    let d = review::include_group(&svc, &store, row.id, Some(&x)).expect("undo");
    assert!(d.excluded.is_empty());
    assert!(!d.reviewed, "undo makes it unreviewed again");
    assert_eq!(d.group.keeper, keeper);

    // Include again (a later session: no undo record) clears the user's exclusions and keeps the review mark.
    review::exclude_group(&svc, &store, row.id, Some("blurry".into())).expect("exclude");
    let d = review::include_group(&svc, &store, row.id, None).expect("include");
    assert!(d.excluded.is_empty());
    assert!(d.reviewed);
    assert_eq!(d.group.keeper, keeper);
    assert!(excluded_by_user(&svc, &members).is_empty());
}

#[test]
fn include_again_keeps_an_exclusion_made_for_another_reason() {
    let tmp = scratch();
    let (_, small) = photo(11);
    let a = tmp.path().join("drive-a");
    fs::create_dir_all(&a).unwrap();
    fs::write(a.join("one.jpg"), &small).unwrap();
    fs::write(a.join("two.jpg"), &small).unwrap();
    let svc = scanned(tmp.path(), std::slice::from_ref(&a));
    let store = ReviewStore::in_memory();
    let row = biggest_group(&svc, &store);
    let members: Vec<FileId> = review::show(&svc, &store, row.id)
        .unwrap()
        .members()
        .iter()
        .map(|m| m.file)
        .collect();
    // One copy was excluded by accepting an AI suggestion ("screenshot") before the group was excluded.
    let ai_copy = members[1];
    svc.engine()
        .catalog()
        .writer()
        .set_user_decisions(vec![ps_catalog::DecisionWrite {
            file: ai_copy,
            decision: Some(ps_catalog::UserDecision::Exclude),
            reason: Some("screenshot".into()),
            ai_kind: Some(ps_model::AiKind::Screenshot),
        }])
        .unwrap();

    let x = review::exclude_group(&svc, &store, row.id, None).expect("exclude");
    assert_eq!(
        x.prior.len(),
        1,
        "only the copy that was not excluded is changed"
    );
    let d = review::include_group(&svc, &store, row.id, None).expect("include");
    assert_eq!(d.excluded, vec![ai_copy], "the AI exclusion stays");
    let rec = svc
        .engine()
        .catalog()
        .read()
        .user_decision(ai_copy)
        .unwrap()
        .expect("decision");
    assert_eq!(rec.ai_kind, Some(ps_model::AiKind::Screenshot));
}
