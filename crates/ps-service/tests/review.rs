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

/// review-progress-anchor 3.1: the navigator's progress counts what the review list counts.
#[test]
fn progress_matches_the_review_list() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let store = ReviewStore::in_memory();
    let list = |store: &ReviewStore| {
        review::list(
            &svc,
            store,
            ReviewFilter::default(),
            ReviewSort::Reclaimable,
        )
        .expect("list")
    };
    let before = review::progress(&svc, &store).expect("progress");
    let all = list(&store);
    assert!(all.groups >= 3, "the corpus has groups");
    assert_eq!((before.groups, before.reviewed), (all.groups, 0));
    let cache = review::ReviewCache::new();
    assert_eq!(cache.progress(&svc, &store).expect("cached"), before);
    for r in all.rows.iter().take(3) {
        review::set_reviewed(&svc, &store, r.id, true).expect("review");
    }
    assert_eq!(
        cache.progress(&svc, &store).expect("cached"),
        review::progress(&svc, &store).expect("progress"),
        "the cached groups see the review state"
    );
    let after = review::progress(&svc, &store).expect("progress");
    let all = list(&store);
    assert_eq!((after.groups, after.reviewed), (all.groups, all.reviewed));
    assert_eq!(after.reviewed, 3);
    assert_eq!(after.left(), all.groups - 3);
}

/// review-progress-anchor 3.4: the navigator's progress pass on a 1M-file catalog with 100k groups of 3 (half of
/// them reviewed), against the review list it replaces for the count. Builds its own synthetic catalog (the
/// shared bench catalog has no groups, and its budgets must not change):
/// `PHOTOSQUIRREL_SYNTHETIC_DIR=target/synthetic cargo test --release -p ps-service --test review -- --ignored
/// --nocapture progress_over_1m`.
#[test]
#[ignore = "builds a 1.2 GB catalog with 100k groups on first run"]
fn progress_over_1m() {
    use ps_catalog::groups::{GroupChanges, GroupRow, MemberEdge, MemberRow};
    use ps_model::FileId;
    use ps_service::library::{Library, synthetic};
    use std::time::Instant;

    let base = std::env::var_os("PHOTOSQUIRREL_SYNTHETIC_DIR")
        .map(PathBuf::from)
        .expect("PHOTOSQUIRREL_SYNTHETIC_DIR");
    let dir = synthetic::ensure_at(base.join("review-progress-1m"), 1_000_000).expect("synthetic");
    let lib = Library::open(&dir).expect("open");
    let cat = lib.catalog().expect("catalog");
    let groups = 100_000u64;
    if cat.read().group_totals().expect("totals").0 != groups {
        let rows = (0..groups)
            .map(|g| {
                let ids: Vec<FileId> = (0..3).map(|k| FileId((g * 3 + k + 1) as i64)).collect();
                GroupRow {
                    id: ids[0],
                    keeper: ids[0],
                    display: ids[0],
                    keeper_set: vec![ids[0]],
                    reasons: serde_json::json!([]),
                    reclaimable_bytes: 1_000,
                    digest: [0; 32],
                    members: ids
                        .iter()
                        .enumerate()
                        .map(|(rank, f)| MemberRow {
                            file: *f,
                            rank: rank as u32,
                            edge: if rank == 0 {
                                MemberEdge::Keeper
                            } else {
                                MemberEdge::Exact
                            },
                            via: (rank > 0).then_some(ids[0]),
                            phash_distance: None,
                            dhash_distance: None,
                            in_keeper_set: rank == 0,
                        })
                        .collect(),
                    reviewed_at_ms: None,
                }
            })
            .collect();
        cat.writer()
            .apply_groups(GroupChanges {
                clear: true,
                groups: rows,
                ..GroupChanges::default()
            })
            .expect("groups");
    }
    let svc = lib.scan().expect("scan");
    let store = ReviewStore::in_memory();
    let list = review::list(
        svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    assert_eq!(list.groups, groups);
    for r in list.rows.iter().step_by(2) {
        review::set_reviewed(svc, &store, r.id, true).expect("review");
    }
    let mut progress_ms = Vec::new();
    let mut list_ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let p = review::progress(svc, &store).expect("progress");
        progress_ms.push(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!((p.groups, p.reviewed), (groups, groups / 2));
        let t = Instant::now();
        let _ = review::list(
            svc,
            &store,
            ReviewFilter::default(),
            ReviewSort::Reclaimable,
        )
        .expect("list");
        list_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let cache = review::ReviewCache::new();
    cache.progress(svc, &store).expect("warm");
    let mut key_ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let p = cache.progress(svc, &store).expect("cached");
        key_ms.push(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!((p.groups, p.reviewed), (groups, groups / 2));
    }
    key_ms.sort_by(f64::total_cmp);
    println!(
        "cached progress (ADR-0133): median {:.1} ms, max {:.1} ms",
        key_ms[2], key_ms[4]
    );
    progress_ms.sort_by(f64::total_cmp);
    list_ms.sort_by(f64::total_cmp);
    println!(
        "progress over {groups} groups: median {:.1} ms, max {:.1} ms; review list: median {:.1} ms",
        progress_ms[2], progress_ms[4], list_ms[2]
    );
}

// ---- ai-filtered-duplicates: the AI part of the review filter -----------------------------------------------

use ps_model::library::InsightCriteria;

/// Store an AI result for `file`'s content (what the AI queue writes).
fn describe(svc: &ScanService, file: ps_model::FileId, insight: serde_json::Value) {
    let cat = svc.engine().catalog();
    let content = cat
        .read()
        .file_by_id(file)
        .expect("read")
        .and_then(|r| r.identity)
        .expect("content");
    cat.writer()
        .ai_complete(ps_catalog::AiResultRow {
            content,
            provider: "test".into(),
            model: "test".into(),
            schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
            prompt_version: 1,
            insight: insight.to_string(),
            latency_ms: 1,
            requests: 1,
            at_ms: 1,
        })
        .expect("store");
}

fn insight(category: &str, people: u32, children: u32, pets: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "subject": "test", "category": category, "is_photo": true,
        "people": {"total": people, "adults": people - children, "children": children},
        "pets": pets, "confidence": 0.9
    })
}

fn pets() -> ReviewFilter {
    ReviewFilter {
        insight: Some(InsightCriteria {
            pets: true,
            ..InsightCriteria::default()
        }),
        ..ReviewFilter::default()
    }
}

/// 1.1: any member decides (here a resized copy, not the keeper), undescribed groups, and `matching` ignoring the
/// other chips.
#[test]
fn ai_filter_matches_any_member_and_counts_matching_groups() {
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
    assert!(all.groups >= 3);
    assert_eq!((all.matching, all.matching_reviewed), (all.groups, 0));

    // Nothing described: every group is undescribed, none has pets.
    let undescribed = ReviewFilter {
        undescribed: true,
        ..ReviewFilter::default()
    };
    let l = review::list(&svc, &store, undescribed, ReviewSort::Reclaimable).expect("list");
    assert_eq!((l.rows.len() as u64, l.matching), (all.groups, all.groups));
    let l = review::list(&svc, &store, pets(), ReviewSort::Reclaimable).expect("list");
    assert_eq!((l.rows.len(), l.matching), (0, 0));

    // Describe a non-keeper of the first group with a dog, the keeper of the second with people only.
    let g1 = review::show(&svc, &store, all.rows[0].id).expect("show");
    let copy = g1
        .group
        .members
        .iter()
        .find(|m| !m.keeper)
        .expect("a copy")
        .file;
    describe(&svc, copy, insight("pets", 0, 0, &["dog"]));
    let g2 = review::show(&svc, &store, all.rows[1].id).expect("show");
    describe(&svc, g2.group.keeper, insight("people", 2, 1, &[]));

    let l = review::list(&svc, &store, pets(), ReviewSort::Reclaimable).expect("list");
    assert_eq!(
        l.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![all.rows[0].id],
        "the group matches on its copy's insight"
    );
    assert_eq!((l.groups, l.matching), (all.groups, 1));

    let l = review::list(&svc, &store, undescribed, ReviewSort::Reclaimable).expect("list");
    assert_eq!(l.matching, all.groups - 2);
    assert!(
        !l.rows
            .iter()
            .any(|r| r.id == all.rows[0].id || r.id == all.rows[1].id)
    );

    // `matching` counts the AI part alone: reviewing the pets group and adding "unreviewed" hides the row but
    // keeps "1 of 1 matching reviewed".
    review::set_reviewed(&svc, &store, all.rows[0].id, true).expect("review");
    let both = ReviewFilter {
        unreviewed: true,
        ..pets()
    };
    let l = review::list(&svc, &store, both, ReviewSort::Reclaimable).expect("list");
    assert!(l.rows.is_empty());
    assert_eq!((l.matching, l.matching_reviewed), (1, 1));
}

/// A matcher that must not be consulted.
struct Panics;

impl review::GroupMatcher for Panics {
    fn matches(&self, _: &[review::GroupFile]) -> bool {
        panic!("consulted without an AI filter")
    }
    fn described(&self, _: &[review::GroupFile]) -> bool {
        panic!("consulted without an AI filter")
    }
}

#[test]
fn a_list_without_an_ai_filter_consults_no_matcher() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let store = ReviewStore::in_memory();
    let l = review::list_with(
        &svc,
        &store,
        ReviewFilter {
            unreviewed: true,
            ..ReviewFilter::default()
        },
        ReviewSort::Reclaimable,
        Some(&Panics),
    )
    .expect("list");
    assert_eq!(l.rows.len() as u64, l.groups);
}

/// The app's index matcher and the CLI's lookup matcher list the same groups, corrections included.
#[test]
fn index_and_lookup_matchers_agree() {
    use ps_service::insights::{self, Correction};
    use ps_service::library::Library;
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    drop(scanned(tmp.path(), &root));
    let lib = Library::open(&tmp.path().join("data")).expect("open");
    let cat = lib.catalog().expect("catalog");
    assert!(insights::seed_demo_from(cat, 10_000, Some(&root)).expect("seed") > 0);
    // A correction must win in both.
    let svc = lib.scan().expect("scan");
    let store = ReviewStore::in_memory();
    let all = review::list(
        svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let keeper = all.rows[0].keeper;
    let content = cat
        .read()
        .file_by_id(keeper)
        .expect("read")
        .and_then(|r| r.identity)
        .expect("content");
    if insights::effective(cat, lib.insight_store().expect("store"), &content)
        .expect("effective")
        .is_some()
    {
        insights::correct(
            cat,
            lib.insight_store().expect("store"),
            &content.to_hex(),
            Correction {
                category: Some(ps_model::SceneCategory::Pets),
                ..Correction::default()
            },
        )
        .expect("correct");
    }
    for criteria in [
        InsightCriteria {
            people: true,
            ..InsightCriteria::default()
        },
        InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        },
        InsightCriteria {
            pets: true,
            ..InsightCriteria::default()
        },
        InsightCriteria {
            category: Some(ps_model::SceneCategory::Pets),
            ..InsightCriteria::default()
        },
    ] {
        for undescribed in [false, true] {
            let filter = ReviewFilter {
                insight: (!undescribed).then_some(criteria),
                undescribed,
                ..ReviewFilter::default()
            };
            let index = review::IndexMatcher {
                matching: lib.insight_matches(&criteria).expect("matches"),
                described: lib
                    .insight_matches(&InsightCriteria::default())
                    .expect("described"),
            };
            let a = review::list_with(svc, &store, filter, ReviewSort::Reclaimable, Some(&index))
                .expect("index");
            let b = review::list(svc, &store, filter, ReviewSort::Reclaimable).expect("lookup");
            let ids = |l: &review::ReviewList| l.rows.iter().map(|r| r.id).collect::<Vec<_>>();
            assert_eq!(ids(&a), ids(&b), "{filter:?}");
            assert_eq!(a.matching, b.matching);
        }
    }
}

/// 1.2 (scenario "Filtering never changes groups"): applying and clearing every AI filter leaves every group's
/// members and keeper as they were.
#[test]
fn scenario_duplicate_review_filtering_never_changes_groups() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    ps_service::insights::seed_demo_from(svc.engine().catalog(), 10_000, Some(&root))
        .expect("seed");
    let store = ReviewStore::in_memory();
    let snapshot = |svc: &ScanService| {
        let l = review::list(
            svc,
            &store,
            ReviewFilter::default(),
            ReviewSort::Reclaimable,
        )
        .expect("list");
        l.rows
            .iter()
            .map(|r| {
                let d = review::show(svc, &store, r.id).expect("show");
                let mut m: Vec<_> = d.group.members.iter().map(|m| m.file).collect();
                m.sort();
                (r.id, d.group.keeper, m)
            })
            .collect::<Vec<_>>()
    };
    let before = snapshot(&svc);
    let mut filters = vec![ReviewFilter {
        undescribed: true,
        ..ReviewFilter::default()
    }];
    for c in [
        InsightCriteria {
            people: true,
            ..InsightCriteria::default()
        },
        InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        },
        InsightCriteria {
            pets: true,
            ..InsightCriteria::default()
        },
        InsightCriteria {
            category: Some(ps_model::SceneCategory::Travel),
            ..InsightCriteria::default()
        },
    ] {
        filters.push(ReviewFilter {
            insight: Some(c),
            ..ReviewFilter::default()
        });
    }
    for f in filters {
        review::list(&svc, &store, f, ReviewSort::Reclaimable).expect("filtered");
        review::list(
            &svc,
            &store,
            ReviewFilter::default(),
            ReviewSort::Reclaimable,
        )
        .expect("cleared");
    }
    assert_eq!(snapshot(&svc), before);
}

/// ai-filtered-duplicates 1.3: the app's AI-filtered group list on the 1M-file catalog of `progress_over_1m`
/// (run that first), with an insight for every photo (seeded once). Measures the list after the gallery's index is
/// built: `PHOTOSQUIRREL_SYNTHETIC_DIR=target/synthetic cargo test --release -p ps-service --test review --
/// --ignored --nocapture --test-threads=1 progress_over_1m ai_filtered_list_over_1m`.
#[test]
#[ignore = "needs the catalog progress_over_1m builds, and seeds 1M insights on first run"]
fn ai_filtered_list_over_1m() {
    use ps_service::library::{Library, synthetic};
    use std::time::Instant;
    let base = std::env::var_os("PHOTOSQUIRREL_SYNTHETIC_DIR")
        .map(PathBuf::from)
        .expect("PHOTOSQUIRREL_SYNTHETIC_DIR");
    let dir = synthetic::ensure_at(base.join("review-progress-1m"), 1_000_000).expect("synthetic");
    let lib = Library::open(&dir).expect("open");
    let cat = lib.catalog().expect("catalog");
    assert_eq!(
        cat.read().group_totals().expect("totals").0,
        100_000,
        "run progress_over_1m first"
    );
    let marker = dir.join(".insights-seeded");
    if !marker.exists() {
        let t = Instant::now();
        let n = ps_service::insights::seed_demo(cat, 1_000_000).expect("seed");
        println!("seeded {n} insights in {:?}", t.elapsed());
        std::fs::write(&marker, b"ok").expect("marker");
    }
    let svc = lib.scan().expect("scan");
    let store = ReviewStore::in_memory();
    let criteria = InsightCriteria {
        people: true,
        ..InsightCriteria::default()
    };
    let filter = ReviewFilter {
        insight: Some(criteria),
        ..ReviewFilter::default()
    };
    let cache = review::ReviewCache::new();
    let run = |filter: ReviewFilter| {
        let mut sets = lib
            .insight_matches_many(&[criteria, InsightCriteria::default()])
            .expect("sets");
        let index = review::IndexMatcher {
            described: sets.pop().expect("described"),
            matching: sets.pop().expect("matching"),
        };
        cache
            .list(svc, &store, filter, ReviewSort::Reclaimable, Some(&index))
            .expect("list")
    };
    let t = Instant::now();
    let first = run(filter);
    println!(
        "first AI-filtered list (builds the index and the cache): {:?}, {} of {} groups match",
        t.elapsed(),
        first.matching,
        first.groups
    );
    let time = |label: &str, f: &dyn Fn()| {
        let mut ms: Vec<f64> = (0..10)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        println!("  {label}: median {:.1} ms, p95 {:.1} ms", ms[5], ms[9]);
    };
    time("AI-filtered reload", &|| {
        run(filter);
    });
    let ids: Vec<_> = first.rows.iter().map(|r| r.id).collect();
    let next = std::cell::Cell::new(0usize);
    time("reload after a review mark", &|| {
        let i = next.get();
        next.set(i + 1);
        review::set_reviewed(svc, &store, ids[i], true).expect("review");
        run(ReviewFilter::default());
    });
    time("groups key alone", &|| {
        review::groups_key(svc, &store).expect("key");
    });
    time("insight stamp alone", &|| {
        ps_service::insights::stamp(cat, lib.insight_store().expect("store")).expect("stamp");
    });
}

/// ai-filtered-duplicates 1b.3 (ADR-0133): a cached reload equals an uncached list after every kind of edit, and
/// reuses the loaded groups when only the review state changed.
#[test]
fn review_cache_reloads_see_every_edit() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let store = ReviewStore::in_memory();
    let cache = review::ReviewCache::new();
    let same = |what: &str| {
        for sort in [ReviewSort::Reclaimable, ReviewSort::NeedsAttention] {
            let cached = cache
                .list(&svc, &store, ReviewFilter::default(), sort, None)
                .expect("cached");
            let fresh = review::list(&svc, &store, ReviewFilter::default(), sort).expect("fresh");
            assert_eq!(cached, fresh, "after {what}");
        }
    };
    same("the first load");
    let first = cache.groups(&svc, &store).expect("groups");
    let all = review::list(
        &svc,
        &store,
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");

    review::set_reviewed(&svc, &store, all.rows[0].id, true).expect("review");
    same("a review mark");
    assert!(
        std::sync::Arc::ptr_eq(&first, &cache.groups(&svc, &store).expect("groups")),
        "a review mark reuses the loaded groups"
    );

    let d = review::show(&svc, &store, all.rows[1].id).expect("show");
    let other = d
        .group
        .members
        .iter()
        .find(|m| !m.keeper)
        .expect("copy")
        .file;
    svc.groups().set_keeper(d.group.id, other).expect("keeper");
    same("a keeper override");

    let (a, b) = (all.rows[2].id, all.rows[3].id);
    let merged = svc.groups().merge(a, b).expect("merge");
    same("a merge");

    let m = review::show(&svc, &store, merged.id).expect("show");
    let out = m
        .group
        .members
        .iter()
        .find(|x| !x.keeper)
        .expect("member")
        .file;
    svc.groups().separate(merged.id, out).expect("separate");
    same("a separation");

    if let Some(s) = review::suggestions(&svc, &store, 1)
        .expect("suggestions")
        .first()
    {
        review::dismiss(&svc, &store, s.info.file_a, s.info.file_b).expect("dismiss");
        same("a dismissed suggestion");
    }

    // A rescan with another copy of a keeper.
    let keeper = review::show(&svc, &store, all.rows[4].id).expect("show");
    let path = keeper
        .group
        .member(keeper.group.keeper)
        .expect("keeper")
        .path
        .clone();
    std::fs::copy(
        &path,
        path.with_file_name(format!(
            "cache-copy-{}",
            path.file_name().expect("name").to_string_lossy()
        )),
    )
    .expect("copy");
    svc.start(&[]).expect("rescan");
    svc.wait().expect("wait");
    same("a rescan");
}

/// ai-filtered-duplicates 1b.2 (ADR-0133): the insight stamp still changes for a new result and for a result
/// replaced in place, though it recounts only when one of them happened.
#[test]
fn insight_stamp_sees_new_and_replaced_results() {
    use ps_service::insights::{InsightStore, stamp};
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let cat = svc.engine().catalog();
    let store = InsightStore::in_memory();
    let all = review::list(
        &svc,
        &ReviewStore::in_memory(),
        ReviewFilter::default(),
        ReviewSort::Reclaimable,
    )
    .expect("list");
    let s0 = stamp(cat, &store).expect("stamp");
    assert_eq!(stamp(cat, &store).expect("again"), s0, "nothing changed");
    describe(&svc, all.rows[0].keeper, insight("pets", 0, 0, &["cat"]));
    let s1 = stamp(cat, &store).expect("stamp");
    assert_eq!(s1.results, s0.results + 1);
    assert_ne!(s1, s0);
    // The same content, provider and model described again: replaced in place.
    describe(&svc, all.rows[0].keeper, insight("people", 1, 0, &[]));
    let s2 = stamp(cat, &store).expect("stamp");
    assert_eq!(s2.results, s1.results, "replaced, not added");
    assert_ne!(s2, s1, "a replaced result changes the stamp");
}
