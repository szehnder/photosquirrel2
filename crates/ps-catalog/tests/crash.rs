//! Crash safety (catalog task 2.2): a child process writing staged results is SIGKILLed at a seeded random
//! moment; the reopened catalog must be consistent.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command as Proc, Stdio};
use std::time::Duration;

use common::*;
use ps_catalog::{Catalog, FileKey};
use rusqlite::Connection;

const CHILD_ENV: &str = "PS_CATALOG_CRASH_DIR";
const FILES: u64 = 100_000;
const CHUNK: u64 = 500;

#[test]
fn scenario_catalog_killed_writer_leaves_consistent_state() {
    let seed: u64 = std::env::var("PS_CATALOG_CRASH_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5EED);
    let exe = std::env::current_exe().expect("exe");
    for round in 0..3u64 {
        let kill_after = Duration::from_millis(30 + mix(seed ^ round) % 1500);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut child = Proc::new(&exe)
            .args([
                "crash_child",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, tmp.path())
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
        std::thread::sleep(kill_after);
        child.kill().expect("kill"); // SIGKILL on Unix, TerminateProcess on Windows
        child.wait().expect("wait");

        let path = tmp.path().join("catalog.db");
        let cat = Catalog::open(tmp.path()).expect("reopen after kill");
        let conn = Connection::open(&path).expect("raw");
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .expect("integrity");
        assert_eq!(integrity, "ok");
        let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).expect(sql) };
        let total = q("SELECT count(*) FROM file");
        let analyzed = q("SELECT count(*) FROM file WHERE stage IN (2, 3)");
        let thumbed = q("SELECT count(*) FROM file WHERE stage = 3");
        let probed = q("SELECT count(*) FROM file WHERE stage IN (1, 4)");
        println!(
            "round {round}: killed after {kill_after:?}: {total} rows, {probed} probed/skipped, \
             {analyzed} analyzed ({thumbed} thumbed)"
        );
        assert!(total > 0, "the child committed something before the kill");
        // Every analyzed file has its identity and metadata ...
        assert_eq!(
            q("SELECT count(*) FROM file WHERE stage IN (2, 3)
               AND (content_id IS NULL OR metadata IS NULL OR class IS NULL OR capture_undated IS NULL)"),
            0
        );
        // ... and no file has results of a stage it has not reached.
        assert_eq!(
            q("SELECT count(*) FROM file WHERE stage IN (1, 4)
               AND (class IS NULL OR content_id IS NOT NULL OR metadata IS NOT NULL OR phash IS NOT NULL)"),
            0
        );
        assert_eq!(
            q("SELECT count(*) FROM file WHERE stage = 0
               AND (class IS NOT NULL OR format IS NOT NULL OR content_id IS NOT NULL OR metadata IS NOT NULL)"),
            0
        );
        // The packed index flags agree with the columns they summarise.
        assert_eq!(
            q("SELECT count(*) FROM file WHERE kflags !=
               (coalesce(class, 7) | (coalesce(media_kind, 0) << 3) | ((missing_since IS NOT NULL) << 4))"),
            0
        );
        // Results belong to their own file.
        let mut stmt = conn
            .prepare("SELECT rel_path, content_id FROM file WHERE stage IN (2, 3)")
            .expect("prepare");
        let mut rows = stmt.query([]).expect("query");
        while let Some(r) = rows.next().expect("row") {
            let p: Vec<u8> = r.get(0).expect("path");
            let d: Vec<u8> = r.get(1).expect("digest");
            let i: u64 = std::str::from_utf8(&p[p.len() - 11..p.len() - 4])
                .expect("utf8")
                .parse()
                .expect("index");
            assert_eq!(d, digest_of(i), "digest of {}", String::from_utf8_lossy(&p));
        }
        // The reopened catalog is writable: the scan can resume.
        let src = ps_model::SourceId(1);
        cat.writer()
            .upsert_files(vec![full_row(src, FILES + 1)])
            .expect("resume write");
        cat.writer().flush().expect("resume flush");
    }
}

/// The child: writes 100,000 files through the stages, interleaved, with no explicit flushes (only the
/// writer's own batching), then idles until killed.
#[test]
#[ignore = "child process of scenario_catalog_killed_writer_leaves_consistent_state"]
fn crash_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let cat = Catalog::open(std::path::Path::new(&dir)).expect("open");
    let src = add_source(&cat, "/mnt/crash");
    println!("child-ready");
    let w = cat.writer();
    let key = |i: u64| FileKey::new(src, path_of(i));
    let chunks = FILES / CHUNK;
    for c in 0..chunks + 2 {
        // Stage pipeline across chunks: walk chunk c, probe chunk c-1, analyze + thumb chunk c-2.
        if c < chunks {
            let r = (c * CHUNK..(c + 1) * CHUNK)
                .map(|i| discovered_row(src, i))
                .collect();
            w.upsert_files(r).expect("walk");
        }
        if c >= 1 && c - 1 < chunks {
            let r = ((c - 1) * CHUNK..c * CHUNK)
                .map(|i| (key(i), probe_of(i)))
                .collect();
            w.probed(r).expect("probe");
        }
        if c >= 2 {
            let range = (c - 2) * CHUNK..(c - 1) * CHUNK;
            let cand: Vec<u64> = range.filter(|i| !probe_of(*i).skip).collect();
            w.analyzed(
                cand.iter()
                    .map(|i| (key(*i), Box::new(analysis_of(*i))))
                    .collect(),
            )
            .expect("analyze");
            w.thumbed(
                cand.iter()
                    .filter(|i| i.is_multiple_of(2))
                    .map(|i| key(*i))
                    .collect(),
            )
            .expect("thumb");
        }
    }
    println!("child-done");
    std::thread::sleep(Duration::from_secs(600));
}
