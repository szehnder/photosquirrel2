//! Migration 4 (duplicate groups): upgrade from a version-3 catalog, group storage round trips, the atomic
//! delta application and user constraints keyed by content identity.

use ps_catalog::{
    Catalog, GroupChanges, GroupOrder, GroupRow, MemberEdge, MemberRow, SuggestionRow, db,
};
use ps_model::{ContentId, FileId};
use rusqlite::Connection;

fn group(id: i64, members: &[i64], digest: u8) -> GroupRow {
    GroupRow {
        id: FileId(id),
        keeper: FileId(members[0]),
        display: FileId(members[0]),
        keeper_set: vec![FileId(members[0])],
        reasons: serde_json::json!([{ "text": "more pixels: 4000x3000 vs 1024x768" }]),
        reclaimable_bytes: 1000 * (members.len() as u64 - 1),
        digest: [digest; 32],
        members: members
            .iter()
            .enumerate()
            .map(|(rank, &f)| MemberRow {
                file: FileId(f),
                rank: rank as u32,
                edge: if rank == 0 {
                    MemberEdge::Keeper
                } else {
                    MemberEdge::Derivative
                },
                via: (rank > 0).then_some(FileId(members[0])),
                phash_distance: (rank > 0).then_some(3),
                dhash_distance: (rank > 0).then_some(4),
                in_keeper_set: rank == 0,
            })
            .collect(),
        reviewed_at_ms: None,
    }
}

#[test]
fn groups_migration_upgrades_a_v3_catalog() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("catalog.db");
    db::create_catalog_at_version(&path, 3).expect("v3 fixture");
    {
        let conn = Connection::open(&path).expect("raw");
        conn.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, X'2F6D6E74', 0)",
            [],
        )
        .expect("source");
    }
    let cat = Catalog::open_file(&path, Default::default()).expect("upgrade");
    assert_eq!(
        cat.read().schema_version().expect("version"),
        ps_catalog::latest_schema_version()
    );
    assert_eq!(cat.read().sources().expect("sources").len(), 1);
    let names: Vec<String> = cat
        .read()
        .with(|c| {
            let mut s = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
            Ok(s.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .expect("tables");
    for t in [
        "dup_group",
        "dup_member",
        "dup_suggestion",
        "keeper_override",
        "dup_separation",
        "dup_merge",
        "ai_queue",
        "export_ledger",
        "user_inclusion",
    ] {
        assert!(names.iter().any(|n| n == t), "missing {t}: {names:?}");
    }
    cat.close().expect("close");
}

#[test]
fn group_changes_apply_atomically_and_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let w = cat.writer();
    w.apply_groups(GroupChanges {
        clear: true,
        groups: vec![group(1, &[2, 1, 3], 1), group(4, &[4, 5], 2)],
        suggestions: vec![SuggestionRow {
            file_a: FileId(3),
            file_b: FileId(9),
            phash_distance: 2,
            dhash_distance: 3,
            reason: serde_json::json!({ "text": "taken at different times" }),
        }],
        ..Default::default()
    })
    .expect("apply");
    let r = cat.read();
    assert_eq!(r.group_totals().expect("totals"), (2, 5, 3000));
    let g = r.group(FileId(1)).expect("query").expect("group 1");
    assert_eq!(g, group(1, &[2, 1, 3], 1));
    assert_eq!(g.reason_texts(), ["more pixels: 4000x3000 vs 1024x768"]);
    assert_eq!(r.group_of_file(FileId(3)).expect("of"), Some(FileId(1)));
    let page = r.groups(GroupOrder::Reclaimable, 0, 10).expect("page");
    assert_eq!(page[0].id, FileId(1));
    assert_eq!(page[0].member_count, 3);
    assert_eq!(r.suggestions(Some(FileId(9)), 10).expect("sugg").len(), 1);

    w.set_group_reviewed(FileId(1), true).expect("reviewed");
    // File 5 moves into group 1 (replaced in place, review kept) and group 4 disappears: the member's primary
    // key would collide if the insert ran before the deletion.
    w.apply_groups(GroupChanges {
        removed_groups: vec![FileId(4)],
        groups: vec![group(1, &[2, 1, 3, 5], 3)],
        removed_suggestions: vec![(FileId(3), FileId(9))],
        ..Default::default()
    })
    .expect("delta");
    let g = r.group(FileId(1)).expect("query").expect("group 1");
    assert_eq!(g.members.len(), 4);
    assert!(g.reviewed_at_ms.is_some());
    assert!(r.group(FileId(4)).expect("query").is_none());
    assert_eq!(r.group_of_file(FileId(5)).expect("of"), Some(FileId(1)));
    assert_eq!(r.suggestion_count().expect("count"), 0);
    assert_eq!(
        r.group_digests().expect("digests"),
        vec![(FileId(1), [3; 32])]
    );

    // A failing change is rolled back entirely: a member listed twice violates the primary key.
    let bad = GroupChanges {
        removed_groups: vec![FileId(1)],
        groups: vec![group(7, &[7, 8], 4), group(9, &[9, 8], 5)],
        ..Default::default()
    };
    assert!(w.apply_groups(bad).is_err());
    assert!(r.group(FileId(1)).expect("query").is_some());
    assert!(r.group(FileId(7)).expect("query").is_none());
    cat.close().expect("close");
}

#[test]
fn constraints_are_keyed_by_content_and_persist() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = ContentId::full([1; 32]);
    let b = ContentId::full([2; 32]);
    let c = ContentId::sampled([3; 32]);
    {
        let cat = Catalog::open(tmp.path()).expect("open");
        let w = cat.writer();
        w.set_keeper_override(a, Some(FileId(11)), vec![])
            .expect("override");
        // Choosing another member of the same group replaces the first override.
        w.set_keeper_override(b, Some(FileId(12)), vec![a])
            .expect("override");
        w.set_separations(vec![(c, a), (b, a)], true).expect("sep");
        w.set_merges(vec![(c, b)], true).expect("merge");
        w.set_separations(vec![(a, b)], false).expect("unsep");
        cat.close().expect("close");
    }
    let cat = Catalog::open(tmp.path()).expect("reopen");
    let k = cat.read().group_constraints().expect("constraints");
    assert_eq!(k.keeper_overrides, vec![(b, Some(FileId(12)))]);
    assert_eq!(k.separations, vec![(a, c)]);
    assert_eq!(k.merges, vec![(b, c)]);
    cat.writer().clear_keeper_overrides(vec![b]).expect("clear");
    assert!(
        cat.read()
            .group_constraints()
            .expect("c")
            .keeper_overrides
            .is_empty()
    );
}

/// Every edit that decides cleanup safety bumps the constraints generation once it is committed; the cleanup
/// journal's own writes and group review marks do not (ADR-0099).
#[test]
fn constraint_edits_bump_the_constraints_generation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let w = cat.writer();
    let a = ContentId::full([1; 32]);
    let b = ContentId::full([2; 32]);
    let mut seen = w.constraints_generation();
    let mut bumped = |what: &str| {
        let now = w.constraints_generation();
        assert!(now > seen, "{what} bumps the generation");
        seen = now;
    };
    w.set_keeper_override(a, Some(FileId(11)), vec![])
        .expect("override");
    bumped("a keeper override");
    w.clear_keeper_overrides(vec![a]).expect("clear");
    bumped("clearing a keeper override");
    w.set_separations(vec![(a, b)], true).expect("sep");
    bumped("a separation");
    w.set_merges(vec![(a, b)], true).expect("merge");
    bumped("a merge");
    w.apply_groups(GroupChanges::default()).expect("regroup");
    bumped("a regroup");
    w.submit(ps_catalog::Command::SetUserDecision {
        file: FileId(1),
        decision: None,
        reason: None,
    })
    .expect("decision");
    w.flush().expect("flush");
    bumped("a user decision");
    let before = w.constraints_generation();
    w.set_group_reviewed(FileId(1), true).expect("reviewed");
    w.flush().expect("flush");
    assert_eq!(
        w.constraints_generation(),
        before,
        "a review mark does not decide cleanup safety"
    );
}
