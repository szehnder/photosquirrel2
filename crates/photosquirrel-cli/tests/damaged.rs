//! CLI scenarios of `damaged-raw-reason` (ADR-0115): the scan summary names damaged files, `report` flags them as
//! at risk (exported as is, keep the original). Drives the built binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn cli(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
    c.env("PHOTOSQUIRREL_DATA_DIR", data)
        .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null());
    c
}

fn ok(data: &Path, args: &[&str]) -> Output {
    let o = cli(data).args(args).output().expect("run cli");
    assert!(
        o.status.success(),
        "{args:?} exited {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The JSON line of `type` in a `--json` command's output.
fn json_line(o: &Output, ty: &str) -> Value {
    let out = stdout(o);
    out.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == ty)
        .unwrap_or_else(|| panic!("no {ty} line: {out}"))
}

fn json(o: &Output) -> Value {
    let s = stdout(o);
    serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("{e}: {s}"))
}

const CR2: &str = "DCIM/100CANON/IMG_0034.CR2";

/// A card with one cut CR2 (cut after its intact preview) and two intact JPEGs.
fn card() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("card");
    let dir = root.join("DCIM/100CANON");
    std::fs::create_dir_all(&dir).unwrap();
    let shot = ps_fixtures::dng::converted_burst_frame(91, 0, 0, 960);
    std::fs::write(
        root.join(CR2),
        ps_fixtures::dng::cut(&shot.cr2, ps_fixtures::dng::Cut::AfterPreview).unwrap(),
    )
    .unwrap();
    for i in 0..2u64 {
        std::fs::write(
            dir.join(format!("IMG_{:04}.JPG", 100 + i)),
            ps_fixtures::media::jpeg(800, 600, 60 + i, 85),
        )
        .unwrap();
    }
    (tmp, root)
}

fn added(root: &Path) -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    ok(data.path(), &["sources", "add", root.to_str().unwrap()]);
    data
}

#[test]
fn scenario_cli_summary_names_a_damaged_raw() {
    let (_tmp, root) = card();
    let data = added(&root);
    let text = stdout(&ok(data.path(), &["scan"]));
    assert!(
        text.contains(
            "1 damaged file could not be read (cut short or corrupt). It is catalogued and copied as is; keep \
             the original card or drive until you have checked it. List: `photosquirrel-cli report`"
        ),
        "{text}"
    );
    let data = added(&root);
    let o = ok(data.path(), &["--json", "scan"]);
    let s = json_line(&o, "summary");
    assert_eq!(s["data"]["damaged"], 1, "{s}");
    assert_eq!(s["data"]["errors"], 1, "{s}");
}

#[test]
fn scenario_cli_no_damaged_line_for_intact_files() {
    let corpus = ps_fixtures::shared_corpus(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "cli-corpus-small-42",
        42,
        ps_fixtures::Preset::Small,
    );
    let data = added(&corpus.join("drive-a"));
    let text = stdout(&ok(data.path(), &["scan"]));
    assert!(!text.contains("damaged"), "{text}");
    let o = ok(data.path(), &["--json", "scan"]);
    assert_eq!(json_line(&o, "summary")["data"]["damaged"], 0);
}

#[test]
fn scenario_completion_report_exported_damaged_raw_is_still_flagged() {
    let (tmp, root) = card();
    let data = added(&root);
    ok(data.path(), &["scan"]);
    let lib = tmp.path().join("Library");
    ok(
        data.path(),
        &[
            "export",
            "run",
            "--library",
            lib.to_str().unwrap(),
            "--profile",
            "lightroom",
            "--yes",
        ],
    );
    let text = stdout(&ok(data.path(), &["report"]));
    assert!(text.contains("100% accounted for, 1 damaged"), "{text}");
    assert!(
        text.contains(&format!("    1 damaged file: {}", ps_export::DAMAGED_HINT)),
        "{text}"
    );
    assert!(text.contains(&format!("      {CR2}")), "{text}");
    let v = json(&ok(data.path(), &["--json", "report"]));
    let s = &v["data"]["sources"][0];
    assert_eq!(s["badge"], "100% accounted for, 1 damaged", "{s}");
    assert_eq!(s["counts"]["damaged"], 1, "{s}");
    assert_eq!(
        s["counts"]["files"]["exported"], 3,
        "the cut CR2 is exported: {s}"
    );
    assert_eq!(s["damaged_files"], serde_json::json!([CR2]), "{s}");
    assert_eq!(v["data"]["totals"]["damaged"], 1);
}

/// An intact corpus's report has no damaged suffix and no damaged files.
#[test]
fn intact_report_has_no_damaged_suffix() {
    let corpus = ps_fixtures::shared_corpus(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "cli-corpus-small-42",
        42,
        ps_fixtures::Preset::Small,
    );
    let data = added(&corpus.join("drive-b"));
    ok(data.path(), &["scan"]);
    let lib = tempfile::tempdir().unwrap();
    ok(
        data.path(),
        &[
            "export",
            "run",
            "--library",
            lib.path().to_str().unwrap(),
            "--profile",
            "lightroom",
            "--yes",
        ],
    );
    let text = stdout(&ok(data.path(), &["report"]));
    assert!(text.contains("100% accounted for"), "{text}");
    assert!(!text.contains("damaged"), "{text}");
    let v = json(&ok(data.path(), &["--json", "report"]));
    assert_eq!(
        v["data"]["sources"][0]["damaged_files"],
        serde_json::json!([])
    );
}
