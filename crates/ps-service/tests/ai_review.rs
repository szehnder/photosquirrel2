//! The AI advisory boundary (spec `ai-review` "Advisory only", ADR-0021): storing insights for every member of
//! every group, correcting them and deciding their suggestions never changes groups or keepers.

use std::path::Path;

use ps_catalog::AiResultRow;
use ps_fixtures::{DirWriter, Params, Preset};
use ps_service::insights::{self, Correction, InsightFilter, InsightStore};
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

fn scanned(dir: &Path) -> ScanService {
    let root = dir.join("corpus");
    ps_fixtures::generate_with(
        11,
        Preset::Small,
        tiny(),
        &DirWriter::new(&root).expect("w"),
    )
    .expect("generate");
    let svc = ScanService::open(&dir.join("data"), ScanOptions::default()).expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    svc
}

#[test]
fn scenario_ai_review_ai_never_changes_keeper() {
    let tmp = tempfile::tempdir().expect("tmp");
    let svc = scanned(tmp.path());
    let g = svc.groups();
    let before = g.named().expect("named");
    assert!(!before.is_empty());
    let catalog = svc.engine().catalog();
    let r = catalog.read();
    // An insight for every member of every group, half of them "not a photo" verdicts.
    let mut n = 0;
    for (_, members, _) in r.group_memberships().expect("memberships") {
        for f in members {
            let Some(c) = r.file_by_id(f).expect("file").and_then(|x| x.identity) else {
                continue;
            };
            let i = insights::sample_insight(n);
            n += 1;
            catalog
                .writer()
                .ai_complete(AiResultRow {
                    content: c,
                    provider: "recorder".into(),
                    model: "fixture".into(),
                    schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
                    prompt_version: 1,
                    insight: serde_json::to_string(&i).expect("json"),
                    latency_ms: 1,
                    requests: 1,
                    at_ms: n as i64,
                })
                .expect("store");
        }
    }
    let store = InsightStore::in_memory();
    let items = insights::items(catalog, &store, &InsightFilter::default(), 10_000).expect("items");
    assert!(!items.is_empty());
    for i in &items {
        store.correct(
            &i.content,
            Correction {
                category: Some(ps_model::SceneCategory::Travel),
                ..Correction::default()
            },
        );
        // Rejecting a suggestion records only the decision.
        insights::decide(catalog, &store, &i.content, false).expect("decide");
    }
    g.regroup().expect("regroup");
    assert_eq!(
        g.named().expect("named"),
        before,
        "members and keepers unchanged"
    );
}
