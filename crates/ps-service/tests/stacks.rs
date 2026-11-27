//! Stacks over a scanned corpus (best-of-stacks): the corpus gate for the stack rule and the calibration table
//! of ADR-0135 (`cargo test -p ps-service --test stacks -- --nocapture` prints it).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ps_dedup::{StackSimilarity, StackThresholds, Stacks};
use ps_fixtures::{MustNotMergeKind, Truth};
use ps_model::FileId;
use ps_service::scan::{ScanOptions, ScanService};
use ps_service::stacks::StackInputs;

fn corpus() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        ps_fixtures::shared_corpus(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            "stacks-corpus-small-42",
            42,
            ps_fixtures::Preset::Small,
        )
    })
}

/// The whole corpus scanned as one source (relative paths then start with the drive name, as in the truth file).
fn scanned(dir: &Path) -> ScanService {
    let svc = ScanService::open(&dir.join("data"), ScanOptions::default()).expect("open");
    svc.add_source(corpus()).expect("add");
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    svc
}

/// Each analyzed file's shot: its group's keeper, or itself.
fn shot_of(svc: &ScanService, inputs: &StackInputs) -> HashMap<FileId, FileId> {
    let mut out: HashMap<FileId, FileId> = inputs.inputs.iter().map(|x| (x.file, x.file)).collect();
    svc.engine()
        .catalog()
        .read()
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT m.file_id, g.keeper_file_id FROM dup_member m JOIN dup_group g ON g.id = m.group_id",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                out.insert(FileId(r.get(0)?), FileId(r.get(1)?));
            }
            Ok(())
        })
        .unwrap();
    out
}

/// File id of a truth path (`drive-a/Pictures/...`).
fn file_of(inputs: &StackInputs, truth_path: &str) -> Option<FileId> {
    let suffix = format!("/{truth_path}");
    inputs
        .inputs
        .iter()
        .find(|x| x.path.ends_with(&suffix))
        .map(|x| x.file)
}

struct Score {
    /// Sets of this kind whose originals are all in one stack.
    together: usize,
    /// Sets of this kind with any two originals in one stack.
    any_two: usize,
    sets: usize,
}

fn score(
    truth: &Truth,
    inputs: &StackInputs,
    shot: &HashMap<FileId, FileId>,
    stacks: &Stacks,
    kind: MustNotMergeKind,
) -> Score {
    let index = stacks.index();
    let mut s = Score {
        together: 0,
        any_two: 0,
        sets: 0,
    };
    for m in truth.must_not_merge.iter().filter(|m| m.kind == kind) {
        s.sets += 1;
        let at: Vec<Option<usize>> = m
            .originals
            .iter()
            .map(|p| {
                let f = file_of(inputs, p).unwrap_or_else(|| panic!("{p} catalogued"));
                index.get(&shot[&f]).copied()
            })
            .collect();
        if at[0].is_some() && at.iter().all(|x| *x == at[0]) {
            s.together += 1;
        }
        let mut seen = std::collections::HashSet::new();
        if at.iter().flatten().any(|i| !seen.insert(*i)) {
            s.any_two += 1;
        }
    }
    s
}

fn compute(inputs: &StackInputs, t: StackThresholds) -> Stacks {
    inputs.stacks(t, &BTreeSet::new(), &BTreeSet::new(), &HashMap::new())
}

#[test]
fn scenario_photo_stacks_corpus_bursts_form_stacks() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let inputs = StackInputs::load(&svc).unwrap();
    let shot = shot_of(&svc, &inputs);
    let truth = Truth::read(corpus()).unwrap();

    // The calibration table (ADR-0135): bursts stacked whole, and sets of other kinds with any two stacked.
    println!(
        "similarity window  stacks shots  burst(whole)  minutes_apart(any)  crop(any)  different_camera(any)"
    );
    for similarity in StackSimilarity::ALL {
        for window_s in [1, 2, 5, 10, 30, 60] {
            let t = StackThresholds {
                window_s,
                similarity,
            };
            let st = compute(&inputs, t);
            let b = score(&truth, &inputs, &shot, &st, MustNotMergeKind::Burst);
            let m = score(&truth, &inputs, &shot, &st, MustNotMergeKind::MinutesApart);
            let c = score(&truth, &inputs, &shot, &st, MustNotMergeKind::Crop);
            let d = score(
                &truth,
                &inputs,
                &shot,
                &st,
                MustNotMergeKind::DifferentCamera,
            );
            println!(
                "{:<10} {:>5}s  {:>6} {:>5}  {:>5}/{:<6}  {:>8}/{:<9}  {:>3}/{:<5}  {:>6}/{}",
                similarity.key(),
                window_s,
                st.stacks.len(),
                st.stacked_shots(),
                b.together,
                b.sets,
                m.any_two,
                m.sets,
                c.any_two,
                c.sets,
                d.any_two,
                d.sets
            );
        }
    }

    // The gate, at the defaults: every burst set in one stack, no set of shots minutes apart in one.
    let st = compute(&inputs, StackThresholds::default());
    let b = score(&truth, &inputs, &shot, &st, MustNotMergeKind::Burst);
    let m = score(&truth, &inputs, &shot, &st, MustNotMergeKind::MinutesApart);
    assert!(b.sets >= 10, "the corpus has burst sets");
    assert_eq!(
        b.together, b.sets,
        "every burst set lies wholly within one stack"
    );
    assert_eq!(m.any_two, 0, "no minutes-apart set is stacked");
}

/// Every group (keeper, members) and suggestion, as stored.
fn grouping_snapshot(svc: &ScanService) -> (Vec<(i64, i64, i64)>, Vec<(i64, i64)>) {
    svc.engine()
        .catalog()
        .read()
        .with(|c| {
            let mut g = c.prepare(
                "SELECT g.id, g.keeper_file_id, m.file_id FROM dup_group g JOIN dup_member m ON m.group_id = g.id
                 ORDER BY 1, 3",
            )?;
            let groups = g
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut s = c.prepare("SELECT file_a, file_b FROM dup_suggestion ORDER BY 1, 2")?;
            let sugg = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((groups, sugg))
        })
        .unwrap()
}

#[test]
fn scenario_photo_stacks_stacks_leave_groups_unchanged() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let before = grouping_snapshot(&svc);
    let inputs = StackInputs::load(&svc).unwrap();
    let loosest = compute(
        &inputs,
        StackThresholds {
            window_s: 60,
            similarity: StackSimilarity::Loose,
        },
    );
    assert!(!loosest.stacks.is_empty());
    assert_eq!(
        grouping_snapshot(&svc),
        before,
        "groups, keepers and suggestions unchanged"
    );
}

#[test]
fn scenario_photo_stacks_no_source_read_for_stacks() {
    // A private copy of the corpus, taken away after the scan.
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = tmp.path().join("corpus");
    let w = ps_fixtures::DirWriter::new(&root).unwrap();
    ps_fixtures::generate(42, ps_fixtures::Preset::Small, &w).unwrap();
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let online = compute(
        &StackInputs::load(&svc).unwrap(),
        StackThresholds::default(),
    );
    std::fs::rename(&root, tmp.path().join("unplugged")).unwrap();
    let offline = compute(
        &StackInputs::load(&svc).unwrap(),
        StackThresholds::default(),
    );
    assert!(!online.stacks.is_empty());
    assert_eq!(offline, online, "the same stacks with the source gone");
}
