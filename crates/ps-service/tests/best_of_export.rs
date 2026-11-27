//! Exporting the best of each stack (best-of-stacks tasks 4.1-4.3, ADR-0135): best of only copies each stack's
//! pick and every photo in no stack; the shots left out are not picked, never pending and never cleaned up.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ps_export::{ExecConfig, ProfileSettings};
use ps_model::{BestOf, Disposition, FileId, SourceId, StackThresholds};
use ps_service::export::profile_settings;
use ps_service::review::ReviewStore;
use ps_service::scan::{ScanOptions, ScanService};
use ps_service::stacks::StackCache;

fn corpus() -> &'static Path {
    static C: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        ps_fixtures::shared_corpus(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            "best-of-corpus-small-42",
            42,
            ps_fixtures::Preset::Small,
        )
    })
}

fn scanned(dir: &Path) -> ScanService {
    let svc = ScanService::open(&dir.join("data"), ScanOptions::default()).expect("open");
    svc.add_source(corpus()).expect("add");
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    svc
}

fn settings(best_of: BestOf) -> ProfileSettings {
    profile_settings("google")
        .unwrap()
        .with_best_of(best_of, StackThresholds::default())
}

fn export(svc: &ScanService, library: &Path, best_of: BestOf) {
    let (_, report) = svc
        .exports()
        .run(
            library,
            settings(best_of),
            &ExecConfig::default(),
            &AtomicBool::new(false),
        )
        .expect("export");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
}

/// Every member of each group (or the photo itself) of the shots that are not their stack's pick, and the picks.
fn left_out_and_picks(svc: &ScanService) -> (HashSet<FileId>, HashSet<FileId>) {
    let v = StackCache::new()
        .view(svc, &ReviewStore::in_memory(), StackThresholds::default())
        .unwrap();
    let shots = v.stacks.not_picked();
    let picks: HashSet<FileId> = v.stacks.stacks.iter().map(|s| s.pick).collect();
    let mut out = shots.clone();
    for (_, members, keeper_set) in svc.engine().catalog().read().group_memberships().unwrap() {
        if keeper_set.first().is_some_and(|k| shots.contains(k)) {
            out.extend(members);
        }
    }
    assert!(!shots.is_empty(), "the corpus has stacks");
    (out, picks)
}

#[test]
fn scenario_export_best_of_only() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let library = tmp.path().join("library");
    let (left_out, picks) = left_out_and_picks(&svc);
    export(&svc, &library, BestOf::PicksOnly);
    let (_, d, _) = svc.exports().dispositions(&library, false).unwrap();
    for x in &d {
        if left_out.contains(&x.file) {
            assert!(
                x.library_path.is_none(),
                "{:?} is not in the library",
                x.file
            );
        }
        if picks.contains(&x.file) {
            assert!(
                matches!(x.disposition, Disposition::Exported | Disposition::Covered),
                "the pick {:?} is in the library: {:?}",
                x.file,
                x.disposition
            );
        }
    }
    // Every photo in no stack is still exported: nothing else is pending.
    assert!(
        d.iter().all(|x| x.disposition != Disposition::Pending),
        "nothing pending"
    );
}

#[test]
fn scenario_export_default_unchanged() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let library = tmp.path().join("library");
    let default = profile_settings("google").unwrap();
    assert_eq!(default.best_of, BestOf::Everything);
    let (plain, _) = svc.exports().plan(&library, default).unwrap();
    let (everything, _) = svc
        .exports()
        .plan(&library, settings(BestOf::Everything))
        .unwrap();
    assert_eq!(plain.totals, everything.totals);
    let (only, _) = svc
        .exports()
        .plan(&library, settings(BestOf::PicksOnly))
        .unwrap();
    assert!(
        only.totals.files < plain.totals.files,
        "best of only copies fewer files"
    );
}

#[test]
fn scenario_completion_report_shots_not_picked_after_a_best_of_export() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let library = tmp.path().join("library");
    let (left_out, _) = left_out_and_picks(&svc);
    export(&svc, &library, BestOf::PicksOnly);
    let (_, d, _) = svc.exports().dispositions(&library, false).unwrap();
    for x in d.iter().filter(|x| left_out.contains(&x.file)) {
        assert!(
            matches!(
                x.disposition,
                Disposition::NotPicked | Disposition::Excluded | Disposition::Missing
            ),
            "{:?}: {:?}",
            x.file,
            x.disposition
        );
    }
    assert!(d.iter().any(|x| x.disposition == Disposition::NotPicked));
    assert!(d.iter().all(|x| x.disposition != Disposition::Pending));
    let c = svc.exports().completion(Some(&library), false).unwrap();
    assert!(
        c.totals.files_of(Disposition::NotPicked) > 0,
        "the report counts them"
    );
    assert!(c.totals.fully_accounted(), "a choice, accounted for");
}

#[test]
fn scenario_completion_report_not_picked_shots_are_exported_by_a_full_export() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let library = tmp.path().join("library");
    let (left_out, _) = left_out_and_picks(&svc);
    export(&svc, &library, BestOf::PicksOnly);
    export(&svc, &library, BestOf::Everything);
    let (_, d, _) = svc.exports().dispositions(&library, false).unwrap();
    assert!(d.iter().all(|x| x.disposition != Disposition::NotPicked));
    for x in d.iter().filter(|x| left_out.contains(&x.file)) {
        assert!(
            matches!(
                x.disposition,
                Disposition::Exported | Disposition::Covered | Disposition::Excluded
            ),
            "{:?}: {:?}",
            x.file,
            x.disposition
        );
    }
}

#[test]
fn scenario_cleanup_shots_not_picked_are_not_eligible() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let library = tmp.path().join("library");
    let (left_out, _) = left_out_and_picks(&svc);
    export(&svc, &library, BestOf::PicksOnly);
    let plan = svc
        .cleanups()
        .plan(SourceId(1), None, Some(&library))
        .unwrap();
    assert!(!plan.moves.is_empty(), "something is eligible");
    for m in &plan.moves {
        assert!(
            !left_out.contains(&m.file),
            "{:?} is not picked and must stay",
            m.file
        );
    }
}
