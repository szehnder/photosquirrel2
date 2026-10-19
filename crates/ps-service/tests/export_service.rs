//! Export through the service (export tasks 1.1, 5.1 and the 7.2 gate): the catalog-built input, the
//! catalog-backed ledger, dispositions derived from catalog facts, and completion reports.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ps_export::ExecConfig;
use ps_model::{Disposition, ExportProfile};
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn small_corpus() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-small");
    if !ps_fixtures::Truth::read(&root).is_ok_and(|t| t.seed == 42 && t.preset == "small") {
        let _ = std::fs::remove_dir_all(&root);
        let w = ps_fixtures::DirWriter::new(&root).expect("writer");
        ps_fixtures::generate(42, ps_fixtures::Preset::Small, &w).expect("generate");
    }
    root.canonicalize().expect("canonical corpus root")
}

fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.clone(), std::fs::metadata(&p).map_or(0, |m| m.len())));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn export_small_corpus_reexport_copies_nothing_and_every_drive_is_accounted_for() {
    let corpus = small_corpus();
    let before = tree(&corpus);
    let tmp = tempfile::tempdir().expect("tmp");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&corpus.join(d)).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("scan");
    let ex = svc.exports();
    let library = tmp.path().join("Library");
    let settings = profile_settings("lightroom").expect("profile");
    assert_eq!(settings.profile, ExportProfile::Lightroom);

    // Dry run: a plan, nothing written.
    let (plan, input) = ex.plan(&library, settings).expect("plan");
    assert!(plan.totals.files > 0);
    assert_eq!(
        input.units.len(),
        plan.bundles.iter().map(|b| b.units.len()).sum::<usize>()
    );
    assert!(!library.exists(), "the dry run writes nothing");

    let cancel = AtomicBool::new(false);
    let (_, report) = ex
        .run(&library, settings, &ExecConfig::default(), &cancel)
        .expect("export");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        report.copied + report.adopted,
        plan.totals.files + plan.totals.adopted
    );

    // Re-export: nothing new to copy (the ledger lives in the catalog).
    let (again, _) = ex.plan(&library, settings).expect("replan");
    assert_eq!(again.totals.files, 0, "{:?}", again.totals);
    assert_eq!(again.totals.already_exported, plan.totals.files);
    let (_, r2) = ex
        .run(&library, settings, &ExecConfig::default(), &cancel)
        .expect("re-export");
    assert_eq!(r2.copied, 0);

    // Every drive 100% accounted for, verified by re-hashing every library copy.
    let c = ex.completion(None, true).expect("completion");
    assert!(c.report.mismatches.is_empty());
    for s in &c.report.sources {
        assert!(
            s.counts.fully_accounted(),
            "source {} pending {} unreadable {}",
            s.source,
            s.counts.files_of(Disposition::Pending),
            s.counts.files_of(Disposition::Unreadable)
        );
    }
    assert!(c.totals.files_of(Disposition::Exported) > 0);
    assert!(c.totals.files_of(Disposition::Covered) > 0);
    assert!(c.totals.files_of(Disposition::Excluded) > 0);
    // Sources untouched.
    assert_eq!(tree(&corpus), before);

    // Deleting a library copy makes its files pending again (dispositions are derived, never stored).
    let (_, d, _) = ex.dispositions(&library, false).expect("dispositions");
    let exported = d
        .iter()
        .find(|x| x.disposition == Disposition::Exported)
        .and_then(|x| x.library_path.clone())
        .expect("an exported file");
    std::fs::remove_file(exported.under(&library)).expect("delete library copy");
    let c = ex.completion(Some(&library), false).expect("completion");
    assert!(c.totals.files_of(Disposition::Pending) >= 1);
    let pending = ex
        .files_with(&library, Disposition::Pending, 10)
        .expect("drill down");
    assert!(!pending.is_empty() && pending.iter().all(|p| p.path.starts_with(&corpus)));
}
