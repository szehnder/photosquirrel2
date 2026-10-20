//! The M1 corpus gate end to end through the service (ADR-0025; duplicate-groups task 6.2): generate (or reuse)
//! the medium corpus with seed 42 under `target/corpus-medium`, add its three drives as sources, scan, let the
//! post-scan regroup store the groups, and score the stored groups against `truth.json`.
//!
//! `cargo test -p ps-service --test corpus_gate --release` runs the medium corpus; debug builds use the small one
//! (in a temp dir) unless `PS_GATE_CORPUS=medium`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ps_fixtures::eval::{ResultGroup, Score};
use ps_fixtures::{DirWriter, Preset, Truth};
use ps_service::scan::{ScanOptions, ScanService};

fn preset() -> Preset {
    match std::env::var("PS_GATE_CORPUS").ok().as_deref() {
        Some("small") => Preset::Small,
        Some("medium") => Preset::Medium,
        _ if cfg!(debug_assertions) => Preset::Small,
        _ => Preset::Medium,
    }
}

/// The corpus root: an existing `target/corpus-<preset>` of the workspace when its truth matches, else one
/// generated once under CARGO_TARGET_TMPDIR (the workspace's ./target need not exist).
fn corpus(p: Preset) -> (PathBuf, Truth) {
    let matches = |t: &Truth| t.seed == 42 && t.preset == p.as_str();
    let shared = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("corpus-{}", p.as_str()));
    if let Ok(t) = Truth::read(&shared)
        && matches(&t)
    {
        return (shared, t);
    }
    let root = ps_fixtures::shared_dir(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        &format!("corpus-gate-{}-42", p.as_str()),
        |root| {
            let t = Instant::now();
            let w = DirWriter::new(root).expect("corpus writer");
            ps_fixtures::generate(42, p, &w).expect("generate");
            eprintln!(
                "generated the {} corpus in {:.1} s",
                p.as_str(),
                t.elapsed().as_secs_f64()
            );
        },
    );
    let truth = Truth::read(&root).expect("truth.json");
    assert!(matches(&truth), "cached corpus at {}", root.display());
    (root, truth)
}

fn scored(p: Preset) -> Score {
    let (root, truth) = corpus(p);
    let data = tempfile::tempdir().expect("data");
    let svc = ScanService::open(data.path(), ScanOptions::default()).expect("open");
    for d in &truth.drives {
        svc.add_source(&root.join(d)).expect("add source");
    }
    let t = Instant::now();
    svc.start(&[]).expect("start");
    let summary = svc.wait().expect("summary");
    let grouping = summary.grouping.clone().expect("regrouped");
    println!(
        "scanned {} files in {:.1} s; {} groups ({} files) in {} ms",
        summary.stages.total(),
        t.elapsed().as_secs_f64(),
        grouping.groups,
        grouping.grouped_files,
        grouping.elapsed_ms
    );
    let results: Vec<ResultGroup> = svc
        .groups()
        .named()
        .expect("groups")
        .into_iter()
        .map(|g| ResultGroup {
            members: g.members,
            keeper: Some(g.keeper),
        })
        .collect();
    let s = Score::evaluate(&truth, &results);
    println!(
        "corpus {}: precision={:.4} recall={:.4} keeper={:.4} must-not-merge={} unknown={}",
        p.as_str(),
        s.precision,
        s.recall,
        s.keeper_accuracy,
        s.must_not_merge_violations,
        s.unknown_paths
    );
    println!(
        "{}",
        s.measure_lines(&format!("service-grouping-{}", p.as_str()))
    );
    s
}

/// Scanned and scored once per test binary.
fn score() -> &'static Score {
    static S: std::sync::OnceLock<Score> = std::sync::OnceLock::new();
    S.get_or_init(|| scored(preset()))
}

#[test]
fn scenario_duplicate_detection_corpus_grouping_gate() {
    let s = score();
    assert_eq!(s.unknown_paths, 0, "every grouped path maps to the truth");
    assert!(s.precision >= 0.995, "precision {}", s.precision);
    assert!(s.recall >= 0.97, "recall {}", s.recall);
    assert_eq!(s.must_not_merge_violations, 0, "{:?}", s.violations);
}

#[test]
fn scenario_best_copy_corpus_keeper_gate() {
    let s = score();
    assert!(
        s.keeper_accuracy >= 0.99,
        "keeper accuracy {}",
        s.keeper_accuracy
    );
    assert!(s.passes_gate(), "{s:?}");
}
