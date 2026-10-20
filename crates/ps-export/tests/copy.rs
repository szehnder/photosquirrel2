//! Copier and safety scenarios: verified copies, never overwriting, mismatches, sampled identities, sources
//! untouched, nothing written outside the library, resume after a kill, disk full, injected corruption.

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::*;
use ps_export::{
    CopyHooks, ExecConfig, ExportInput, FsLibrary, Ledger, MemoryLedger, NoHooks, ProfileSettings,
    StopReason, execute, plan,
};
use ps_model::{ContentIdKind, ExportProfile, FileId, LedgerState};

fn google() -> ProfileSettings {
    ProfileSettings::defaults(ExportProfile::GooglePhotos)
}

fn run(
    input: &ExportInput,
    lib: &Path,
    ledger: &mut dyn Ledger,
    cfg: &ExecConfig,
    hooks: &dyn CopyHooks,
) -> ps_export::ExportReport {
    let rows = ledger.entries().expect("ledger");
    let p = plan(input, &rows, lib, &FsLibrary::new(lib), google()).expect("plan");
    execute(&p, input, ledger, cfg, hooks, &AtomicBool::new(false)).expect("export")
}

#[test]
fn scenario_export_verified_copy_renamed() {
    let mut s = Sources::new("verified", 1);
    let data = bytes(20, 3_000_000);
    let f = s.add(0, "DCIM/IMG_1.JPG", &data, 1_300_000_123, day(2013, 3, 3));
    let input = s.input(vec![unit(vec![f.clone()])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let cfg = ExecConfig {
        copy: ps_export::CopyConfig {
            buffer_bytes: 256 << 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let r = run(&input, &lib, &mut ledger, &cfg, &NoHooks);
    assert_eq!(r.copied, 1);
    let out = lib.join("2013/2013-03-03/IMG_1.JPG");
    assert_eq!(
        *blake3::hash(&fs::read(&out).expect("copy")).as_bytes(),
        f.content.digest
    );
    assert_eq!(mtime_ns(&out), f.mtime_ns, "modification time preserved");
    assert!(partials_under(&lib).is_empty());
    let e = ledger.get(&rel("2013/2013-03-03/IMG_1.JPG")).expect("row");
    assert_eq!(e.state, LedgerState::Done);
    assert_eq!(e.full_hash, Some(f.content.digest));
    if cfg!(target_os = "linux") {
        assert_eq!(r.renames.get(&ps_export::RenameMethod::NoReplace), Some(&1));
    }
}

#[test]
fn scenario_export_existing_file_never_overwritten() {
    let mut s = Sources::new("nooverwrite", 1);
    let f = s.add(0, "IMG_5.JPG", &bytes(21, 5000), 0, day(2014, 4, 4));
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let existing = lib.join("2014/2014-04-04/IMG_5.JPG");
    fs::create_dir_all(existing.parent().expect("p")).expect("mkdir");
    fs::write(&existing, b"unrelated file").expect("existing");
    set_mtime(&existing, 777);
    let before = (fs::read(&existing).expect("r"), mtime_ns(&existing));
    let mut ledger = MemoryLedger::new();
    let r = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!(r.copied, 1);
    assert_eq!(
        (fs::read(&existing).expect("r"), mtime_ns(&existing)),
        before
    );
    assert_eq!(
        fs::read(lib.join("2014/2014-04-04/IMG_5 (2).JPG")).expect("copy"),
        bytes(21, 5000)
    );
}

/// A file appears at the target after planning (a race): the no-replace rename refuses and it survives.
struct Racer(PathBuf);

impl CopyHooks for Racer {
    fn before_rename(&self, _file: FileId, _partial: &Path) {
        fs::write(&self.0, b"appeared meanwhile").expect("race");
    }
}

#[test]
fn target_appearing_during_copy_is_never_overwritten() {
    let mut s = Sources::new("race", 1);
    let f = s.add(0, "IMG_9.JPG", &bytes(22, 4000), 0, day(2014, 4, 5));
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let target = lib.join("2014/2014-04-05/IMG_9.JPG");
    let mut ledger = MemoryLedger::new();
    let r = run(
        &input,
        &lib,
        &mut ledger,
        &ExecConfig::default(),
        &Racer(target.clone()),
    );
    assert_eq!(r.copied, 0);
    assert_eq!(r.failed.len(), 1);
    assert!(
        r.failed[0].error.contains("never overwritten"),
        "{}",
        r.failed[0].error
    );
    assert_eq!(fs::read(&target).expect("r"), b"appeared meanwhile");
    assert!(partials_under(&lib).is_empty());
}

#[test]
fn scenario_export_hash_mismatch_aborts_file() {
    let mut s = Sources::new("mismatch", 1);
    let a = s.add(0, "a.JPG", &bytes(23, 4000), 0, day(2015, 1, 1));
    let b = s.add(0, "b.JPG", &bytes(24, 4000), 0, day(2015, 1, 1));
    let c = s.add(0, "c.JPG", &bytes(25, 4000), 0, day(2015, 1, 1));
    // b is altered after scanning (same size, different bytes).
    let bp = b.rel_path.under(&s.sources[0].root);
    fs::write(&bp, bytes(99, 4000)).expect("alter");
    let input = s.input(vec![unit(vec![a]), unit(vec![b]), unit(vec![c])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!(r.copied, 2, "the other files are exported");
    assert_eq!(r.failed.len(), 1);
    assert!(
        r.failed[0].error.contains("hash mismatch"),
        "{}",
        r.failed[0].error
    );
    assert!(!lib.join("2015/2015-01-01/b.JPG").exists());
    assert!(
        lib.join("2015/2015-01-01/a.JPG").is_file() && lib.join("2015/2015-01-01/c.JPG").is_file()
    );
    assert!(partials_under(&lib).is_empty());
    assert_eq!(
        ledger
            .get(&rel("2015/2015-01-01/b.JPG"))
            .expect("row")
            .state,
        LedgerState::Failed
    );
}

/// Corrupts one byte after hashing, before writing (a bad write path).
struct Corrupt;

impl CopyHooks for Corrupt {
    fn before_write(&self, _f: FileId, offset: u64, chunk: &mut [u8]) -> io::Result<()> {
        if offset == 0 {
            chunk[10] ^= 0xFF;
        }
        Ok(())
    }
}

#[test]
fn corrupted_copy_is_detected_and_not_renamed() {
    let mut s = Sources::new("corrupt", 1);
    let f = s.add(0, "IMG_C.JPG", &bytes(26, 10_000), 0, day(2015, 2, 2));
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(&input, &lib, &mut ledger, &ExecConfig::default(), &Corrupt);
    assert_eq!(r.copied, 0);
    assert!(
        r.failed[0].error.contains("differs"),
        "{}",
        r.failed[0].error
    );
    assert!(!lib.join("2015/2015-02-02/IMG_C.JPG").exists());
    assert!(partials_under(&lib).is_empty());
}

#[test]
fn scenario_export_large_video_verified_fully() {
    let mut s = Sources::new("video", 1);
    let len = 300 * 1_000_000;
    let data = bytes(27, len);
    let f = s.add(0, "Movies/MVI_0001.MOV", &data, 0, day(2018, 8, 8));
    assert_eq!(f.content.kind, ContentIdKind::Sampled);
    let full = *blake3::hash(&data).as_bytes();
    drop(data);
    let input = s.input(vec![unit(vec![f.clone()])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!(r.copied, 1, "{:?}", r.failed);
    let e = ledger
        .get(&rel("2018/2018-08-08/MVI_0001.MOV"))
        .expect("row");
    assert_eq!(
        e.content, f.content,
        "sampled identity re-checked and recorded"
    );
    assert_eq!(
        e.full_hash,
        Some(full),
        "full hash computed during the copy"
    );
    let out = lib.join("2018/2018-08-08/MVI_0001.MOV");
    let (h, n) =
        ps_export::library::full_hash_of(fs::File::open(&out).expect("open")).expect("hash");
    assert_eq!((h, n), (full, len as u64));
    let sampled =
        ps_hash::identity_from_reader(fs::File::open(&out).expect("open"), n).expect("id");
    assert_eq!(sampled, f.content);
}

#[test]
fn scenario_export_export_leaves_sources_untouched() {
    let c = small_corpus();
    let m = corpus_model(c, &[], &[]);
    let lib_dir = scratch("untouched");
    let lib = lib_dir.path().join("library");
    let sources_before = snapshot(&c.root, &[]);
    let scratch_before = snapshot(lib_dir.path(), &[&lib]);
    let mut ledger = MemoryLedger::new();
    let cfg = ExecConfig {
        threads: 4,
        ..Default::default()
    };
    let rows = ledger.entries().expect("rows");
    let p = plan(
        &m.input,
        &rows,
        &lib,
        &FsLibrary::new(&lib),
        ProfileSettings::defaults(ExportProfile::Lightroom),
    )
    .expect("plan");
    let r = execute(
        &p,
        &m.input,
        &mut ledger,
        &cfg,
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(
        r.copied as usize,
        m.input.units.iter().map(|u| u.files.len()).sum::<usize>()
    );
    // Sources: paths, sizes, modification times and hashes identical.
    assert_eq!(snapshot(&c.root, &[]), sources_before);
    // Nothing written outside the library.
    assert_eq!(snapshot(lib_dir.path(), &[&lib]), scratch_before);
    // Every library file is a verified keeper copy or our own sidecar/manifest.
    let by_hash = library_by_hash(&lib);
    assert_eq!(by_hash.len(), r.copied as usize);
    assert!(by_hash.values().all(|v| v.len() == 1));
    assert!(partials_under(&lib).is_empty());
}

#[test]
fn scenario_export_re_export_copies_only_new() {
    let c = small_corpus();
    let drives: Vec<&str> = c.truth.drives.iter().map(String::as_str).collect();
    let lib_dir = scratch("reexport");
    let lib = lib_dir.path().join("library");
    let mut ledger = MemoryLedger::new();
    // First export: the first two drives.
    let first = corpus_model(c, &drives[..2], &[]);
    let r1 = run(
        &first.input,
        &lib,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
    );
    assert!(r1.copied > 0 && r1.failed.is_empty());
    let snap = snapshot(&lib, &[&lib.join(".photosquirrel")]);
    // Then "a new drive is scanned": all three drives.
    let all = corpus_model(c, &[], &[]);
    let rows = ledger.entries().expect("rows");
    let p = plan(&all.input, &rows, &lib, &FsLibrary::new(&lib), google()).expect("plan");
    let first_keepers: std::collections::HashSet<_> = first
        .input
        .units
        .iter()
        .flat_map(|u| u.files.iter().map(|f| f.content))
        .collect();
    assert!(
        p.files().all(|f| !first_keepers.contains(&f.content)),
        "nothing already exported is planned"
    );
    let new_count = p.totals.files;
    let r2 = execute(
        &p,
        &all.input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert_eq!(r2.copied, new_count);
    // Old library files untouched.
    let after = snapshot(&lib, &[&lib.join(".photosquirrel")]);
    for (k, v) in &snap {
        assert_eq!(after.get(k), Some(v));
    }
    // A third run copies nothing.
    let r3 = run(
        &all.input,
        &lib,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
    );
    assert_eq!((r3.copied, r3.adopted), (0, 0));
}

/// Fails with "disk full" when the n-th file starts.
struct DiskFullAt {
    n: usize,
    seen: Mutex<Vec<FileId>>,
}

impl CopyHooks for DiskFullAt {
    fn before_write(&self, f: FileId, offset: u64, _c: &mut [u8]) -> io::Result<()> {
        let mut seen = self.seen.lock().expect("lock");
        if !seen.contains(&f) {
            seen.push(f);
        }
        if seen.len() >= self.n && offset > 0 {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "No space left on device",
            ));
        }
        Ok(())
    }
}

/// ADR-0044: a re-plan trusts the ledger. With every keeper exported and nothing changed, planning again stats
/// the library copies but reads none of them; a copy whose modification time changed with the same bytes is
/// re-hashed once and still counts as exported.
#[test]
fn scenario_export_re_plan_reads_no_file_contents() {
    const N: usize = 24;
    let mut s = Sources::new("replan", 1);
    let units: Vec<_> = (0..N)
        .map(|i| {
            unit(vec![s.add(
                0,
                &format!("DCIM/IMG_{i:04}.JPG"),
                &bytes(300 + i as u64, 20_000),
                1_500_000_000 + i as i64,
                day(2019, 3, 1 + (i % 20) as u8),
            )])
        })
        .collect();
    let input = s.input(units);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!(r.copied as usize, N);
    let rows = ledger.entries().expect("rows");
    assert!(
        rows.iter().all(|e| e.library_mtime_ns.is_some()),
        "done rows record the copy's time"
    );

    let probe = CountingProbe::new(&lib);
    let p = plan(&input, &rows, &lib, &probe, google()).expect("plan");
    assert_eq!(p.totals.already_exported as usize, N);
    assert_eq!((p.totals.files, p.totals.adopted), (0, 0));
    assert_eq!(probe.reads(), 0, "no library file was read");

    // A copy touched by another program (same bytes, new time) is re-hashed, alone, and still exported.
    let touched = lib.join(rows[3].library_rel_path.to_path_buf());
    set_mtime(&touched, 1_700_000_000);
    let probe = CountingProbe::new(&lib);
    let p = plan(&input, &rows, &lib, &probe, google()).expect("plan");
    assert_eq!(p.totals.already_exported as usize, N);
    assert_eq!(probe.reads(), 1);

    // `--verify` re-hashes every recorded copy.
    let probe = CountingProbe::new(&lib);
    let p = plan(&input, &rows, &lib, &probe, google().with_verify(true)).expect("plan");
    assert_eq!(p.totals.already_exported as usize, N);
    assert_eq!(probe.reads(), N);
}

#[test]
fn scenario_export_disk_full_stops_cleanly() {
    let mut s = Sources::new("diskfull", 1);
    let units: Vec<_> = (0..6)
        .map(|i| {
            unit(vec![s.add(
                0,
                &format!("IMG_{i}.JPG"),
                &bytes(30 + i, 300_000),
                0,
                day(2017, 1, 1),
            )])
        })
        .collect();
    let input = s.input(units);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let cfg = ExecConfig {
        copy: ps_export::CopyConfig {
            buffer_bytes: 64 << 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let hooks = DiskFullAt {
        n: 3,
        seen: Mutex::new(Vec::new()),
    };
    let r = run(&input, &lib, &mut ledger, &cfg, &hooks);
    assert_eq!(r.copied, 2, "completed files remain");
    assert!(
        matches!(&r.stopped, Some(StopReason::DiskFull(m)) if m.contains("full")),
        "{:?}",
        r.stopped
    );
    assert!(partials_under(&lib).is_empty(), "partial removed");
    assert_eq!(ledger.done().count(), 2, "completed files recorded");
    assert_eq!(library_by_hash(&lib).len(), 2);
    // With space again, the export completes.
    let r2 = run(&input, &lib, &mut ledger, &cfg, &NoHooks);
    assert_eq!(r2.copied, 4);
    assert_eq!(library_by_hash(&lib).len(), 6);
}

// ---- kill mid-export, then resume ----

const KILL_ENV: &str = "PS_EXPORT_KILL_DIR";
const KILL_FILES: u64 = 8;

/// The same input from the same folder in the parent and the child.
fn kill_input(dir: &Path) -> ExportInput {
    let root = dir.join("src");
    let mut names: Vec<_> = fs::read_dir(&root)
        .expect("src")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let src = ps_export::SourceInfo {
        id: ps_model::SourceId(1),
        root: root.clone(),
        online: true,
        name: "drive-k".into(),
    };
    let units = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let p = root.join(n);
            let data = fs::read(&p).expect("read");
            unit(vec![ps_export::KeeperFile {
                file: FileId(i as i64 + 1),
                source: src.id,
                rel_path: rel(n),
                size: data.len() as u64,
                mtime_ns: mtime_ns(&p),
                content: ps_hash::identity_from_buffer(&data),
                format: format_of(n),
                capture: day(2021, 9, 9),
                undated: false,
                date_origin: ps_export::DateOrigin::OWN_EXIF,
                content_identifier: None,
            }])
        })
        .collect();
    ExportInput {
        sources: vec![src],
        units,
    }
}

/// Kills the process in the middle of the 5th file.
struct AbortMidFile {
    files: AtomicUsize,
}

impl CopyHooks for AbortMidFile {
    fn before_write(&self, _f: FileId, offset: u64, _c: &mut [u8]) -> io::Result<()> {
        if offset == 0 {
            self.files.fetch_add(1, Ordering::SeqCst);
        }
        if self.files.load(Ordering::SeqCst) == 5 && offset >= 128 << 10 {
            std::process::abort();
        }
        Ok(())
    }
}

fn kill_cfg() -> ExecConfig {
    ExecConfig {
        copy: ps_export::CopyConfig {
            buffer_bytes: 64 << 10,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn scenario_export_interrupted_export_resumes() {
    if let Some(dir) = std::env::var_os(KILL_ENV) {
        // Child: export until killed.
        let dir = PathBuf::from(dir);
        let input = kill_input(&dir);
        let mut ledger = FileLedger::new(dir.join("ledger.jsonl"));
        let hooks = AbortMidFile {
            files: AtomicUsize::new(0),
        };
        run(
            &input,
            &dir.join("library"),
            &mut ledger,
            &kill_cfg(),
            &hooks,
        );
        unreachable!("the child is killed mid-copy");
    }
    let tmp = scratch("kill");
    let dir = tmp.path();
    fs::create_dir(dir.join("src")).expect("src");
    for i in 0..KILL_FILES {
        let p = dir.join("src").join(format!("IMG_{i:04}.JPG"));
        fs::write(&p, bytes(40 + i, 400_000)).expect("write");
        set_mtime(&p, 1_600_000_000 + i as i64);
    }
    let status = std::process::Command::new(std::env::current_exe().expect("exe"))
        .args([
            "scenario_export_interrupted_export_resumes",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KILL_ENV, dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn child");
    assert!(!status.success(), "child was killed");
    let lib = dir.join("library");
    assert_eq!(
        partials_under(&lib).len(),
        1,
        "the kill left one stale partial"
    );
    let mut ledger = FileLedger::new(dir.join("ledger.jsonl"));
    let rows = ledger.entries().expect("rows");
    assert_eq!(
        rows.iter().filter(|e| e.state == LedgerState::Done).count(),
        4
    );
    assert_eq!(
        rows.iter()
            .filter(|e| e.state == LedgerState::InProgress)
            .count(),
        1
    );

    let input = kill_input(dir);
    let r = run(&input, &lib, &mut ledger, &kill_cfg(), &NoHooks);
    assert_eq!(r.stale_partials_removed, 1);
    assert_eq!(r.copied, KILL_FILES - 4);
    assert!(partials_under(&lib).is_empty());
    // Every keeper exactly once, byte-identical.
    let by_hash = library_by_hash(&lib);
    assert_eq!(by_hash.len() as u64, KILL_FILES);
    assert!(by_hash.values().all(|v| v.len() == 1), "{by_hash:?}");
    for u in &input.units {
        assert!(by_hash.contains_key(&u.files[0].content.digest));
    }
    let rows = ledger.entries().expect("rows");
    assert_eq!(
        rows.iter().filter(|e| e.state == LedgerState::Done).count() as u64,
        KILL_FILES
    );
    // The manifest has a line per file (resumed files may not repeat).
    let manifest = ps_export::manifest::read_manifest(&lib).expect("manifest");
    let paths: std::collections::BTreeSet<_> =
        manifest.iter().map(|m| m.library_path.clone()).collect();
    assert_eq!(paths.len() as u64, KILL_FILES);
}

/// The worst case: the ledger is lost entirely after a kill. Re-planning adopts what is already in place
/// and copies the rest; stale partials without a ledger row are never deleted (they are not provably ours),
/// their names are avoided instead.
#[test]
fn resume_without_ledger_adopts_and_never_deletes_unknown_partials() {
    let mut s = Sources::new("lostledger", 1);
    let units: Vec<_> = (0..4)
        .map(|i| {
            unit(vec![s.add(
                0,
                &format!("IMG_{i}.JPG"),
                &bytes(60 + i, 50_000),
                0,
                day(2022, 2, 2),
            )])
        })
        .collect();
    let input = s.input(units);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    let stray = lib.join("2022/2022-02-02/IMG_9.JPG.ps-partial");
    fs::write(&stray, b"not provably ours").expect("stray");
    let mut fresh = MemoryLedger::new();
    let r = run(&input, &lib, &mut fresh, &ExecConfig::default(), &NoHooks);
    assert_eq!((r.copied, r.adopted), (0, 4));
    assert!(stray.exists());
    assert_eq!(library_by_hash(&lib).len(), 4);
}

/// Throughput measurement (run with `--ignored --nocapture`): 1 GB in 64 files of 16 MB.
#[test]
#[ignore = "measurement"]
fn measure_export_throughput() {
    for threads in [1, 4] {
        let mut s = Sources::new("throughput", 1);
        let units: Vec<_> = (0..64)
            .map(|i| {
                unit(vec![s.add(
                    0,
                    &format!("V_{i}.MOV"),
                    &bytes(i, 16 << 20),
                    0,
                    day(2023, 1, 1),
                )])
            })
            .collect();
        let input = s.input(units);
        let lib = s.library();
        let mut ledger = MemoryLedger::new();
        let cfg = ExecConfig {
            threads,
            ..Default::default()
        };
        let r = run(&input, &lib, &mut ledger, &cfg, &NoHooks);
        let mbps = r.bytes_copied as f64 / 1e6 / r.elapsed.as_secs_f64();
        println!(
            "export throughput threads={threads}: {} MB in {:.2} s = {mbps:.0} MB/s ({:.2} ms/MB)",
            r.bytes_copied / 1_000_000,
            r.elapsed.as_secs_f64(),
            1000.0 / mbps
        );
    }
}

/// Library files that are not sidecars.
fn photos_under(dir: &Path) -> usize {
    files_under(dir)
        .iter()
        .filter(|p| p.extension().is_none_or(|e| e != "xmp"))
        .count()
}

/// The folder fsync after the rename fails.
struct FolderSyncFails;

impl CopyHooks for FolderSyncFails {
    fn sync_folder(&self, _file: FileId, _dir: &Path) -> io::Result<()> {
        Err(io::Error::other("injected folder fsync failure"))
    }
}

/// D6: the file is renamed into place and verified when the folder fsync fails; that is a warning, not a
/// failed file, so the next export does not copy it again as " (2)".
#[test]
fn folder_fsync_failure_after_rename_still_records_done() {
    let mut s = Sources::new("dirsync", 1);
    let f = s.add(0, "DCIM/IMG_7.JPG", &bytes(70, 40_000), 0, day(2015, 5, 5));
    let input = s.input(vec![unit(vec![f.clone()])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(
        &input,
        &lib,
        &mut ledger,
        &ExecConfig::default(),
        &FolderSyncFails,
    );
    assert_eq!(r.copied, 1);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    let row = rel("2015/2015-05-05/IMG_7.JPG");
    assert_eq!(ledger.get(&row).expect("row").state, LedgerState::Done);
    let again = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!((again.copied, again.adopted), (0, 0));
    assert_eq!(photos_under(&lib.join("2015")), 1, "no \" (2)\" copy");
}

/// Something replaces the final file right after the rename (the check-then-rename race of ADR-0038).
struct ReplacedAfterRename;

impl CopyHooks for ReplacedAfterRename {
    fn after_rename(&self, _file: FileId, target: &Path) {
        fs::write(target, b"someone else's bytes").expect("replace");
    }
}

/// D6: the post-rename verification (ADR-0038/ADR-0040) finds the final file differs: it is kept (never
/// deleted), its ledger row is `suspect`, and the report lists it.
#[test]
fn file_differing_after_rename_is_kept_and_marked_suspect() {
    let mut s = Sources::new("suspect", 1);
    let f = s.add(0, "DCIM/IMG_8.JPG", &bytes(80, 30_000), 0, day(2016, 6, 6));
    let input = s.input(vec![unit(vec![f.clone()])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(
        &input,
        &lib,
        &mut ledger,
        &ExecConfig::default(),
        &ReplacedAfterRename,
    );
    let path = rel("2016/2016-06-06/IMG_8.JPG");
    assert_eq!(r.copied, 0);
    assert_eq!(r.suspect.len(), 1, "{r:?}");
    assert_eq!(r.suspect[0].target, path);
    assert_eq!(ledger.get(&path).expect("row").state, LedgerState::Suspect);
    assert_eq!(
        fs::read(lib.join("2016/2016-06-06/IMG_8.JPG")).expect("kept"),
        b"someone else's bytes"
    );
    // The next export makes a verified copy beside it and leaves the suspect file alone.
    let again = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!(again.copied, 1);
    assert_eq!(photos_under(&lib.join("2016")), 2);
}

/// D7: a ledger row says "done" and a same-size file is at its path, but its bytes are different: it is not
/// taken as already exported; the keeper is copied again under a free name, and that file is left as it is.
#[test]
fn same_size_different_bytes_is_not_already_exported() {
    let mut s = Sources::new("samesize", 1);
    let f = s.add(0, "DCIM/IMG_9.JPG", &bytes(90, 25_000), 0, day(2017, 7, 7));
    let input = s.input(vec![unit(vec![f.clone()])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let r = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!(r.copied, 1);
    let out = lib.join("2017/2017-07-07/IMG_9.JPG");
    let other = bytes(91, 25_000);
    fs::write(&out, &other).expect("replace with same-size bytes");
    let again = run(&input, &lib, &mut ledger, &ExecConfig::default(), &NoHooks);
    assert_eq!((again.copied, again.adopted), (1, 0), "{again:?}");
    assert_eq!(fs::read(&out).expect("untouched"), other);
    let by_hash = library_by_hash(&lib);
    assert!(
        by_hash.contains_key(&f.content.digest),
        "a verified copy exists"
    );
}
