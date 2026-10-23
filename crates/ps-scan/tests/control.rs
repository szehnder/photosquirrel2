//! `scan-control` scenarios (scanning tasks 5.1-5.5): run records, the single-active-run rule, pause and
//! cancel latency, no partial results, resume, crash recovery and sources lost mid-scan.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use common::*;
use ps_model::{MediaKind, RunStatus, SourceId, SourceStatus, Stage};
use ps_scan::{ScanError, ScanOptions, Scanner};

const CHILD_ENV: &str = "PS_SCAN_KILL_CHILD";

fn drive_roots() -> Vec<PathBuf> {
    let c = corpus();
    c.truth.drives.iter().map(|d| c.root.join(d)).collect()
}

/// Add every corpus drive as a source.
fn add_drives(e: &Env) -> Vec<SourceId> {
    drive_roots().iter().map(|r| e.add(r)).collect()
}

/// The final catalog of a scanner, normalised: drive name + path -> comparable record.
fn final_catalog(e: &Env) -> BTreeMap<String, String> {
    let roots: BTreeMap<SourceId, String> = e
        .s()
        .sources()
        .expect("sources")
        .into_iter()
        .map(|s| {
            (
                s.id,
                s.root
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .collect();
    let mut out = BTreeMap::new();
    for (id, name) in roots {
        for (p, f) in e.files(id) {
            out.insert(format!("{name}/{p}"), normalized(&f));
        }
    }
    out
}

/// An uninterrupted scan of the corpus, for comparisons.
fn reference() -> &'static BTreeMap<String, String> {
    static R: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let e = env();
        add_drives(&e);
        completed(&e.scan(&[]));
        final_catalog(&e)
    })
}

#[test]
fn scenario_scan_control_completed_run_recorded() {
    let _cores = shared();
    let e = env();
    add_drives(&e);
    let summary = e.scan(&[]);
    completed(&summary);
    let runs = e.s().runs().expect("runs");
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    assert_eq!(r.status, RunStatus::Completed);
    assert!(r.started_at_ms > 0);
    let ended = r.ended_at_ms.expect("end time");
    assert!(ended >= r.started_at_ms);
    let stored = e.s().summary(r.id).expect("summary").expect("stored");
    assert_eq!(stored, summary);
    assert_eq!(stored.stages.total(), corpus().truth.files.len() as u64);
    assert!(stored.stages.thumbed > 0 && stored.stages.skipped > 0);
    assert!(
        r.stats["stages"]["thumbed"].as_u64().is_some(),
        "per-stage totals in the run row"
    );
}

#[test]
fn scenario_scan_control_second_scan_rejected_while_running() {
    let _cores = shared();
    let e = env_with(ScanOptions {
        read_delay: Some(Duration::from_millis(50)),
        ..ScanOptions::default()
    });
    add_drives(&e);
    let first = e.s().start_scan(&[]).expect("start");
    let err = e.s().start_scan(&[]).expect_err("second start");
    match err {
        ScanError::RunActive(id) => assert_eq!(id, first),
        other => panic!("expected RunActive, got {other:?}"),
    }
    assert!(
        e.s()
            .start_scan(&[])
            .expect_err("again")
            .to_string()
            .contains(&first.to_string()),
        "the refusal names the active run"
    );
    e.s().cancel().expect("cancel");
    e.s().wait();
    assert_eq!(e.s().runs().expect("runs").len(), 1);
}

#[test]
// A wall-clock budget: run alone, after the other tests, by `cargo xtask timing` (part of `cargo xtask ci`; ADR-0076).
#[ignore = "timing"]
fn scenario_scan_control_cancel_within_250_ms() {
    let _cores = exclusive();
    let e = env();
    add_drives(&e);
    e.s().start_scan(&[]).expect("start");
    wait_snapshot(e.s(), Duration::from_secs(120), "analysis under way", |s| {
        s.to_process > 0 && s.processed * 5 >= s.to_process && s.active_workers > 0
    });
    let t = Instant::now();
    e.s().cancel().expect("cancel");
    let summary = e.s().wait().expect("summary");
    let latency = t.elapsed();
    println!("cancel latency: {latency:?}");
    assert!(
        latency <= ps_fixtures::ci_slack(Duration::from_millis(250)),
        "all threads stopped after {latency:?}"
    );
    assert_eq!(summary.status, RunStatus::Cancelled);
    assert_eq!(e.s().snapshot().expect("snapshot").active_workers, 0);
    let run = &e.s().runs().expect("runs")[0];
    assert_eq!(run.status, RunStatus::Cancelled);
    // A cancelled run leaves unfinished files (it really stopped early).
    assert!(summary.processed < summary.sources.iter().map(|s| s.to_process).sum());
}

#[test]
// A wall-clock budget: run alone, after the other tests, by `cargo xtask timing` (part of `cargo xtask ci`; ADR-0076).
#[ignore = "timing"]
fn scenario_scan_control_pause_within_250_ms() {
    let _cores = exclusive();
    let e = env();
    add_drives(&e);
    let run = e.s().start_scan(&[]).expect("start");
    wait_snapshot(e.s(), Duration::from_secs(120), "analysis under way", |s| {
        s.processed > 20 && s.active_workers > 0
    });
    let t = Instant::now();
    e.s().pause().expect("pause");
    wait_snapshot(e.s(), Duration::from_secs(5), "idle threads", |s| {
        s.active_workers == 0
    });
    let latency = t.elapsed();
    println!("pause latency: {latency:?}");
    assert!(
        latency <= ps_fixtures::ci_slack(Duration::from_millis(250)),
        "idle after {latency:?}"
    );
    // No new file starts while paused.
    let opens = e.instr.open_count();
    let processed = e.s().snapshot().expect("snap").processed;
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(e.instr.open_count(), opens, "no file opened while paused");
    let snap = e.s().snapshot().expect("snap");
    assert!(
        snap.processed <= processed + 64,
        "only in-flight results land"
    );
    assert_eq!(snap.status, RunStatus::Paused);
    assert_eq!(
        e.s()
            .catalog()
            .read()
            .run(run)
            .expect("run")
            .expect("rec")
            .status,
        RunStatus::Paused
    );
    e.s().resume().expect("resume");
    completed(&e.s().wait().expect("summary"));
}

/// Candidate-sized files that take long to read: a camera JPEG padded (sparsely) to `size` bytes.
fn big_files(dir: &Path, n: usize, size: u64) {
    std::fs::create_dir_all(dir).expect("mkdir");
    for i in 0..n {
        let jpeg = ps_fixtures::media::jpeg_with_exif(
            1200,
            900,
            i as u64,
            85,
            &ps_fixtures::media::ExifSpec::canon_5d(),
        );
        let p = dir.join(format!("big-{i:03}.jpg"));
        std::fs::write(&p, &jpeg).expect("write");
        let f = std::fs::File::options().write(true).open(&p).expect("open");
        f.set_len(size).expect("pad");
    }
}

#[test]
fn scenario_scan_control_no_partial_results_visible() {
    let _cores = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("big");
    big_files(&root, 24, 40 * 1024 * 1024);
    let e = env_with(ScanOptions {
        chunk_delay: Some(Duration::from_millis(4)),
        device_class: Some(std::sync::Arc::new(|_: &Path| {
            ps_model::DeviceClass::SolidState
        })),
        ..ScanOptions::default()
    });
    let id = e.add(&root);
    for (round, after) in [60u64, 150, 300, 450].into_iter().enumerate() {
        e.s().start_scan(&[id]).expect("start");
        std::thread::sleep(Duration::from_millis(after));
        let mid_read = e.s().snapshot().expect("snapshot").buffered_bytes;
        let _ = e.s().cancel();
        let s = e.s().wait().expect("summary");
        println!("round {round}: cancelled after {after} ms with {mid_read} bytes buffered; {s:?}");
        for f in e.files(id).values() {
            match f.stage {
                Stage::Analyzed | Stage::Thumbed => {
                    assert!(
                        f.identity.is_some(),
                        "{} analyzed without identity",
                        f.key.path
                    );
                    assert!(
                        e.readers().metadata(f.id).expect("metadata").is_some(),
                        "{} analyzed without metadata",
                        f.key.path
                    );
                    if f.media_kind() == Some(MediaKind::Image) {
                        assert!(f.hashes.is_some(), "{} analyzed without hashes", f.key.path);
                    }
                }
                Stage::Discovered | Stage::Probed => {
                    assert!(f.identity.is_none() && f.hashes.is_none(), "{}", f.key.path);
                }
                Stage::Skipped | Stage::Failed => {}
            }
        }
    }
    // Every round made progress and the files resume to completion.
    let last = e.s().runs().expect("runs").last().expect("run").id;
    e.s().resume_run(last).expect("resume");
    completed(&e.s().wait().expect("summary"));
    assert!(e.files(id).values().all(|f| f.stage == Stage::Thumbed));
}

#[test]
fn scenario_scan_control_resume_skips_finished_files() {
    let _cores = shared();
    let e = env();
    let ids = add_drives(&e);
    let run = e.s().start_scan(&[]).expect("start");
    wait_snapshot(e.s(), Duration::from_secs(120), "40% thumbed", |s| {
        s.to_process > 0 && s.processed * 10 >= s.to_process * 4
    });
    e.s().cancel().expect("cancel");
    assert_eq!(e.s().wait().expect("summary").status, RunStatus::Cancelled);
    let roots: BTreeMap<SourceId, PathBuf> = e
        .s()
        .sources()
        .expect("sources")
        .into_iter()
        .map(|s| (s.id, s.root))
        .collect();
    let mut finished = BTreeSet::new();
    for id in &ids {
        for (p, f) in e.files(*id) {
            if f.stage.is_terminal() {
                finished.insert(roots[id].join(p));
            }
        }
    }
    assert!(!finished.is_empty());
    e.instr.reset();
    assert_eq!(e.s().resume_run(run).expect("resume"), run);
    let summary = e.s().wait().expect("summary");
    completed(&summary);
    for o in e.instr.opens() {
        assert!(
            !finished.contains(&o.path),
            "{} read again",
            o.path.display()
        );
    }
    assert_eq!(
        &final_catalog(&e),
        reference(),
        "same catalog as an uninterrupted scan"
    );
    assert_eq!(
        e.s().runs().expect("runs").len(),
        1,
        "the same run was resumed"
    );
}

#[test]
fn scenario_scan_control_killed_scan_becomes_interrupted() {
    let _cores = shared();
    let exe = std::env::current_exe().expect("exe");
    let corpus_root = corpus().root.clone();
    for (round, extra) in [0u64, 400].into_iter().enumerate() {
        let data = tempfile::tempdir().expect("data");
        let mut child = Proc::new(&exe)
            .args([
                "kill_child",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, data.path())
            .env("PS_SCAN_KILL_CORPUS", &corpus_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child");
        let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
        loop {
            let line = lines.next().expect("child exited early").expect("line");
            if line.contains("child-ready") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(extra));
        child.kill().expect("kill"); // SIGKILL
        child.wait().expect("wait");

        let scanner = Scanner::open(data.path(), ScanOptions::default()).expect("reopen");
        let runs = scanner.runs().expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Interrupted, "round {round}");
        assert_eq!(scanner.recovered_runs(), &[runs[0].id]);
        drop(scanner);
        let e = env_at(data.path());
        e.s().resume_run(runs[0].id).expect("resume");
        completed(&e.s().wait().expect("summary"));
        assert_eq!(&final_catalog(&e), reference(), "round {round}");
    }
}

/// The child of the kill test: scans the corpus slowly, reports once a tenth is done, then waits to die.
#[test]
#[ignore = "child process of scenario_scan_control_killed_scan_becomes_interrupted"]
fn kill_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let corpus_root = PathBuf::from(std::env::var_os("PS_SCAN_KILL_CORPUS").expect("corpus"));
    let s = Scanner::open(
        Path::new(&dir),
        ScanOptions {
            read_delay: Some(Duration::from_millis(2)),
            ..ScanOptions::default()
        },
    )
    .expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        s.add_source(&corpus_root.join(d)).expect("add");
    }
    s.start_scan(&[]).expect("start");
    loop {
        let snap = s.snapshot().expect("snapshot");
        if snap.to_process > 0 && snap.processed * 10 >= snap.to_process {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    println!("child-ready");
    std::thread::sleep(Duration::from_secs(600));
}

/// Make `from` vanish mid-scan by moving it to `to`. On Unix the rename succeeds while the scanner has files
/// under it open (like a drive pulled out mid-read); Windows refuses it (`Access is denied`) until no handle
/// under the folder is open, so retry until the scanner is between files.
fn unplug(from: &Path, to: &Path) {
    let t = Instant::now();
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return,
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && t.elapsed() < Duration::from_secs(60) =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("unplug {}: {e}", from.display()),
        }
    }
}

#[test]
fn scenario_scan_control_source_vanishes_mid_scan() {
    let _cores = shared();
    let c = corpus();
    let tmp = tempfile::tempdir().expect("tmp");
    let a = tmp.path().join("drive-a");
    let b = tmp.path().join("drive-c");
    copy_tree(&c.root.join("drive-a"), &a);
    copy_tree(&c.root.join("drive-c"), &b);
    let e = env_with(ScanOptions {
        read_delay: Some(Duration::from_millis(15)),
        // Windows cannot rename a folder while a file under it is open, so the unplug below waits for a
        // moment between files; one reader per lane makes those moments regular (every `read_delay`).
        max_readers: if cfg!(windows) {
            1
        } else {
            ScanOptions::default().max_readers
        },
        ..ScanOptions::default()
    });
    let ia = e.add(&a);
    let ib = e.add(&b);
    let run = e.s().start_scan(&[]).expect("start");
    wait_snapshot(e.s(), Duration::from_secs(120), "drive-c under way", |s| {
        s.sources
            .iter()
            .any(|p| p.source == ib && p.processed >= 10)
    });
    let away = tmp.path().join("unplugged");
    unplug(&b, &away);
    let summary = e.s().wait().expect("summary");
    completed(&summary);
    assert_eq!(summary.incomplete_sources, vec![ib], "{summary:#?}");
    assert!(
        e.files(ia).values().all(|f| f.stage.is_terminal()),
        "drive-a completes"
    );
    let src = e.s().sources().expect("sources");
    assert_eq!(
        src.iter().find(|s| s.id == ib).expect("b").status,
        SourceStatus::Offline
    );
    let fb = e.files(ib);
    assert!(
        fb.values().any(|f| !f.stage.is_terminal()),
        "drive-c is unfinished"
    );
    assert!(
        fb.values().all(|f| f.stage != Stage::Failed),
        "unfinished files keep their last completed stage"
    );
    // Re-linked at its new place, a resume finishes it.
    e.s().relink_source(ib, &away).expect("relink");
    e.s().resume_run(run).expect("resume");
    let s2 = e.s().wait().expect("summary");
    completed(&s2);
    assert!(s2.incomplete_sources.is_empty());
    assert!(e.files(ib).values().all(|f| f.stage.is_terminal()));
    assert_eq!(e.files(ib).len(), truth_under(c, "drive-c").count());
}
