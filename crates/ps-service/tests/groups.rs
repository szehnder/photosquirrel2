//! Duplicate groups through the service facade (duplicate-groups task 4.6): listing and explaining groups,
//! keeper overrides that persist and survive rescans, separations and merges.

use std::path::{Path, PathBuf};

use ps_fixtures::{DirWriter, Params, Preset};
use ps_model::FileId;
use ps_service::groups::{GroupDetail, GroupOrder, MemberEdge};
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
    let s = svc.wait().expect("summary");
    assert!(s.grouping.is_some());
    svc
}

/// A group whose ranking has a member with other content than the keeper (an override can change it).
fn overridable(svc: &ScanService) -> (GroupDetail, FileId) {
    let g = svc.groups();
    for item in g.list(GroupOrder::Reclaimable, 0, 1000).expect("list") {
        let d = g.show(item.id).expect("show");
        let keeper = d.member(d.keeper).expect("keeper member").clone();
        if let Some(m) = d.members.iter().find(|m| {
            !m.keeper
                && m.edge == MemberEdge::Derivative
                && m.size != keeper.size
                && !m.in_keeper_set
        }) {
            let f = m.file;
            return (d, f);
        }
    }
    panic!("no group with a derivative member");
}

#[test]
fn groups_listed_and_explained() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    // Member paths are under the stored, canonical source roots (macOS: /private/var/...; Windows: long
    // names, no `\\?\` prefix), so compare with the canonical corpus folder.
    let root = ps_model::path::simplify_canonical(std::fs::canonicalize(&root).expect("canonical"));
    let g = svc.groups();
    let t = g.totals().expect("totals");
    assert!(t.groups > 5 && t.grouped_files > t.groups, "{t:?}");
    let list = g.list(GroupOrder::Reclaimable, 0, 1000).expect("list");
    assert_eq!(list.len() as u64, t.groups);
    assert!(
        list.windows(2)
            .all(|w| w[0].reclaimable_bytes >= w[1].reclaimable_bytes)
    );
    for item in &list {
        let d = g.show(item.id).expect("show");
        assert_eq!(d.members[0].file, d.keeper);
        assert!(!d.keeper_reasons.is_empty());
        assert!(d.members.iter().all(|m| m.path.starts_with(&root)));
        for m in &d.members[1..] {
            assert!(m.via.is_some());
            if m.edge == MemberEdge::Derivative {
                assert!(m.phash_distance.is_some() && m.dhash_distance.is_some());
                assert!(m.reason.contains("pHash distance"));
            }
        }
        assert_eq!(g.group_of(d.members[1].file).expect("of"), Some(d.id));
    }
    let (d, f) = overridable(&svc);
    let lines = g.explain_pair(d.keeper, f).expect("explain");
    assert!(
        lines.iter().any(|l| l.starts_with("decision: duplicate")),
        "{lines:?}"
    );
}

#[test]
fn scenario_best_copy_keeper_override_persists() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let chosen;
    {
        let svc = scanned(tmp.path(), &root);
        let (d, f) = overridable(&svc);
        chosen = f;
        let after = svc.groups().set_keeper(d.id, f).expect("set keeper");
        assert_eq!(after.keeper, f);
        assert_eq!(after.keeper_reasons[0], "chosen by you");
        assert!(after.chosen_by_you);
    }
    // Closed and reopened (a new process would do the same).
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("reopen");
    let g = svc.groups();
    let id = g.group_of(chosen).expect("of").expect("grouped");
    let d = g.show(id).expect("show");
    assert_eq!(d.keeper, chosen);
    assert_eq!(d.keeper_reasons[0], "chosen by you");
    // A regroup from scratch in the new process keeps it too.
    g.regroup().expect("regroup");
    assert_eq!(g.show(id).expect("show").keeper, chosen);
    // Clearing restores the automatic choice.
    let back = g.clear_keeper(id).expect("clear");
    assert_ne!(back.keeper, chosen);
    assert!(!back.chosen_by_you);
}

fn copy_into(src: &Path, dst_dir: &Path, name: &str) {
    std::fs::create_dir_all(dst_dir).expect("mkdir");
    std::fs::copy(src, dst_dir.join(name)).expect("copy");
}

#[test]
fn scenario_best_copy_override_survives_rescan() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let (d, f) = overridable(&svc);
    let before = svc.groups().set_keeper(d.id, f).expect("set keeper");
    // New copies of the group's (former automatic) keeper and of the chosen file appear on a new source.
    let extra = tmp.path().join("usb");
    let old_keeper = d.member(d.keeper).expect("keeper").path.clone();
    let chosen_path = before.member(f).expect("chosen").path.clone();
    copy_into(&old_keeper, &extra.join("DCIM"), "copy_of_keeper.jpg");
    copy_into(&chosen_path, &extra.join("Album"), "copy_of_chosen.jpg");
    let src = svc.add_source(&extra).expect("add").source.id;
    svc.start(&[src]).expect("start");
    let s = svc.wait().expect("summary");
    let gr = s.grouping.expect("regrouped");
    assert!(!gr.full && gr.changed >= 2, "{gr:?}");
    let g = svc.groups();
    let now = g
        .show(g.group_of(f).expect("of").expect("grouped"))
        .expect("show");
    assert_eq!(now.members.len(), before.members.len() + 2);
    assert!(
        now.members
            .iter()
            .any(|m| m.path.ends_with("copy_of_keeper.jpg"))
    );
    // The chosen content stays the keeper (the chosen file or its new exact copy, never another content).
    let keeper = now.member(now.keeper).expect("keeper");
    assert!(keeper.file == f || keeper.path.ends_with("copy_of_chosen.jpg"));
    assert_eq!(now.keeper_reasons[0], "chosen by you");
}

#[test]
fn separated_member_stays_apart_and_merge_joins() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = corpus(tmp.path());
    let svc = scanned(tmp.path(), &root);
    let g = svc.groups();
    let (d, f) = overridable(&svc);
    let out = g.separate(d.id, f).expect("separate");
    assert!(out.separations_added >= 1);
    assert_ne!(out.member_group, out.rest_group);
    assert!(!g.separations().expect("seps").is_empty());
    // Survives a full regroup in a new process.
    drop(svc);
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("reopen");
    let g = svc.groups();
    g.regroup().expect("regroup");
    let rest = g.group_of(d.keeper).expect("of");
    assert!(rest.is_some());
    assert_ne!(g.group_of(f).expect("of"), rest);

    // Merging two unrelated groups makes one group with a user-merge edge.
    let list = g.list(GroupOrder::Id, 0, 1000).expect("list");
    let (a, b) = (list[0].id, list[1].id);
    let total = g.show(a).expect("a").members.len() + g.show(b).expect("b").members.len();
    let merged = g.merge(a, b).expect("merge");
    assert_eq!(merged.members.len(), total);
    assert!(
        merged
            .members
            .iter()
            .any(|m| m.edge == MemberEdge::UserMerge)
    );
}
