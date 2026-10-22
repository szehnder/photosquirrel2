//! `cleanup` scenarios at the engine level (change `quarantine-cleanup`): re-verification, same-volume
//! no-replace renames, the two journals, undo and its conflicts. Hand-made sources and libraries in scratch
//! folders under the target dir; never a real photo folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use common::{bytes, mtime_ns, rel, scratch, set_mtime};
use ps_export::quarantine::{
    self, CleanupJournal, JournalEvent, NoQuarantineHooks, QuarantineHooks, RunConfig, RunProgress,
    SkipReason,
};
use ps_model::{
    CleanupMove, ContentId, Disposition, ExportProfile, FileId, LedgerEntry, LedgerKind,
    LedgerState, MoveState, RelPath,
};

/// The catalog half of the journal, in memory: current state per file and the full history.
#[derive(Default)]
struct MemJournal {
    state: BTreeMap<FileId, (MoveState, Option<String>)>,
    history: Vec<(FileId, MoveState)>,
}

impl CleanupJournal for MemJournal {
    fn record(&mut self, file: FileId, state: MoveState, note: Option<String>) -> io::Result<()> {
        self.state.insert(file, (state, note));
        self.history.push((file, state));
        Ok(())
    }
}

impl MemJournal {
    /// `moves` with the journaled states applied (what the catalog would return).
    fn apply(&self, moves: &[CleanupMove]) -> Vec<CleanupMove> {
        moves
            .iter()
            .map(|m| match self.state.get(&m.file) {
                Some((s, n)) => CleanupMove {
                    state: *s,
                    note: n.clone(),
                    ..m.clone()
                },
                None => m.clone(),
            })
            .collect()
    }

    fn of(&self, f: FileId) -> MoveState {
        self.state.get(&f).map_or(MoveState::Planned, |s| s.0)
    }
}

const RUN: i64 = 7;
const UID: &str = "test-run-uid";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    lib: PathBuf,
    ledger: Vec<LedgerEntry>,
    moves: Vec<CleanupMove>,
}

impl Fixture {
    /// One source file per `(path, library path)`; files sharing a library path are covered by it (the
    /// first is its exported keeper; the others are byte-identical duplicates).
    fn new(tag: &str, files: &[(&str, &str)]) -> Fixture {
        let dir = scratch(tag);
        let root = dir.path().join("drive");
        let lib = dir.path().join("library");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&lib).unwrap();
        let mut ledger: BTreeMap<String, LedgerEntry> = BTreeMap::new();
        let mut moves = Vec::new();
        for (i, (path, lib_path)) in files.iter().enumerate() {
            let data = match ledger.get(*lib_path) {
                Some(_) => fs::read(lib.join(lib_path)).unwrap(),
                None => bytes(1000 + i as u64, 20_000 + i * 37),
            };
            let src = rel(path).under(&root);
            fs::create_dir_all(src.parent().unwrap()).unwrap();
            fs::write(&src, &data).unwrap();
            set_mtime(&src, 1_600_000_000 + i as i64);
            let content = ps_hash::identity_from_buffer(&data);
            let disposition = if ledger.contains_key(*lib_path) {
                Disposition::Covered
            } else {
                let lp = lib.join(lib_path);
                fs::create_dir_all(lp.parent().unwrap()).unwrap();
                fs::write(&lp, &data).unwrap();
                ledger.insert(
                    (*lib_path).to_owned(),
                    LedgerEntry {
                        content,
                        full_hash: Some(*blake3::hash(&data).as_bytes()),
                        file: FileId(i as i64 + 1),
                        library_rel_path: rel(lib_path),
                        size: data.len() as u64,
                        state: LedgerState::Done,
                        profile: ExportProfile::Lightroom,
                        at_ms: 0,
                        library_mtime_ns: Some(mtime_ns(&lp)),
                        kind: LedgerKind::File,
                    },
                );
                Disposition::Exported
            };
            let md = fs::metadata(&src).unwrap();
            moves.push(CleanupMove {
                file: FileId(i as i64 + 1),
                rel_path: rel(path),
                quarantine_rel_path: quarantine::quarantine_rel_path(RUN, &rel(path)),
                size: data.len() as u64,
                mtime_ns: mtime_ns(&src),
                file_index: ino(&md),
                content,
                disposition,
                covering_library_path: rel(lib_path),
                state: MoveState::Planned,
                note: None,
                at_ms: None,
            });
        }
        Fixture {
            _dir: dir,
            root,
            lib,
            ledger: ledger.into_values().collect(),
            moves,
        }
    }

    fn cfg(&self) -> RunConfig<'_> {
        RunConfig {
            run: RUN,
            uid: UID,
            source_root: &self.root,
            library_root: &self.lib,
            ledger: &self.ledger,
        }
    }

    fn run(&self, j: &mut MemJournal, hooks: &dyn QuarantineHooks) -> quarantine::RunReport {
        let moves = j.apply(&self.moves);
        quarantine::run(
            &self.cfg(),
            &moves,
            j,
            hooks,
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .expect("run")
    }

    fn undo(
        &self,
        j: &mut MemJournal,
        select: &dyn Fn(&CleanupMove) -> bool,
    ) -> quarantine::UndoReport {
        let moves = j.apply(&self.moves);
        quarantine::undo(&self.cfg(), &moves, select, j).expect("undo")
    }

    fn src(&self, m: &CleanupMove) -> PathBuf {
        m.rel_path.under(&self.root)
    }

    fn q(&self, m: &CleanupMove) -> PathBuf {
        m.quarantine_rel_path.under(&self.root)
    }

    fn journal_path(&self) -> PathBuf {
        quarantine::run_dir_rel(RUN)
            .under(&self.root)
            .join(quarantine::JOURNAL_FILE)
    }
}

fn ino(m: &fs::Metadata) -> Option<u64> {
    ps_export::quarantine::fsops::file_index_of(m)
}

/// Every regular file under `root` except the journal: relative path -> BLAKE3.
fn files_under(root: &Path) -> BTreeMap<PathBuf, [u8; 32]> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n != quarantine::JOURNAL_FILE) {
                out.insert(
                    p.strip_prefix(root).unwrap().to_path_buf(),
                    *blake3::hash(&fs::read(&p).unwrap()).as_bytes(),
                );
            }
        }
    }
    out
}

fn snapshot_outside_quarantine(root: &Path) -> BTreeMap<PathBuf, [u8; 32]> {
    files_under(root)
        .into_iter()
        .filter(|(p, _)| !p.starts_with(quarantine::QUARANTINE_DIR))
        .collect()
}

const THREE: &[(&str, &str)] = &[
    ("DCIM/100/IMG_0001.JPG", "2020/IMG_0001.JPG"),
    ("DCIM/100/IMG_0002.JPG", "2020/IMG_0002.JPG"),
    ("Backup/IMG_0001.JPG", "2020/IMG_0001.JPG"),
];

#[test]
fn scenario_cleanup_move_is_a_same_volume_rename() {
    let f = Fixture::new("q-rename", THREE);
    let before: Vec<_> = f
        .moves
        .iter()
        .map(|m| {
            let md = fs::metadata(f.src(m)).unwrap();
            (ino(&md), fs::read(f.src(m)).unwrap())
        })
        .collect();
    let mut j = MemJournal::default();
    let r = f.run(&mut j, &NoQuarantineHooks);
    assert_eq!(r.moved, 3, "{:?}", r.skipped);
    for (m, (ino_before, bytes_before)) in f.moves.iter().zip(before) {
        assert!(!f.src(m).exists(), "{} left its path", m.rel_path);
        let q = f.q(m);
        assert_eq!(
            q,
            f.root
                .join(".photosquirrel-quarantine")
                .join(RUN.to_string())
                .join(m.rel_path.to_path_buf())
        );
        let md = fs::metadata(&q).unwrap();
        assert_eq!(
            ino(&md),
            ino_before,
            "same file identity: a rename, no copy"
        );
        assert_eq!(fs::read(&q).unwrap(), bytes_before, "unchanged bytes");
        assert_eq!(j.of(m.file), MoveState::Quarantined);
    }
}

#[test]
fn scenario_cleanup_changed_source_skipped() {
    let f = Fixture::new("q-changed", THREE);
    // Same size, other bytes, the same modification time restored: only the re-hash can notice.
    let m0 = &f.moves[0];
    let mut data = fs::read(f.src(m0)).unwrap();
    data[100] ^= 0xff;
    fs::write(f.src(m0), &data).unwrap();
    set_mtime(&f.src(m0), 1_600_000_000);
    assert_eq!(mtime_ns(&f.src(m0)), m0.mtime_ns);
    // Another one grew.
    let m1 = &f.moves[1];
    fs::write(f.src(m1), bytes(5, 30_000)).unwrap();
    let mut j = MemJournal::default();
    let r = f.run(&mut j, &NoQuarantineHooks);
    assert_eq!(r.moved, 1);
    assert_eq!(r.skipped.len(), 2);
    for s in &r.skipped {
        assert!(matches!(s.reason, SkipReason::SourceChanged(_)), "{s:?}");
        assert!(s.text.contains("changed since scanning"), "{}", s.text);
    }
    for m in &f.moves[..2] {
        assert!(f.src(m).is_file(), "not moved");
        assert!(!f.q(m).exists());
        assert_eq!(j.of(m.file), MoveState::Skipped);
        assert!(
            j.state[&m.file]
                .1
                .as_deref()
                .unwrap()
                .contains("changed since scanning")
        );
    }
    assert_eq!(j.of(f.moves[2].file), MoveState::Quarantined);
}

#[test]
fn scenario_cleanup_missing_library_copy_blocks_move() {
    let f = Fixture::new("q-libmissing", THREE);
    // The copy covering IMG_0001 and its backup duplicate is deleted from the library.
    fs::remove_file(f.lib.join("2020/IMG_0001.JPG")).unwrap();
    let mut j = MemJournal::default();
    let r = f.run(&mut j, &NoQuarantineHooks);
    assert_eq!(r.moved, 1);
    let mut skipped: Vec<_> = r.skipped.iter().map(|s| s.rel_path.clone()).collect();
    skipped.sort();
    assert_eq!(
        skipped,
        vec![rel("Backup/IMG_0001.JPG"), rel("DCIM/100/IMG_0001.JPG")]
    );
    for s in &r.skipped {
        assert!(matches!(s.reason, SkipReason::LibraryCopy(_)), "{s:?}");
        assert!(s.text.contains("library copy"), "{}", s.text);
    }
    for m in [&f.moves[0], &f.moves[2]] {
        assert!(f.src(m).is_file());
        assert_eq!(j.of(m.file), MoveState::Skipped);
    }
    // A library copy whose bytes changed (same size) blocks too: verified by hash, not by size.
    let g = Fixture::new("q-libchanged", THREE);
    let lp = g.lib.join("2020/IMG_0002.JPG");
    let mut data = fs::read(&lp).unwrap();
    data[10] ^= 1;
    let t = fs::metadata(&lp).unwrap().modified().unwrap();
    fs::write(&lp, &data).unwrap();
    fs::File::options()
        .write(true)
        .open(&lp)
        .unwrap()
        .set_modified(t)
        .unwrap();
    let mut j = MemJournal::default();
    let r = g.run(&mut j, &NoQuarantineHooks);
    assert_eq!(r.moved, 2);
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].rel_path, rel("DCIM/100/IMG_0002.JPG"));
    assert_eq!(
        r.skipped[0].reason,
        SkipReason::LibraryCopy(ps_export::CopyCheck::HashMismatch)
    );
}

/// Reports the quarantine folder on another device.
struct OtherVolume;

impl QuarantineHooks for OtherVolume {
    fn device(&self, path: &Path) -> io::Result<Option<u64>> {
        let d = quarantine::fsops::device_of(path)?;
        let in_q = path
            .components()
            .any(|c| c.as_os_str() == quarantine::QUARANTINE_DIR);
        Ok(if in_q {
            d.map(|x| x ^ 1).or(Some(1))
        } else {
            d.or(Some(0))
        })
    }
}

#[test]
fn scenario_cleanup_cross_volume_move_refused() {
    let f = Fixture::new("q-xvol", THREE);
    let mut j = MemJournal::default();
    let r = f.run(&mut j, &OtherVolume);
    assert_eq!(r.moved, 0);
    assert_eq!(r.skipped.len(), 3);
    for (s, m) in r.skipped.iter().zip(&f.moves) {
        assert_eq!(s.rel_path, m.rel_path);
        assert_eq!(s.reason, SkipReason::CrossVolume);
        assert!(s.text.contains("another volume"), "{}", s.text);
        assert!(f.src(m).is_file());
        assert!(!f.q(m).exists());
    }
    // A quarantine folder that is a symlink (possibly to another volume) is refused too.
    #[cfg(unix)]
    {
        let g = Fixture::new("q-symlinked", THREE);
        let elsewhere = g.root.parent().unwrap().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, g.root.join(quarantine::QUARANTINE_DIR)).unwrap();
        let mut j = MemJournal::default();
        let r = g.run(&mut j, &NoQuarantineHooks);
        assert_eq!(r.moved, 0);
        assert!(
            r.skipped
                .iter()
                .all(|s| matches!(s.reason, SkipReason::UnsafeFolder(_))),
            "{:?}",
            r.skipped
        );
        assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
    }
}

#[test]
fn scenario_cleanup_journal_records_every_move() {
    let names: Vec<(String, String)> = (0..50)
        .map(|i| {
            (
                format!("DCIM/{:03}/IMG_{i:04}.JPG", i / 10),
                format!("2021/IMG_{i:04}.JPG"),
            )
        })
        .collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let f = Fixture::new("q-journal", &files);
    let mut j = MemJournal::default();
    let r = f.run(&mut j, &NoQuarantineHooks);
    assert_eq!(r.moved, 50);
    // The catalog journal: intent before done for every move.
    for m in &f.moves {
        let h: Vec<MoveState> = j
            .history
            .iter()
            .filter(|(id, _)| *id == m.file)
            .map(|(_, s)| *s)
            .collect();
        assert_eq!(h, vec![MoveState::Moving, MoveState::Quarantined]);
    }
    // The JSON journal: a header, then intent and done with paths and hashes for each of the 50.
    let lines = quarantine::read_journal(&f.journal_path()).unwrap();
    assert_eq!(lines[0].event, JournalEvent::Run);
    assert_eq!(lines[0].uid.as_deref(), Some(UID));
    let done: Vec<_> = lines
        .iter()
        .filter(|l| l.event == JournalEvent::Done)
        .collect();
    assert_eq!(done.len(), 50);
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.event == JournalEvent::Intent)
            .count(),
        50
    );
    for m in &f.moves {
        let l = done
            .iter()
            .find(|l| l.file == Some(m.file.0))
            .expect("done line");
        assert_eq!(l.original.as_deref(), Some(m.rel_path.display().as_ref()));
        assert_eq!(
            l.quarantine.as_deref(),
            Some(m.quarantine_rel_path.display().as_ref())
        );
        assert_eq!(l.content.as_deref(), Some(m.content.to_hex().as_str()));
        assert_eq!(l.size, Some(m.size));
        let intent = lines
            .iter()
            .position(|x| x.event == JournalEvent::Intent && x.file == Some(m.file.0))
            .unwrap();
        let d = lines
            .iter()
            .position(|x| x.event == JournalEvent::Done && x.file == Some(m.file.0))
            .unwrap();
        assert!(intent < d, "write-ahead: the intent precedes the move");
    }
    // Another run's folder with the same number (another catalog) is never used.
    let g = Fixture::new("q-foreign", THREE);
    let run_dir = quarantine::run_dir_rel(RUN).under(&g.root);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join(quarantine::JOURNAL_FILE),
        b"{\"v\":1,\"event\":\"run\",\"run\":7,\"at\":0,\"uid\":\"someone-else\"}\n",
    )
    .unwrap();
    let mut j = MemJournal::default();
    let moves = j.apply(&g.moves);
    let e = quarantine::run(
        &g.cfg(),
        &moves,
        &mut j,
        &NoQuarantineHooks,
        &AtomicBool::new(false),
        &RunProgress::default(),
    )
    .expect_err("foreign run folder");
    assert!(e.to_string().contains("another cleanup run"), "{e}");
    assert!(g.moves.iter().all(|m| g.src(m).is_file()), "nothing moved");
}

#[test]
fn scenario_cleanup_undo_restores_run() {
    let f = Fixture::new("q-undo", THREE);
    let before = snapshot_outside_quarantine(&f.root);
    let mut j = MemJournal::default();
    assert_eq!(f.run(&mut j, &NoQuarantineHooks).moved, 3);
    let r = f.undo(&mut j, &|_| true);
    assert_eq!(r.restored, 3);
    assert!(r.conflicts.is_empty() && r.changed.is_empty() && r.missing.is_empty());
    assert!(r.run_fully_undone);
    assert_eq!(snapshot_outside_quarantine(&f.root), before);
    for m in &f.moves {
        assert_eq!(j.of(m.file), MoveState::Restored);
    }
    // Only the journal is left in the quarantine folder; the folders the run created are gone.
    let left = files_under(&f.root.join(quarantine::QUARANTINE_DIR));
    assert!(left.is_empty(), "{left:?}");
    assert!(f.journal_path().is_file());
    let lines = quarantine::read_journal(&f.journal_path()).unwrap();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.event == JournalEvent::Restored)
            .count(),
        3
    );
}

#[test]
fn scenario_cleanup_partial_undo_by_folder() {
    let f = Fixture::new("q-partial", THREE);
    let mut j = MemJournal::default();
    assert_eq!(f.run(&mut j, &NoQuarantineHooks).moved, 3);
    let dcim = rel("DCIM");
    let r = f.undo(&mut j, &|m: &CleanupMove| m.rel_path.is_within(&dcim));
    assert_eq!(r.restored, 2);
    assert!(!r.run_fully_undone);
    for m in &f.moves {
        if m.rel_path.is_within(&dcim) {
            assert!(f.src(m).is_file());
            assert_eq!(j.of(m.file), MoveState::Restored);
        } else {
            assert!(f.q(m).is_file(), "the rest stays quarantined");
            assert_eq!(j.of(m.file), MoveState::Quarantined);
        }
    }
    // The rest later.
    let r = f.undo(&mut j, &|_| true);
    assert_eq!(r.restored, 1);
    assert!(r.run_fully_undone);
}

#[test]
fn scenario_cleanup_occupied_path_not_overwritten() {
    let f = Fixture::new("q-occupied", THREE);
    let mut j = MemJournal::default();
    assert_eq!(f.run(&mut j, &NoQuarantineHooks).moved, 3);
    let m = &f.moves[1];
    fs::write(f.src(m), b"a new file the user made here").unwrap();
    let r = f.undo(&mut j, &|_| true);
    assert_eq!(r.restored, 2);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].rel_path, m.rel_path);
    assert!(
        r.conflicts[0].text.contains("occupied"),
        "{}",
        r.conflicts[0].text
    );
    assert_eq!(
        fs::read(f.src(m)).unwrap(),
        b"a new file the user made here"
    );
    assert!(f.q(m).is_file(), "the quarantined file stays in quarantine");
    assert_eq!(j.of(m.file), MoveState::Quarantined);
    assert!(!r.run_fully_undone);
    let lines = quarantine::read_journal(&f.journal_path()).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.event == JournalEvent::Conflict && l.file == Some(m.file.0))
    );
    // A quarantined file edited meanwhile is still restored, and flagged.
    let g = Fixture::new("q-edited", THREE);
    let mut j = MemJournal::default();
    assert_eq!(g.run(&mut j, &NoQuarantineHooks).moved, 3);
    fs::write(g.q(&g.moves[0]), b"edited in quarantine").unwrap();
    let r = g.undo(&mut j, &|_| true);
    assert_eq!(r.restored, 3);
    assert_eq!(r.changed.len(), 1);
    assert_eq!(
        fs::read(g.src(&g.moves[0])).unwrap(),
        b"edited in quarantine"
    );
}

#[test]
fn eligibility_and_plan_are_pure() {
    use ps_export::{FileDisposition, FileFacts};
    let facts = |i: i64, p: &str| FileFacts {
        file: FileId(i),
        source: ps_model::SourceId(1),
        rel_path: rel(p),
        size: 10,
        content: Some(ContentId::full([i as u8; 32])),
        class: None,
        unreadable: false,
        missing: false,
        user_excluded: false,
        quarantined: None,
    };
    let disp = |i: i64, d: Disposition| FileDisposition {
        file: FileId(i),
        disposition: d,
        reason: None,
        library_path: matches!(d, Disposition::Exported | Disposition::Covered)
            .then(|| rel("2020/x.jpg")),
        prior: None,
        quarantine_path: None,
    };
    let f: Vec<FileFacts> = vec![
        facts(1, "a/1.jpg"),
        facts(2, "a/2.jpg"),
        facts(3, "b/3.jpg"),
        FileFacts {
            missing: true,
            ..facts(4, "a/4.jpg")
        },
        facts(5, "a/5.jpg"),
        FileFacts {
            quarantined: Some(rel(".photosquirrel-quarantine/1/a/6.jpg")),
            ..facts(6, "a/6.jpg")
        },
    ];
    let d = [
        disp(1, Disposition::Exported),
        disp(2, Disposition::Covered),
        disp(3, Disposition::Covered),
        disp(4, Disposition::Exported),
        disp(5, Disposition::Pending),
        disp(6, Disposition::Quarantined),
    ];
    let stat = ps_export::quarantine::Stat {
        size: 10,
        mtime_ns: 1,
        file_index: None,
    };
    let all = quarantine::plan_moves(3, None, f.iter().zip(&d).map(|(a, b)| (a, b, stat)));
    let ids: Vec<i64> = all.iter().map(|m| m.file.0).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(
        all[0].quarantine_rel_path,
        rel(".photosquirrel-quarantine/3/a/1.jpg")
    );
    let only_a = quarantine::plan_moves(
        3,
        Some(&rel("a")),
        f.iter().zip(&d).map(|(a, b)| (a, b, stat)),
    );
    assert_eq!(only_a.len(), 2);
    let t = quarantine::totals(&all);
    assert_eq!((t.files, t.bytes, t.folders.len()), (3, 30, 2));
    assert_eq!(t.folders[0].covered, 1);
    let _: &RelPath = &all[0].rel_path;
}

impl Fixture {
    fn reconcile(&self, j: &mut MemJournal) -> quarantine::ReconcileReport {
        let moves = j.apply(&self.moves);
        quarantine::reconcile(&self.cfg(), &moves, j).expect("reconcile")
    }
}

/// The catalog half of the journal lost its last commits (power loss with the catalog's commit not yet on
/// disk): the catalog says planned, the file is in quarantine and `journal.jsonl` records the move. Reconcile
/// reads the JSON journal, finds the file and marks it quarantined; undo then restores it.
#[test]
fn scenario_cleanup_lost_catalog_commit_recovered() {
    let f = Fixture::new("q-lost-commit", THREE);
    let before = snapshot_outside_quarantine(&f.root);
    let mut j = MemJournal::default();
    assert_eq!(f.run(&mut j, &NoQuarantineHooks).moved, 3);
    // Lose the catalog commits of the first move (intent and done) and of the second one's done.
    let (a, b) = (f.moves[0].file, f.moves[1].file);
    j.state.insert(a, (MoveState::Planned, None));
    j.state
        .insert(b, (MoveState::Skipped, Some("stale".into())));
    let r = f.reconcile(&mut j);
    let fixed: Vec<FileId> = r.resolved.iter().map(|x| x.0).collect();
    assert_eq!(fixed, vec![a, b]);
    assert_eq!(j.of(a), MoveState::Quarantined);
    assert_eq!(j.of(b), MoveState::Quarantined);
    assert!(f.q(&f.moves[0]).is_file() && !f.src(&f.moves[0]).exists());
    // Nothing was moved by reconciling; a second pass finds nothing to do.
    assert!(f.reconcile(&mut j).resolved.is_empty());
    let u = f.undo(&mut j, &|_| true);
    assert_eq!(u.restored, 3, "{:?}", u.conflicts);
    assert_eq!(snapshot_outside_quarantine(&f.root), before);
    // A lost undo commit: the catalog says quarantined, the file is home and the JSON journal says restored.
    let mut j2 = MemJournal::default();
    let g = Fixture::new("q-lost-undo", THREE);
    assert_eq!(g.run(&mut j2, &NoQuarantineHooks).moved, 3);
    assert_eq!(g.undo(&mut j2, &|_| true).restored, 3);
    let c = g.moves[2].file;
    j2.state.insert(c, (MoveState::Quarantined, None));
    let r = g.reconcile(&mut j2);
    assert_eq!(r.resolved.len(), 1);
    assert_eq!(j2.of(c), MoveState::Restored);
}

/// A file of the right size is not enough: reconcile re-hashes before deciding where a file is. An
/// interrupted undo whose original path now holds another file of the same size (and whose quarantined file
/// is gone) is not marked restored.
#[test]
fn reconcile_compares_content_not_only_size() {
    let f = Fixture::new("q-reconcile-hash", THREE);
    let mut j = MemJournal::default();
    assert_eq!(f.run(&mut j, &NoQuarantineHooks).moved, 3);
    let m = &f.moves[0];
    // Undo intent journaled, then the crash; meanwhile the quarantined file was moved away by someone and a
    // same-size impostor sits at the original path.
    j.state.insert(m.file, (MoveState::Restoring, None));
    fs::rename(f.q(m), f._dir.path().join("elsewhere")).unwrap();
    let mut impostor = fs::read(f._dir.path().join("elsewhere")).unwrap();
    impostor[0] ^= 0xff;
    fs::write(f.src(m), &impostor).unwrap();
    f.reconcile(&mut j);
    assert_eq!(
        j.of(m.file),
        MoveState::Quarantined,
        "never restored onto another file's bytes"
    );
    assert_eq!(
        fs::read(f.src(m)).unwrap(),
        impostor,
        "the impostor is untouched"
    );
    // An interrupted move whose quarantine path holds a same-size impostor while the file is still home:
    // the rename never happened.
    let n = &f.moves[1];
    fs::rename(f.q(n), f.src(n)).unwrap();
    let mut other = fs::read(f.src(n)).unwrap();
    other[1] ^= 0xff;
    fs::write(f.q(n), &other).unwrap();
    j.state.insert(n.file, (MoveState::Moving, None));
    f.reconcile(&mut j);
    assert_eq!(j.of(n.file), MoveState::Planned);
}
