//! `cleanup` scenarios through the service (change `quarantine-cleanup`): eligibility from the derived
//! dispositions, the stored plan, the never-delete invariant, crash reconciliation in a killed child process,
//! and rescans of a drive with a quarantine folder. Every test works on its own copy of the ps-fixtures small
//! corpus in a scratch folder under the target dir (the shared corpus is never changed).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ps_export::ExecConfig;
use ps_export::quarantine::{QUARANTINE_DIR, QuarantineHooks, RunProgress};
use ps_model::{CleanupMove, CleanupRunState, Disposition, FileId, MoveState, SourceId};
use ps_service::cleanup::{RunOptions, UndoSelection};
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn shared_small() -> PathBuf {
    ps_fixtures::shared_corpus(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "cleanup-corpus-small-42",
        42,
        ps_fixtures::Preset::Small,
    )
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&p, &t);
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}

/// A private copy of the small corpus, scanned and exported (lightroom profile) into a library beside it.
struct Env {
    tmp: tempfile::TempDir,
    corpus: PathBuf,
    library: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Env {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(base).unwrap();
        let tmp = tempfile::Builder::new()
            .prefix(&format!("ps-cleanup-{tag}-"))
            .tempdir_in(base)
            .unwrap();
        let corpus = tmp.path().join("corpus");
        copy_tree(&shared_small(), &corpus);
        let corpus = corpus
            .canonicalize()
            .map(ps_model::path::simplify_canonical)
            .unwrap();
        let env = Env {
            library: tmp.path().join("Library"),
            corpus,
            tmp,
        };
        let svc = env.open();
        for d in ["drive-a", "drive-b", "drive-c"] {
            svc.add_source(&env.corpus.join(d)).unwrap();
        }
        svc.start(&[]).unwrap();
        svc.wait().unwrap();
        let (_, report) = svc
            .exports()
            .run(
                &env.library,
                profile_settings("lightroom").unwrap(),
                &ExecConfig::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        env
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    fn open(&self) -> ScanService {
        ScanService::open(&self.data(), ScanOptions::default()).unwrap()
    }

    fn drive(&self, id: i64) -> PathBuf {
        self.corpus
            .join(["drive-a", "drive-b", "drive-c"][id as usize - 1])
    }
}

/// Every file under `root` outside its quarantine folder: relative path -> (size, BLAKE3).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, [u8; 32])> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if rel.starts_with(QUARANTINE_DIR) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                let b = fs::read(&p).unwrap();
                out.insert(rel, (b.len() as u64, *blake3::hash(&b).as_bytes()));
            }
        }
    }
    out
}

fn dispositions(
    svc: &ScanService,
    env: &Env,
) -> BTreeMap<FileId, (SourceId, Disposition, Option<Disposition>)> {
    let (files, d, _) = svc.exports().dispositions(&env.library, false).unwrap();
    files
        .iter()
        .zip(d)
        .map(|(f, x)| (f.file, (f.source, x.disposition, x.prior)))
        .collect()
}

fn run_all(svc: &ScanService, id: i64) -> ps_export::quarantine::RunReport {
    svc.cleanups()
        .run(
            id,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap()
}

#[test]
fn quarantine_folder_name_matches_the_scanner() {
    assert_eq!(QUARANTINE_DIR, ps_scan::walk::QUARANTINE_DIR);
}

#[test]
fn scenario_cleanup_plan_before_move() {
    let env = Env::new("plan");
    let svc = env.open();
    let before = snapshot(&env.drive(2));
    let disp = dispositions(&svc, &env);
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    let summary = plan.summary.as_ref().expect("a stored plan");
    assert_eq!(summary.run.state, CleanupRunState::Planned);
    // Every exported and covered file of the drive, each with its quarantine path.
    let want: BTreeSet<FileId> = disp
        .iter()
        .filter(|(_, (s, d, _))| *s == SourceId(2) && d.is_accounted_safe())
        .map(|(f, _)| *f)
        .collect();
    let got: BTreeSet<FileId> = plan.moves.iter().map(|m| m.file).collect();
    assert!(!want.is_empty());
    assert_eq!(got, want);
    for m in &plan.moves {
        assert_eq!(
            m.quarantine_rel_path.to_path_buf(),
            Path::new(QUARANTINE_DIR)
                .join(summary.run.id.to_string())
                .join(m.rel_path.to_path_buf())
        );
        assert_eq!(m.state, MoveState::Planned);
    }
    assert_eq!(plan.totals.files, plan.moves.len() as u64);
    assert_eq!(
        plan.totals.bytes,
        plan.moves.iter().map(|m| m.size).sum::<u64>()
    );
    assert!(!plan.totals.folders.is_empty());
    // The drive's tree is unchanged: no file moved, no quarantine folder made.
    assert_eq!(snapshot(&env.drive(2)), before);
    assert!(!env.drive(2).join(QUARANTINE_DIR).exists());
    // The plan is stored and listed.
    let runs = svc.cleanups().runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].counts.planned, plan.moves.len() as u64);
}

#[test]
fn scenario_cleanup_pending_files_never_eligible() {
    let env = Env::new("pending");
    let svc = env.open();
    // Make one keeper unexported again: its library copy disappears, so it and its group are pending.
    let (files, d, _) = svc.exports().dispositions(&env.library, false).unwrap();
    let groups = svc.engine().catalog().read().group_memberships().unwrap();
    let on_b: HashSet<FileId> = files
        .iter()
        .filter(|f| f.source == SourceId(2))
        .map(|f| f.file)
        .collect();
    let (members, keeper_lib) = groups
        .iter()
        .find_map(|(_, members, set)| {
            let k = set.first()?;
            let (_, x) = files.iter().zip(&d).find(|(f, _)| f.file == *k)?;
            (set.len() == 1
                && members.iter().any(|m| on_b.contains(m))
                && x.disposition == Disposition::Exported)
                .then(|| (members.clone(), x.library_path.clone().unwrap()))
        })
        .expect("a group with a member on drive-b");
    fs::rename(
        keeper_lib.under(&env.library),
        env.tmp.path().join("set-aside"),
    )
    .unwrap();
    let disp = dispositions(&svc, &env);
    for m in &members {
        assert_eq!(disp[m].1, Disposition::Pending, "{m:?}");
    }
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    let planned: HashSet<FileId> = plan.moves.iter().map(|m| m.file).collect();
    assert!(!planned.is_empty());
    for m in &members {
        assert!(!planned.contains(m), "pending {m:?} is never eligible");
    }
    assert!(
        plan.moves
            .iter()
            .all(|m| disp[&m.file].1.is_accounted_safe())
    );
}

#[test]
fn scenario_cleanup_excluded_files_not_eligible() {
    let env = Env::new("excluded");
    let svc = env.open();
    let disp = dispositions(&svc, &env);
    let mut checked = 0;
    for s in 1..=3 {
        let excluded: HashSet<FileId> = disp
            .iter()
            .filter(|(_, (src, d, _))| *src == SourceId(s) && *d == Disposition::Excluded)
            .map(|(f, _)| *f)
            .collect();
        let plan = svc.cleanups().plan(SourceId(s), None, None).unwrap();
        assert!(plan.moves.iter().all(|m| !excluded.contains(&m.file)));
        checked += excluded.len();
    }
    assert!(checked > 0, "the corpus has junk and graphics");
}

#[test]
fn scenario_cleanup_cleanup_never_deletes() {
    let env = Env::new("never-deletes");
    let svc = env.open();
    let root = env.drive(2);
    let before = snapshot(&root);
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    let id = plan.summary.as_ref().unwrap().run.id;
    let r = run_all(&svc, id);
    assert_eq!(r.moved, plan.moves.len() as u64, "{:?}", r.skipped);
    // Partial undo: the first folder only.
    let folder = plan.moves[0].rel_path.parent().unwrap();
    let u = svc
        .cleanups()
        .undo(
            id,
            &UndoSelection {
                folder: Some(folder.clone()),
                files: Vec::new(),
            },
        )
        .unwrap();
    assert!(u.restored > 0 && u.conflicts.is_empty());
    let (_, moves) = svc.cleanups().run_detail(id).unwrap();
    let qdir = root.join(QUARANTINE_DIR).join(id.to_string());
    let mut in_quarantine = 0;
    for (path, (size, hash)) in &before {
        let here = root.join(path);
        let there = qdir.join(path);
        let found = [here, there]
            .into_iter()
            .filter(|p| p.is_file())
            .map(|p| {
                let b = fs::read(&p).unwrap();
                (b.len() as u64, *blake3::hash(&b).as_bytes(), p)
            })
            .collect::<Vec<_>>();
        assert_eq!(found.len(), 1, "{} exists exactly once", path.display());
        assert_eq!((found[0].0, found[0].1), (*size, *hash), "unchanged bytes");
        if found[0].2.starts_with(&qdir) {
            in_quarantine += 1;
        }
    }
    let still = moves
        .iter()
        .filter(|m| m.state == MoveState::Quarantined)
        .count();
    assert_eq!(in_quarantine, still);
    assert!(still > 0 && still < moves.len());
    // The total number of user files in the drive plus its quarantine folder is unchanged.
    let total = snapshot(&root).len() + in_quarantine;
    assert_eq!(total, before.len());
}

#[test]
fn scenario_cleanup_quarantine_ignored_by_scanner() {
    let env = Env::new("rescan");
    let svc = env.open();
    let plan = svc.cleanups().plan(SourceId(1), None, None).unwrap();
    let id = plan.summary.as_ref().unwrap().run.id;
    let before_disp = dispositions(&svc, &env);
    assert_eq!(run_all(&svc, id).moved, plan.moves.len() as u64);
    let files_before = count_files(&svc);
    svc.start(&[SourceId(1)]).unwrap();
    let rescan = svc.wait().unwrap();
    // The rescan counts the moved files as quarantined, not missing.
    assert_eq!(rescan.diff.missing, 0, "{:?}", rescan.diff);
    assert_eq!(rescan.diff.quarantined, plan.moves.len() as u64);
    // Nothing under the quarantine folder is catalogued; the moved files are quarantined, not missing.
    let mut paths = Vec::new();
    svc.engine()
        .catalog()
        .read()
        .for_each_file(|f| paths.push(f.key.path.display().into_owned()))
        .unwrap();
    assert!(paths.iter().all(|p| !p.contains(QUARANTINE_DIR)));
    assert_eq!(count_files(&svc), files_before, "no new files");
    let disp = dispositions(&svc, &env);
    for m in &plan.moves {
        let (_, d, prior) = disp[&m.file];
        assert_eq!(d, Disposition::Quarantined, "{}", m.rel_path);
        // Quarantined files keep their place in their groups (ADR-0034): the prior disposition is unchanged.
        assert_eq!(Some(before_disp[&m.file].1), prior, "{}", m.rel_path);
    }
    let moved: HashSet<FileId> = plan.moves.iter().map(|m| m.file).collect();
    for (f, v) in &before_disp {
        if !moved.contains(f) {
            assert_eq!(disp[f], *v, "{f:?} is unaffected by the cleanup");
        }
    }
    assert!(!disp.values().any(|(_, d, _)| *d == Disposition::Missing));
    let c = svc.exports().completion(None, false).unwrap();
    let s1 = c
        .report
        .sources
        .iter()
        .find(|s| s.source == SourceId(1))
        .unwrap();
    assert_eq!(
        s1.counts.files_of(Disposition::Quarantined),
        plan.moves.len() as u64
    );
    assert!(s1.counts.fully_accounted());
    // Undo, rescan: every file is back with the disposition it had.
    let u = svc.cleanups().undo(id, &UndoSelection::all()).unwrap();
    assert!(u.run_fully_undone);
    svc.start(&[SourceId(1)]).unwrap();
    svc.wait().unwrap();
    assert_eq!(dispositions(&svc, &env), before_disp);
    let run = svc.cleanups().run_detail(id).unwrap().0;
    assert_eq!(run.run.state, CleanupRunState::Undone);
}

fn count_files(svc: &ScanService) -> usize {
    let mut n = 0;
    svc.engine()
        .catalog()
        .read()
        .for_each_file(|_| n += 1)
        .unwrap();
    n
}

// ---- kill mid-run, then reconcile and undo ----

const KILL_DATA: &str = "PS_CLEANUP_KILL_DATA";
const KILL_RUN: &str = "PS_CLEANUP_KILL_RUN";
const KILL_AFTER: usize = 10;

/// Kills the process right after the 10th rename, before `done` is journaled.
struct KillAfterRename(AtomicUsize);

impl QuarantineHooks for KillAfterRename {
    fn after_rename(&self, _m: &CleanupMove) {
        if self.0.fetch_add(1, Ordering::SeqCst) + 1 == KILL_AFTER {
            std::process::abort();
        }
    }
}

#[test]
fn scenario_cleanup_crash_mid_run_is_recoverable() {
    if let (Some(data), Some(run)) = (std::env::var_os(KILL_DATA), std::env::var_os(KILL_RUN)) {
        // Child: run the cleanup until killed.
        let svc = ScanService::open(Path::new(&data), ScanOptions::default()).unwrap();
        let id: i64 = run.to_string_lossy().parse().unwrap();
        let _ = svc.cleanups().run_with_hooks(
            id,
            RunOptions::default(),
            &KillAfterRename(AtomicUsize::new(0)),
            &AtomicBool::new(false),
            &RunProgress::default(),
        );
        unreachable!("the child is killed mid-run");
    }
    let env = Env::new("crash");
    let root = env.drive(2);
    let before = snapshot(&root);
    let id = {
        let svc = env.open();
        let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
        assert!(plan.moves.len() > KILL_AFTER);
        plan.summary.unwrap().run.id
    };
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "scenario_cleanup_crash_mid_run_is_recoverable",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KILL_DATA, env.data())
        .env(KILL_RUN, id.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "the child was killed");

    // Restart: the run was left running with one move mid-way.
    let svc = env.open();
    let raw = svc.engine().catalog().read().cleanup_moves(id).unwrap();
    assert_eq!(
        raw.iter().filter(|m| m.state == MoveState::Moving).count(),
        1
    );
    assert_eq!(
        raw.iter()
            .filter(|m| m.state == MoveState::Quarantined)
            .count(),
        KILL_AFTER - 1
    );
    let reconciled = svc.cleanups().reconcile().unwrap();
    assert_eq!(reconciled.len(), 1);
    let (summary, moves) = svc.cleanups().run_detail(id).unwrap();
    assert_eq!(summary.run.state, CleanupRunState::Completed);
    // Every journaled move is reconciled from the file system.
    for m in &moves {
        let here = m.rel_path.under(&root).is_file();
        let there = m.quarantine_rel_path.under(&root).is_file();
        match m.state {
            MoveState::Quarantined => assert!(!here && there, "{}", m.rel_path),
            MoveState::Planned => assert!(here && !there, "{}", m.rel_path),
            s => panic!("{}: unexpected {s:?}", m.rel_path),
        }
    }
    assert_eq!(summary.counts.quarantined, KILL_AFTER as u64);
    // The JSON journal records the reconciled move as done too.
    let lines = ps_export::quarantine::read_journal(
        &summary
            .quarantine_dir
            .join(ps_export::quarantine::JOURNAL_FILE),
    )
    .unwrap();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.event == ps_export::quarantine::JournalEvent::Done)
            .count(),
        KILL_AFTER
    );
    // Undo restores every moved file; the drive is as it was.
    let u = svc.cleanups().undo(id, &UndoSelection::all()).unwrap();
    assert_eq!(u.restored, KILL_AFTER as u64);
    assert!(u.run_fully_undone);
    assert_eq!(snapshot(&root), before);
}

// ---- gate-review fixes (ADR-0062 to ADR-0066) ----

/// Plan and run a cleanup of `source`; every planned file moved.
fn clean(svc: &ScanService, source: i64) -> (i64, Vec<CleanupMove>) {
    let plan = svc.cleanups().plan(SourceId(source), None, None).unwrap();
    let id = plan.summary.as_ref().unwrap().run.id;
    let r = run_all(svc, id);
    assert_eq!(r.moved, plan.moves.len() as u64, "{:?}", r.skipped);
    (id, plan.moves)
}

/// A move whose file is its own exported keeper (its library copy is the file's content).
fn exported_move(moves: &[CleanupMove]) -> &CleanupMove {
    moves
        .iter()
        .find(|m| m.disposition == Disposition::Exported)
        .expect("an exported keeper among the moves")
}

/// Clean the first drive that has exported keepers (not only covered copies); its source id and moves.
fn clean_a_drive_with_keepers(svc: &ScanService) -> (i64, Vec<CleanupMove>) {
    let source = (1..=3)
        .find(|s| {
            let plan = svc.cleanups().plan(SourceId(*s), None, None).unwrap();
            plan.moves
                .iter()
                .any(|m| m.disposition == Disposition::Exported)
        })
        .expect("a drive with exported keepers");
    let (_, moves) = clean(svc, source);
    (source, moves)
}

#[test]
fn scenario_cleanup_quarantined_file_without_library_copy_not_accounted() {
    let env = Env::new("q-uncovered");
    let svc = env.open();
    let (src, moves) = clean_a_drive_with_keepers(&svc);
    svc.start(&[SourceId(src)]).unwrap();
    svc.wait().unwrap();
    let source_report = |verify: bool| {
        svc.exports()
            .completion(None, verify)
            .unwrap()
            .report
            .sources
            .into_iter()
            .find(|s| s.source == SourceId(src))
            .unwrap()
    };
    assert!(source_report(true).counts.fully_accounted());
    // The user deletes the library copy that covers a quarantined keeper.
    let m = exported_move(&moves);
    fs::remove_file(m.covering_library_path.under(&env.library)).unwrap();
    for verify in [false, true] {
        let s = source_report(verify);
        assert!(
            !s.counts.fully_accounted(),
            "verify={verify}: {:?}",
            s.counts
        );
        assert!(s.counts.pending_in_quarantine >= 1);
        assert!(
            s.badge.label().contains("only in the quarantine folder"),
            "{}",
            s.badge.label()
        );
    }
    // The file says why: pending, with its quarantine path.
    let (files, d, _) = svc.exports().dispositions(&env.library, false).unwrap();
    let (_, x) = files
        .iter()
        .zip(&d)
        .find(|(f, _)| f.file == m.file)
        .unwrap();
    assert_eq!(x.disposition, Disposition::Pending);
    assert_eq!(x.quarantine_path.as_ref(), Some(&m.quarantine_rel_path));
    let why = ps_service::library::disposition_reason(
        x.disposition,
        x.reason,
        x.library_path.as_ref(),
        x.quarantine_path.as_ref(),
    );
    assert!(
        why.contains("quarantine") && why.contains("library copy is gone"),
        "{why}"
    );
    // Nothing on the drive changed: the file is still in quarantine.
    assert!(m.quarantine_rel_path.under(&env.drive(src)).is_file());
}

#[test]
fn scenario_cleanup_export_reads_quarantined_keeper() {
    let env = Env::new("q-reexport");
    let svc = env.open();
    let (src, moves) = clean_a_drive_with_keepers(&svc);
    let m = exported_move(&moves).clone();
    let q = m.quarantine_rel_path.under(&env.drive(src));
    let before = fs::read(&q).unwrap();
    let q_mtime = fs::metadata(&q).unwrap().modified().unwrap();
    let lib_copy = m.covering_library_path.under(&env.library);
    fs::remove_file(&lib_copy).unwrap();
    let (_, report) = svc
        .exports()
        .run(
            &env.library,
            profile_settings("lightroom").unwrap(),
            &ExecConfig::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    // Copied again from the quarantine folder, which it never left; read-only.
    assert!(!m.rel_path.under(&env.drive(src)).exists());
    assert_eq!(fs::read(&q).unwrap(), before);
    assert_eq!(fs::metadata(&q).unwrap().modified().unwrap(), q_mtime);
    let c = svc.exports().completion(None, true).unwrap();
    let s2 = c
        .report
        .sources
        .iter()
        .find(|s| s.source == SourceId(src))
        .unwrap();
    assert!(s2.counts.fully_accounted(), "{:?}", s2.counts);
    assert_eq!(s2.counts.pending_in_quarantine, 0);
    let (files, d, _) = svc.exports().dispositions(&env.library, true).unwrap();
    let (_, x) = files
        .iter()
        .zip(&d)
        .find(|(f, _)| f.file == m.file)
        .unwrap();
    assert_eq!(x.disposition, Disposition::Quarantined);
    let copy = x.library_path.as_ref().unwrap().under(&env.library);
    assert_eq!(
        fs::read(copy).unwrap(),
        before,
        "the new library copy is the photo"
    );
}

#[test]
fn scenario_cleanup_source_with_quarantined_files_not_removed() {
    let env = Env::new("q-remove");
    let svc = env.open();
    let (id, moves) = clean(&svc, 3);
    let e = svc
        .remove_source(SourceId(3), ps_service::scan::Confirmation::Confirmed)
        .unwrap_err();
    let text = e.to_string();
    assert!(
        text.contains("quarantine") && text.contains("undo"),
        "{text}"
    );
    assert!(svc.sources().unwrap().iter().any(|s| s.id == SourceId(3)));
    // Adding a folder that contains the drive (which would fold it) is refused too.
    let e = svc.add_source(&env.corpus).unwrap_err();
    assert!(e.to_string().contains("quarantine"), "{e}");
    // After a full undo the source can be removed, and its id is never handed out again: the run keeps
    // pointing at drive-c.
    assert!(
        svc.cleanups()
            .undo(id, &UndoSelection::all())
            .unwrap()
            .run_fully_undone
    );
    svc.remove_source(SourceId(3), ps_service::scan::Confirmation::Confirmed)
        .unwrap();
    let other = env.tmp.path().join("another-folder");
    fs::create_dir_all(other.join("DCIM")).unwrap();
    fs::copy(
        moves[0].rel_path.under(&env.drive(3)),
        other.join("DCIM/x.jpg"),
    )
    .unwrap();
    let added = svc.add_source(&other).unwrap();
    assert_ne!(added.source.id, SourceId(3));
    let runs = svc.cleanups().runs().unwrap();
    let r = runs.iter().find(|r| r.run.id == id).unwrap();
    assert_eq!(
        r.root,
        env.drive(3),
        "the run's own root, not the new folder"
    );
}

/// A run planned on a source that was removed (and whose id another source could carry) never runs on
/// another folder.
#[test]
fn scenario_cleanup_run_of_removed_source_refused() {
    let env = Env::new("q-removed-run");
    let svc = env.open();
    let plan = svc.cleanups().plan(SourceId(3), None, None).unwrap();
    let id = plan.summary.as_ref().unwrap().run.id;
    let before = snapshot(&env.drive(3));
    svc.remove_source(SourceId(3), ps_service::scan::Confirmation::Confirmed)
        .unwrap();
    let e = svc
        .cleanups()
        .run(
            id,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap_err();
    assert!(e.to_string().contains("removed"), "{e}");
    assert_eq!(snapshot(&env.drive(3)), before);
}

/// While a cleanup run is live (inside its rename step), a scan, an export, a source change, another cleanup
/// command and reconciliation are refused or skipped; the live run is never reconciled under it.
struct Interfere<'a> {
    svc: &'a ScanService,
    library: PathBuf,
    checked: AtomicUsize,
}

impl QuarantineHooks for Interfere<'_> {
    fn before_rename(&self, m: &CleanupMove) {
        if self.checked.fetch_add(1, Ordering::SeqCst) > 0 {
            return;
        }
        let busy = |e: String| assert!(e.contains("cleanup run"), "{e}");
        busy(self.svc.start(&[]).unwrap_err().to_string());
        busy(
            self.svc
                .exports()
                .run(
                    &self.library,
                    profile_settings("lightroom").unwrap(),
                    &ExecConfig::default(),
                    &AtomicBool::new(false),
                )
                .unwrap_err()
                .to_string(),
        );
        busy(
            self.svc
                .remove_source(SourceId(1), ps_service::scan::Confirmation::Confirmed)
                .unwrap_err()
                .to_string(),
        );
        busy(
            self.svc
                .cleanups()
                .plan(SourceId(1), None, None)
                .unwrap_err()
                .to_string(),
        );
        // Reconciliation skips: the move in flight stays `moving` (its intent is journaled; the rename is next).
        assert!(self.svc.cleanups().reconcile().unwrap().is_empty());
        let (run, _) = self.svc.cleanups().runs().unwrap()[0]
            .clone()
            .run_and_root();
        let moves = self
            .svc
            .engine()
            .catalog()
            .read()
            .cleanup_moves(run)
            .unwrap();
        let mine = moves.iter().find(|x| x.file == m.file).unwrap();
        assert_eq!(mine.state, MoveState::Moving);
    }
}

trait RunAndRoot {
    fn run_and_root(self) -> (i64, PathBuf);
}

impl RunAndRoot for ps_service::cleanup::RunSummary {
    fn run_and_root(self) -> (i64, PathBuf) {
        (self.run.id, self.root)
    }
}

#[test]
fn scenario_cleanup_operations_exclude_each_other() {
    let env = Env::new("q-exclusive");
    let svc = env.open();
    let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
    let id = plan.summary.as_ref().unwrap().run.id;
    let hooks = Interfere {
        svc: &svc,
        library: env.library.clone(),
        checked: AtomicUsize::new(0),
    };
    let r = svc
        .cleanups()
        .run_with_hooks(
            id,
            RunOptions::default(),
            &hooks,
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    assert!(hooks.checked.load(Ordering::SeqCst) > 0, "the hook ran");
    assert_eq!(r.moved, plan.moves.len() as u64, "{:?}", r.skipped);
    // Once the run is over, everything works again.
    assert_eq!(svc.current_operation(), None);
    assert!(svc.cleanups().reconcile().unwrap().is_empty());
    let u = svc.cleanups().undo(id, &UndoSelection::all()).unwrap();
    assert!(u.run_fully_undone);
}
