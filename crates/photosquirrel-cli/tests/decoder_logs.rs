//! Scenarios of `quiet-decoder-logs`: rawler's per-file warnings hidden by the default log filter, shown again by
//! `RUST_LOG`, and a damaged RAW file still reported by the run whatever the filter (since `damaged-raw-reason`:
//! as "could not be read", an error and a damaged file, ADR-0115). Drives the built binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const LENS_WARNING: &str = "No lens definition found";

fn cli(data: &Path, rust_log: Option<&str>) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
    c.env("PHOTOSQUIRREL_DATA_DIR", data)
        .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null());
    if let Some(v) = rust_log {
        c.env("RUST_LOG", v);
    }
    c
}

fn ok(data: &Path, rust_log: Option<&str>, args: &[&str]) -> Output {
    let o = cli(data, rust_log).args(args).output().expect("run cli");
    assert!(
        o.status.success(),
        "{args:?} exited {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

/// Adds `photos` as a source of a fresh data folder and scans it; returns the data folder and the scan's output.
fn scan(photos: &Path, rust_log: Option<&str>, json: bool) -> (tempfile::TempDir, Output) {
    let data = tempfile::tempdir().unwrap();
    ok(
        data.path(),
        rust_log,
        &["sources", "add", photos.to_str().unwrap()],
    );
    let args: &[&str] = if json { &["--json", "scan"] } else { &["scan"] };
    let o = ok(data.path(), rust_log, args);
    (data, o)
}

/// A folder with one Canon CR2 whose lens rawler does not know, read through ps-media's rawler fallback.
fn unknown_lens_card() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("DCIM/100CANON")).unwrap();
    std::fs::write(
        dir.path().join("DCIM/100CANON/IMG_0001.CR2"),
        ps_fixtures::dng::cr2_unknown_lens(7),
    )
    .unwrap();
    dir
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The JSON line of `type` in a `--json` command's output.
fn json_line(o: &Output, ty: &str) -> Value {
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    out.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == ty)
        .unwrap_or_else(|| panic!("no {ty} line: {out}"))
}

#[test]
fn scenario_cli_lens_database_warnings_are_hidden_by_default() {
    let card = unknown_lens_card();
    let (_data, o) = scan(card.path(), None, true);
    let err = stderr(&o);
    assert!(!err.contains(LENS_WARNING), "{err}");
    assert!(!err.contains("rawler"), "{err}");
    let summary = json_line(&o, "summary");
    assert_eq!(summary["data"]["status"], "completed", "{summary}");
    assert_eq!(summary["data"]["processed"], 1, "{summary}");
    assert_eq!(summary["data"]["errors"], 0, "{summary}");
}

#[test]
fn scenario_cli_rust_log_shows_them_again() {
    let card = unknown_lens_card();
    let (_data, o) = scan(card.path(), Some("rawler=warn"), true);
    let err = stderr(&o);
    assert!(err.contains(LENS_WARNING), "{err}");
    assert!(err.contains("rawler::lens"), "{err}");
    // A plain `RUST_LOG=warn` replaces the default whole, too.
    let (_data, o) = scan(card.path(), Some("warn"), true);
    assert!(stderr(&o).contains(LENS_WARNING), "{}", stderr(&o));
}

/// A folder with the unknown-lens CR2 cut off after its first eighth: the directories and the maker note are
/// there (rawler has things to warn about), most of the preview JPEG is not.
fn truncated_card() -> tempfile::TempDir {
    let card = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(card.path().join("DCIM/100CANON")).unwrap();
    let full = ps_fixtures::dng::cr2_unknown_lens(11);
    std::fs::write(
        card.path().join("DCIM/100CANON/IMG_0034.CR2"),
        &full[..full.len() / 8],
    )
    .unwrap();
    card
}

#[test]
fn scenario_scan_progress_damaged_raw_file_still_reported_under_the_default_filter() {
    let card = truncated_card();
    let (data, o) = scan(card.path(), None, true);
    let err = stderr(&o);
    assert!(!err.contains("rawler"), "{err}");
    // The run took the file in and finished, and counts it as an error and a damaged file (ADR-0115).
    let summary = json_line(&o, "summary");
    assert_eq!(summary["data"]["status"], "completed", "{summary}");
    assert_eq!(summary["data"]["processed"], 1, "{summary}");
    assert_eq!(summary["data"]["diff"]["new"], 1, "{summary}");
    assert_eq!(summary["data"]["errors"], 1, "{summary}");
    assert_eq!(summary["data"]["damaged"], 1, "{summary}");
    // The run lists it among its errors (its last snapshot carries them).
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let last = out
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["type"] == "snapshot")
        .last()
        .unwrap_or_else(|| panic!("no snapshot: {out}"));
    let listed = last["data"]["recent_errors"].to_string();
    assert!(listed.contains("IMG_0034.CR2"), "{last}");
    assert!(listed.contains("cut short"), "{last}");
    // Its outcome, a placeholder with the reason, comes from PhotoSquirrel's own result.
    let st = ok(data.path(), None, &["--json", "previews", "status"]);
    let v: Value = serde_json::from_slice(&st.stdout).unwrap();
    let rows = v["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["format"], "raw:cr2", "{rows:?}");
    assert_eq!(rows[0]["reason"], "unreadable", "{rows:?}");
    assert_eq!(rows[0]["count"], 1, "{rows:?}");
    let text = String::from_utf8_lossy(&ok(data.path(), None, &["previews", "status"]).stdout)
        .into_owned();
    assert!(text.contains("raw:cr2"), "{text}");
    // The same scan with rawler's warnings shown: the log has more to say, the outcome is the same.
    let (data, o) = scan(card.path(), Some("rawler=warn"), true);
    assert!(stderr(&o).contains("WARN  rawler"), "{}", stderr(&o));
    let summary = json_line(&o, "summary");
    assert_eq!(
        (&summary["data"]["errors"], &summary["data"]["damaged"]),
        (&Value::from(1), &Value::from(1)),
        "{summary}"
    );
    let st = ok(data.path(), None, &["--json", "previews", "status"]);
    let again: Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(again["data"], v["data"]);
}
