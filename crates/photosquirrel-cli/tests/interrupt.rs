//! `cli` interrupt scenarios (headless-cli task 2.3), in their own test binary so no other scan competes for the
//! cores while the 1 s exit bound is measured. The two tests run one at a time.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn cli(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
    c.env("PHOTOSQUIRREL_DATA_DIR", data)
        .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
        .stdin(Stdio::null());
    c
}

fn ok(data: &Path, args: &[&str]) -> Output {
    let o = cli(data).args(args).output().unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap()
}

/// The medium corpus when present (`target/corpus-medium`), else a small one generated here.
fn medium_or_small() -> PathBuf {
    let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-medium");
    if ps_fixtures::Truth::read(&m).is_ok() {
        return m.canonicalize().unwrap();
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-interrupt-corpus-small-42");
    if ps_fixtures::Truth::read(&root).is_err() {
        let w = ps_fixtures::DirWriter::new(&root).unwrap();
        ps_fixtures::generate(42, ps_fixtures::Preset::Small, &w).unwrap();
    }
    root
}

/// Start a scan of the medium corpus (small when absent), interrupt it with SIGINT once files are being
/// processed; returns the data folder, run id and files processed before the interrupt.
fn interrupted_scan() -> (tempfile::TempDir, i64, u64) {
    let data = tempfile::tempdir().unwrap();
    let root = medium_or_small();
    for d in ["drive-a", "drive-b", "drive-c"] {
        ok(
            data.path(),
            &["sources", "add", root.join(d).to_str().unwrap()],
        );
    }
    let mut child = cli(data.path())
        .args(["--json", "scan"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    // Wait until files are being processed.
    loop {
        let l = out.next().expect("snapshot line").unwrap();
        let v: Value = serde_json::from_str(&l).unwrap();
        if v["data"]["processed"].as_u64().unwrap_or(0) > 20 {
            break;
        }
    }
    let t = Instant::now();
    let kill = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let rest: Vec<String> = out.map(|l| l.unwrap()).collect();
    let status = child.wait().unwrap();
    let took = t.elapsed();
    assert_eq!(status.code(), Some(130));
    assert!(took < Duration::from_secs(1), "exited after {took:?}");
    let summary: Value = serde_json::from_str(rest.last().unwrap()).unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["data"]["status"], "cancelled");
    let run = summary["data"]["run"].as_i64().unwrap();
    let first = summary["data"]["processed"].as_u64().unwrap();
    let st = json(&ok(data.path(), &["--json", "status"]));
    assert_eq!(st["data"]["runs"][0]["status"], "cancelled");
    (data, run, first)
}

#[test]
fn scenario_cli_ctrl_c_cancels_scan() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    interrupted_scan();
}

#[test]
fn scenario_cli_resume_an_interrupted_run() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (data, run, first) = interrupted_scan();
    let o = ok(data.path(), &["--json", "scan", "--resume"]);
    let s = String::from_utf8_lossy(&o.stdout);
    let last: Value = serde_json::from_str(s.lines().last().unwrap()).unwrap();
    assert_eq!(last["data"]["status"], "completed");
    assert_eq!(last["data"]["run"].as_i64().unwrap(), run, "same run id");
    let files = last["data"]["stages"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_u64().unwrap())
        .sum::<u64>();
    let second = last["data"]["processed"].as_u64().unwrap();
    assert!(
        first > 0 && second < files,
        "finished files are not re-read: {second} of {files}"
    );
}
