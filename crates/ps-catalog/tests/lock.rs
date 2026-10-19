//! Single-writer protection (D5, ADR-0005): the owning process holds an exclusive OS lock on the data folder's
//! `catalog.db.lock`; a second process cannot open the catalog for writing, only as an observer.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command as Proc, Stdio};

use common::*;
use ps_catalog::{Catalog, CatalogError, Command, OpenOptions};
use ps_model::RunStatus;

const CHILD_ENV: &str = "PS_CATALOG_LOCK_DIR";

#[test]
fn second_process_cannot_own_the_catalog_and_observes_it_read_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("exe");
    let mut child = Proc::new(&exe)
        .args([
            "lock_owner_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, tmp.path())
        .stdin(Stdio::piped())
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
    // The child owns the catalog and has a running scan run.
    let err = Catalog::open(tmp.path()).expect_err("second owner refused");
    assert!(matches!(err, CatalogError::Locked { .. }), "{err:?}");
    assert!(err.to_string().contains("another PhotoSquirrel process"));
    // Observers read, never write: the live run stays `running`.
    let obs = Catalog::open_or_observe(tmp.path(), OpenOptions::default()).expect("observer");
    assert!(obs.is_observer());
    let runs = obs.read().runs().expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Running);
    assert!(matches!(
        obs.writer().submit(Command::InterruptRunning),
        Err(CatalogError::ReadOnly)
    ));
    drop(obs);
    // The owner exits: the lock is released and the catalog can be owned again.
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"exit\n")
        .expect("tell child");
    assert!(child.wait().expect("wait").success());
    let cat = Catalog::open(tmp.path()).expect("owner after the child exited");
    assert!(!cat.is_observer());
}

#[test]
fn one_owner_per_catalog_within_a_process_too() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = Catalog::open(tmp.path()).expect("first");
    assert!(matches!(
        Catalog::open(tmp.path()),
        Err(CatalogError::Locked { .. })
    ));
    drop(a);
    Catalog::open(tmp.path()).expect("reopen after drop");
}

/// Read-only commands observe even when nobody owns the catalog (they must never take the lock and make a
/// starting scan fail): an observer reads a closed catalog without its WAL files.
#[test]
fn observer_reads_a_closed_catalog_without_taking_the_lock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    {
        let cat = Catalog::open(tmp.path()).expect("owner");
        add_source(&cat, "/mnt/x");
        cat.close().expect("close");
    }
    let obs = Catalog::open_reader(tmp.path(), OpenOptions::default()).expect("observer");
    assert!(obs.is_observer());
    assert_eq!(obs.read().sources().expect("sources").len(), 1);
    // The lock is free while the observer is open.
    let owner = Catalog::open(tmp.path()).expect("owner while observed");
    drop(owner);
    drop(obs);
    // No catalog yet: the reader creates it as the owner, then lets go.
    let fresh = tempfile::tempdir().expect("tempdir");
    let r = Catalog::open_reader(fresh.path(), OpenOptions::default()).expect("fresh");
    assert!(r.read().sources().expect("sources").is_empty());
}

/// The child: owns the catalog with a running scan run until told to exit on stdin.
#[test]
#[ignore = "child process of second_process_cannot_own_the_catalog_and_observes_it_read_only"]
fn lock_owner_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let cat = Catalog::open(std::path::Path::new(&dir)).expect("open");
    let src = add_source(&cat, "/mnt/lock");
    cat.writer().start_run(vec![src]).expect("run");
    println!("child-ready");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}
