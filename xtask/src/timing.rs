//! `cargo xtask timing [--runs N]`: the timing pass. Tests that assert a wall-clock or frame-time budget in the UI
//! crates are `#[ignore = "timing"]`, so the main `cargo test` pass skips them; this pass runs exactly them, one
//! test at a time (`--test-threads=1`), after the main pass and with nothing else of CI running (ADR-0075).
//! `cargo xtask ci` runs it once after `cargo test`.

use std::path::Path;

use anyhow::{Result, bail};

/// The `ignore` reason that marks a timing test.
pub const IGNORE_REASON: &str = "timing";

/// One timing test: package, test target, test name.
pub struct TimingTest {
    pub package: &'static str,
    pub target: &'static str,
    pub name: &'static str,
}

/// Every `#[ignore = "timing"]` test (checked against the sources by a unit test).
pub const TESTS: &[TimingTest] = &[
    TimingTest {
        package: "ui-workspace",
        target: "scan_real",
        name: "dashboard_controls_on_the_real_engine",
    },
    TimingTest {
        package: "ui-workspace",
        target: "scan_dashboard",
        name: "scenario_scan_progress_ui_cost_independent_of_rate",
    },
    TimingTest {
        package: "ui-workspace",
        target: "shell",
        name: "scenario_app_shell_slow_query_keeps_ui_responsive",
    },
    TimingTest {
        package: "ui-gallery",
        target: "grid",
        name: "scenario_gallery_jump_to_any_position",
    },
    TimingTest {
        package: "ui-gallery",
        target: "render_cost",
        name: "gallery_frame_cpu_cost_flinging_1m",
    },
];

/// Run the timing pass `runs` times; fails on the first failing run.
pub fn run(root: &Path, runs: u32) -> Result<()> {
    if runs == 0 {
        bail!("--runs must be at least 1");
    }
    eprintln!(
        "timing pass: {} tests marked #[ignore = \"{IGNORE_REASON}\"], one at a time",
        TESTS.len()
    );
    for i in 1..=runs {
        for package in ["ui-workspace", "ui-gallery"] {
            let tests: Vec<&TimingTest> = TESTS.iter().filter(|t| t.package == package).collect();
            let mut cmd = crate::cargo(root)?;
            cmd.args(["test", "-p", package]);
            for t in &tests {
                cmd.args(["--test", t.target]);
            }
            cmd.args(["--", "--ignored", "--exact", "--test-threads=1"]);
            cmd.args(tests.iter().map(|t| t.name));
            crate::run(&format!("timing pass {i}/{runs}: {package}"), &mut cmd)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(file, test name)` of every test marked `#[ignore = "timing"]` under `crates/*/tests`.
    fn marked() -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let marker = format!("#[ignore = \"{IGNORE_REASON}\"]");
        let mut out = Vec::new();
        for krate in std::fs::read_dir(root.join("crates")).expect("crates") {
            let tests = krate.expect("entry").path().join("tests");
            let Ok(files) = std::fs::read_dir(&tests) else {
                continue;
            };
            for file in files {
                let path = file.expect("entry").path();
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("test source");
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if line.trim() != marker {
                        continue;
                    }
                    let name = lines[i + 1..]
                        .iter()
                        .find_map(|l| l.trim().strip_prefix("fn "))
                        .and_then(|rest| rest.split('(').next())
                        .expect("a test after the marker");
                    let stem = path.file_stem().expect("stem").to_string_lossy();
                    out.push((stem.into_owned(), name.to_owned()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn timing_list_matches_the_marked_tests() {
        let mut listed: Vec<(String, String)> = TESTS
            .iter()
            .map(|t| (t.target.to_owned(), t.name.to_owned()))
            .collect();
        listed.sort();
        assert_eq!(marked(), listed, "update xtask/src/timing.rs TESTS");
    }

    #[test]
    fn ci_runs_the_timing_pass_after_the_tests() {
        let main =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
                .expect("main.rs");
        let ci = main.split("fn ci(").nth(1).expect("fn ci");
        let tests = ci
            .find("\"--workspace\", \"--no-fail-fast\"")
            .expect("cargo test");
        let timing = ci.find("timing::run(").expect("the timing pass");
        assert!(tests < timing);
    }
}
