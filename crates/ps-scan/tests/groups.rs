//! The post-scan stage (duplicate-groups task 3.3): every completed run regroups incrementally and stores the
//! groups through the catalog writer. Scanning the corpus drive by drive yields exactly the groups a full
//! regroup of everything produces, and the stored groups score at the ADR-0025 gate.

mod common;

use std::collections::HashMap;

use common::*;
use ps_fixtures::eval::{ResultGroup, Score};
use ps_model::{FileId, SourceId};

#[test]
fn post_stage_regroups_incrementally_and_equals_a_full_regroup() {
    let _g = shared();
    let c = corpus();
    let env = env();
    let mut ids: Vec<(String, SourceId)> = Vec::new();
    for (step, drive) in c.truth.drives.iter().enumerate() {
        let id = env.add(&c.root.join(drive));
        ids.push((drive.clone(), id));
        let summary = env.scan(&[id]);
        completed(&summary);
        let g = summary.grouping.expect("completed runs regroup");
        assert_eq!(g.full, step == 0, "only the first refresh is full");
        if step > 0 {
            assert!(g.changed > 0 && g.changed < g.inputs, "{g:?}");
        }
        // A fresh engine regrouping everything finds nothing to write: incremental == full.
        let check = ps_scan::GroupEngine::new()
            .refresh(env.s().catalog())
            .expect("full regroup");
        assert!(check.full);
        assert_eq!(
            (check.groups_written, check.groups_removed),
            (0, 0),
            "after {drive}: {check:?}"
        );
        assert_eq!(check.groups, g.groups);
    }
    // A no-change rescan rewrites nothing.
    let again = env.scan(&[]).grouping.expect("regroup");
    assert_eq!(
        (again.changed, again.groups_written, again.groups_removed),
        (0, 0, 0)
    );

    // Score the stored groups against the truth (paths are `<drive>/<relative path>`).
    let roots: HashMap<SourceId, String> = ids.iter().map(|(d, s)| (*s, d.clone())).collect();
    let r = env.readers();
    let path_of = |f: FileId| {
        let rec = r.file_by_id(f).expect("file").expect("present");
        format!("{}/{}", roots[&rec.key.source], rec.key.path.display())
    };
    let (n, _, _) = r.group_totals().expect("totals");
    let results: Vec<ResultGroup> = r
        .groups(ps_catalog::GroupOrder::Id, 0, n)
        .expect("groups")
        .iter()
        .map(|s| {
            let g = r.group(s.id).expect("group").expect("present");
            ResultGroup {
                members: g.members.iter().map(|m| path_of(m.file)).collect(),
                keeper: Some(path_of(g.keeper)),
            }
        })
        .collect();
    let score = Score::evaluate(&c.truth, &results);
    println!("{}", score.measure_lines("scan-grouping"));
    assert!(score.passes_gate(), "{score:?}");
}
