//! Duplicate review through the service facade (duplicate-review tasks 1.1 and 1.2): the group list with its
//! sorts and filters, review progress, review state that follows membership, and suggestion dismissal.

use std::path::{Path, PathBuf};

use ps_fixtures::{DirWriter, Params, Preset};
use ps_service::review::{self, ReviewFilter, ReviewSort, ReviewStore};
use ps_service::scan::{ScanOptions, ScanService};

fn tiny() -> Params {
    Params {
        standalone: 10,
        sets_per_kind: 2,
        burst_sets: 2,
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

fn scanned(dir: &Path, root: &Path) -> ScanService {
    let svc = ScanService::open(&dir.join("data"), ScanOptions::default()).expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    svc
}

#[test]
fn groups_list_filters_and_sorts() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let store = ReviewStore::in_memory();
    let all = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let totals = svc.groups().totals().expect("totals");
    assert_eq!(all.groups, totals.groups);
    assert_eq!(all.rows.len() as u64, totals.groups);
    assert_eq!(all.reclaimable_bytes, totals.reclaimable_bytes);
    assert_eq!(all.reviewed, 0);
    assert!(
        all.rows
            .windows(2)
            .all(|w| w[0].reclaimable_bytes >= w[1].reclaimable_bytes)
    );
    // Every row is listed once, with its keeper's name and thumbnail key.
    let mut ids: Vec<_> = all.rows.iter().map(|r| r.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), all.rows.len());
    assert!(
        all.rows
            .iter()
            .all(|r| !r.keeper_name.is_empty() && r.thumb.is_some())
    );

    let by_count = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::MemberCount,
    )
    .expect("list");
    assert!(
        by_count
            .rows
            .windows(2)
            .all(|w| w[0].member_count >= w[1].member_count)
    );
    let by_date = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::CaptureDate,
    )
    .expect("list");
    let dated: Vec<i64> = by_date.rows.iter().filter_map(|r| r.capture_ms).collect();
    assert!(dated.windows(2).all(|w| w[0] >= w[1]));
    let attention = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::NeedsAttention,
    )
    .expect("list");
    assert!(
        attention
            .rows
            .windows(2)
            .all(|w| w[0].attention_score >= w[1].attention_score)
    );

    let exact = review::list(
        &svc,
        &store,
        ReviewFilter {
            exact_only: true,
            ..ReviewFilter::default()
        },
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let derived = review::list(
        &svc,
        &store,
        ReviewFilter {
            has_derivatives: true,
            ..ReviewFilter::default()
        },
        ReviewSort::Reclaimable,
    )
    .expect("list");
    assert!(
        exact
            .rows
            .iter()
            .all(|r| r.exact_only && !r.has_derivatives)
    );
    assert!(derived.rows.iter().all(|r| r.has_derivatives));
    assert!(!derived.rows.is_empty());
    assert_eq!(exact.groups, all.groups, "progress counts every group");

    // Review two groups: the unreviewed filter hides them and progress counts them.
    for r in all.rows.iter().take(2) {
        assert!(review::set_reviewed(&svc, &store, r.id, true).expect("review"));
    }
    let open = review::list(
        &svc,
        &store,
        ReviewFilter {
            unreviewed: true,
            ..ReviewFilter::default()
        },
        ReviewSort::Reclaimable,
    )
    .expect("list");
    assert_eq!(open.reviewed, 2);
    assert_eq!(open.rows.len(), all.rows.len() - 2);
    assert!(open.rows.iter().all(|r| !r.reviewed));
    // The catalog's flag follows (CLI).
    assert!(svc.groups().show(all.rows[0].id).expect("show").reviewed);

    // An override shows up in the overridden filter.
    let d = review::show(&svc, &store, all.rows[2].id).expect("show");
    let d =
        all.rows
            .iter()
            .map(|r| review::show(&svc, &store, r.id).expect("show"))
            .find(|d| {
                d.group.members.iter().any(|m| {
                    m.edge == ps_service::groups::MemberEdge::Derivative && !m.in_keeper_set
                })
            })
            .unwrap_or(d);
    let other = d
        .group
        .members
        .iter()
        .find(|m| m.edge == ps_service::groups::MemberEdge::Derivative && !m.in_keeper_set)
        .expect("derivative member")
        .file;
    svc.groups()
        .set_keeper(d.group.id, other)
        .expect("set keeper");
    let overridden = review::list(
        &svc,
        &store,
        ReviewFilter {
            overridden: true,
            ..ReviewFilter::default()
        },
        ReviewSort::Reclaimable,
    )
    .expect("list");
    assert!(overridden.rows.iter().any(|r| r.keeper == other));
    assert!(overridden.rows.iter().all(|r| r.overridden));
}

#[test]
fn review_state_persists_and_member_change_unreviews() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let data = tmp.path().join("data");
    let store = ReviewStore::open(&data);
    let all = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let (a, b) = (all.rows[0].clone(), all.rows[1].clone());
    review::set_reviewed(&svc, &store, a.id, true).expect("review a");
    review::set_reviewed(&svc, &store, b.id, true).expect("review b");

    // Restart: the review state is still there.
    drop(store);
    let store = ReviewStore::open(&data);
    assert!(review::show(&svc, &store, a.id).expect("show").reviewed);

    // Add a member to group `a`: another exact copy of its keeper, then rescan.
    let d = review::show(&svc, &store, a.id).expect("show");
    let keeper = d.group.member(d.group.keeper).expect("keeper").path.clone();
    let copy = keeper.with_file_name(format!(
        "copy-for-review-{}",
        keeper.file_name().expect("name").to_string_lossy()
    ));
    std::fs::copy(&keeper, &copy).expect("copy");
    svc.start(&[]).expect("rescan");
    svc.wait().expect("summary");

    let after = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let a_now = after
        .rows
        .iter()
        .find(|r| r.keeper == a.keeper || r.id == a.id)
        .expect("group a still listed");
    assert_eq!(a_now.member_count, a.member_count + 1);
    assert!(
        !a_now.reviewed,
        "a group that gained a member is unreviewed"
    );
    let b_now = after.rows.iter().find(|r| r.id == b.id).expect("group b");
    assert!(b_now.reviewed, "an unchanged group stays reviewed");
    assert_eq!(after.reviewed, 1);
}

#[test]
fn dismissed_suggestions_stay_dismissed() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let data = tmp.path().join("data");
    let store = ReviewStore::open(&data);
    let before = review::suggestions(&svc, &store, 1_000).expect("suggestions");
    assert!(!before.is_empty(), "the corpus has burst suggestions");
    let s = &before[0];
    let groups_before = svc.groups().named().expect("named");
    review::dismiss(&svc, &store, s.info.file_a, s.info.file_b).expect("dismiss");
    drop(store);
    let store = ReviewStore::open(&data);
    let after = review::suggestions(&svc, &store, 1_000).expect("suggestions");
    // The pair is dismissed by content: the same two shots on other drives are dismissed with it.
    let content = |f| {
        svc.engine()
            .catalog()
            .read()
            .file_by_id(f)
            .expect("read")
            .and_then(|r| r.identity)
            .expect("content")
    };
    let key = |a, b| {
        let (x, y) = (content(a), content(b));
        if x <= y { (x, y) } else { (y, x) }
    };
    let dismissed = key(s.info.file_a, s.info.file_b);
    let same = before
        .iter()
        .filter(|x| key(x.info.file_a, x.info.file_b) == dismissed)
        .count();
    assert!(same >= 1);
    assert_eq!(after.len(), before.len() - same);
    assert!(
        !after
            .iter()
            .any(|x| (x.info.file_a, x.info.file_b) == (s.info.file_a, s.info.file_b))
    );
    assert_eq!(svc.groups().named().expect("named"), groups_before);
    let list = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    assert_eq!(list.suggestions as usize, after.len());
}

#[test]
fn exact_copy_chains_are_labelled_by_their_origin() {
    use ps_service::groups::MemberEdge;
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let store = ReviewStore::in_memory();
    let all = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let mut exact = 0;
    for row in &all.rows {
        let d = review::show(&svc, &store, row.id).expect("show");
        for m in d.members().iter().filter(|m| m.edge == MemberEdge::Exact) {
            exact += 1;
            let origin = d.matched_against(m.file).expect("matched");
            let o = d.group.member(origin).expect("origin is a member");
            assert_ne!(o.edge, MemberEdge::Exact, "chains collapse to their origin");
            let label = d.matched_label(m.file);
            if o.keeper || o.in_keeper_set {
                assert_eq!(label, " of the keeper");
            } else {
                assert!(label.starts_with(" of #"), "{label}");
            }
        }
    }
    assert!(exact > 0, "the corpus has exact copies");
}
