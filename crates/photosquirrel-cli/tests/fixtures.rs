//! CLI scenarios of the `test-corpus` capability.

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"))
}

#[test]
fn scenario_test_corpus_cli_generates_corpus() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("corpus");
    let o = cli()
        .args([
            "fixtures", "generate", "--seed", "42", "--preset", "small", "--out",
        ])
        .arg(&out)
        .output()
        .expect("run");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let truth = ps_fixtures::Truth::read(&out).expect("truth.json");
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        stdout.contains(&format!("generated {} files", truth.files.len())),
        "{stdout}"
    );
    for d in ["drive-a", "drive-b", "drive-c"] {
        assert!(out.join(d).is_dir(), "{d}");
    }
    // Writes only inside the output folder.
    let siblings: Vec<_> = std::fs::read_dir(dir.path()).expect("ls").collect();
    assert_eq!(siblings.len(), 1);

    // --json envelope.
    let out2 = dir.path().join("corpus2");
    let o = cli()
        .args(["--json", "fixtures", "generate", "--seed", "42", "--out"])
        .arg(&out2)
        .output()
        .expect("run");
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["command"], "fixtures generate");
    assert_eq!(v["data"]["files"], truth.files.len());
}

#[test]
fn scenario_test_corpus_refuses_non_empty_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("keep.txt"), b"mine").expect("write");
    let o = cli()
        .args([
            "fixtures", "generate", "--seed", "42", "--preset", "small", "--out",
        ])
        .arg(dir.path())
        .output()
        .expect("run");
    assert!(!o.status.success());
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains(&dir.path().display().to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("not empty"), "{stderr}");
    let entries: Vec<_> = std::fs::read_dir(dir.path()).expect("ls").collect();
    assert_eq!(entries.len(), 1, "nothing written");
    assert_eq!(
        std::fs::read(dir.path().join("keep.txt")).expect("read"),
        b"mine"
    );
}
