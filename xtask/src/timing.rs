//! `cargo xtask timing [--runs N]`: the timing pass. Tests that assert a wall-clock or frame-time budget are
//! `#[ignore = "timing"]`, so the main `cargo test` pass skips them; this pass runs exactly them, one test at a time
//! (`--test-threads=1`), after the main pass and with nothing else of CI running (ADR-0075, ADR-0076).
//! `cargo xtask ci` runs it once after `cargo test`.
//!
//! The pass selects its tests with `--workspace` and `--test <target>` filters, the main pass's own package
//! selection, so cargo resolves the same features and reuses the main pass's build: at most the test binaries
//! compile (ADR-0076). `cargo test -p <package>` would resolve features for that package alone and build the
//! dependency graph a second time.

use std::path::Path;

use anyhow::{Result, bail};

/// The `ignore` reason that marks a timing test.
pub const IGNORE_REASON: &str = "timing";

/// One timing test: package, test target, test name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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
    TimingTest {
        package: "ps-scan",
        target: "control",
        name: "scenario_scan_control_cancel_within_250_ms",
    },
    TimingTest {
        package: "ps-scan",
        target: "control",
        name: "scenario_scan_control_pause_within_250_ms",
    },
    TimingTest {
        package: "ps-scan",
        target: "control",
        name: "cancel_reaches_a_long_tiff_decode_within_250_ms",
    },
    TimingTest {
        package: "ps-scan",
        target: "control",
        name: "cancel_reaches_a_hung_avif_decode_within_250_ms",
    },
    TimingTest {
        package: "photosquirrel-cli",
        target: "interrupt",
        name: "scenario_cli_ctrl_c_cancels_scan",
    },
    TimingTest {
        package: "photosquirrel-cli",
        target: "power",
        name: "scenario_cli_ctrl_c_while_paused_on_battery",
    },
];

/// The `cargo` arguments of one timing pass: the main pass's package selection (`--workspace`), the timing tests'
/// targets, and their exact names, ignored ones included, one at a time.
pub fn cargo_args(tests: &[TimingTest]) -> Vec<String> {
    let mut args: Vec<String> = ["test", "--workspace"].map(str::to_owned).to_vec();
    let mut targets: Vec<&str> = tests.iter().map(|t| t.target).collect();
    targets.sort_unstable();
    targets.dedup();
    for target in targets {
        args.push("--test".into());
        args.push(target.into());
    }
    args.extend(
        ["--", "--ignored", "--exact", "--test-threads=1"]
            .iter()
            .map(|s| (*s).to_owned()),
    );
    args.extend(tests.iter().map(|t| t.name.to_owned()));
    args
}

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
        let mut cmd = crate::cargo(root)?;
        cmd.args(cargo_args(TESTS));
        crate::run(&format!("timing pass {i}/{runs}"), &mut cmd)?;
    }
    Ok(())
}

/// A test marked `#[ignore = "timing"]` in the sources: package, test target, test name.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Marked {
    package: String,
    target: String,
    name: String,
}

/// The timing markers in one source file of `package`, at `rel` (relative to the crate, `/`-separated).
///
/// The timing pass runs `--test <target> -- --exact <name>`, so a marker is only valid on an unindented test
/// function of a top-level integration test file (`tests/<target>.rs`). Any other `ignore = "timing"` (in `src/`,
/// a nested `tests/` directory, a module, behind `cfg_attr`, or spelt differently) is an error: such a test would
/// be skipped by the main pass and never run by this one.
#[cfg(test)]
fn markers_in(package: &str, rel: &str, text: &str) -> Result<Vec<Marked>, Vec<String>> {
    let marker = format!("#[ignore = \"{IGNORE_REASON}\"]");
    let needle = format!("ignore=\"{IGNORE_REASON}\"");
    let target = rel
        .strip_prefix("tests/")
        .and_then(|f| f.strip_suffix(".rs"))
        .filter(|stem| !stem.contains('/'));
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if !squeezed.contains(&needle) {
            continue;
        }
        let at = format!("crates/{package}/{rel}:{}", i + 1);
        if line.trim_end() != marker {
            errors.push(format!(
                "{at}: `{}` is not the exact line `{marker}` (unindented, on its own, no cfg_attr)",
                line.trim()
            ));
            continue;
        }
        let Some(target) = target else {
            errors.push(format!(
                "{at}: timing tests must live in a top-level integration test file (crates/{package}/tests/<target>.rs)"
            ));
            continue;
        };
        // The test function: the next line that is not an attribute or a comment.
        let name = lines[i + 1..]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with("#[") && !l.starts_with("//"))
            .and_then(|l| l.strip_prefix("fn "))
            .and_then(|rest| rest.split('(').next())
            .filter(|n| !n.is_empty());
        match name {
            Some(name) => found.push(Marked {
                package: package.to_owned(),
                target: target.to_owned(),
                name: name.to_owned(),
            }),
            None => errors.push(format!("{at}: no test function follows the marker")),
        }
    }
    if errors.is_empty() {
        Ok(found)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
    }

    /// Every `.rs` file under `dir`, recursively (`target` directories skipped).
    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "target") {
                    rust_files(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Every timing marker in every crate's sources (`src/`, `tests/`, `benches/`, `examples/`, nested
    /// directories included); panics listing every marker the timing pass could not run.
    fn marked() -> Vec<Marked> {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        for krate in std::fs::read_dir(root().join("crates")).expect("crates") {
            let dir = krate.expect("entry").path();
            let Ok(manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
                continue;
            };
            let manifest: toml::Value = toml::from_str(&manifest).expect("Cargo.toml");
            let package = manifest["package"]["name"]
                .as_str()
                .expect("package name")
                .to_owned();
            let mut files = Vec::new();
            rust_files(&dir, &mut files);
            for path in files {
                let rel = path
                    .strip_prefix(&dir)
                    .expect("under the crate")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let text = std::fs::read_to_string(&path).expect("source");
                match markers_in(&package, &rel, &text) {
                    Ok(found) => out.extend(found),
                    Err(e) => errors.extend(e),
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        out.sort();
        out
    }

    #[test]
    fn timing_list_matches_the_marked_tests() {
        let mut listed: Vec<Marked> = TESTS
            .iter()
            .map(|t| Marked {
                package: t.package.to_owned(),
                target: t.target.to_owned(),
                name: t.name.to_owned(),
            })
            .collect();
        listed.sort();
        assert_eq!(marked(), listed, "update xtask/src/timing.rs TESTS");
    }

    #[test]
    fn markers_found_in_top_level_test_files() {
        let text = "#[test]\n#[ignore = \"timing\"]\nfn a() {}\n\n#[gpui_kit::test]\n// why\n\
                    #[ignore = \"timing\"]\n#[cfg(unix)]\nfn b(cx: &mut T) {}\n#[ignore = \"slow\"]\nfn c() {}\n";
        let found = markers_in("p", "tests/t.rs", text).expect("valid markers");
        let names: Vec<&str> = found.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(found.iter().all(|m| m.package == "p" && m.target == "t"));
    }

    #[test]
    fn markers_the_pass_cannot_run_fail() {
        let plain = "#[test]\n#[ignore = \"timing\"]\nfn a() {}\n";
        // In src/ (a unit test), a nested tests directory or a helper module of the tests.
        for rel in [
            "src/lib.rs",
            "src/a/b.rs",
            "tests/support/mod.rs",
            "tests/x/main.rs",
        ] {
            let err = markers_in("p", rel, plain).expect_err(rel);
            assert!(
                err[0].contains("top-level integration test file"),
                "{rel}: {err:?}"
            );
            assert!(err[0].contains(&format!("crates/p/{rel}:2")), "{err:?}");
        }
        // Spelt differently, behind cfg_attr, indented (inside a module) or sharing its line.
        for text in [
            "#[test]\n#[ignore=\"timing\"]\nfn a() {}\n",
            "#[test]\n#[cfg_attr(not(feature = \"x\"), ignore = \"timing\")]\nfn a() {}\n",
            "mod m {\n    #[test]\n    #[ignore = \"timing\"]\n    fn a() {}\n}\n",
            "#[test] #[ignore = \"timing\"] fn a() {}\n",
            "#[test]\n#[ignore = \"timing\" ]\nfn a() {}\n",
        ] {
            let err = markers_in("p", "tests/t.rs", text).expect_err(text);
            assert!(err[0].contains("is not the exact line"), "{text}: {err:?}");
        }
        // A marker with no test after it.
        let err = markers_in(
            "p",
            "tests/t.rs",
            "#[ignore = \"timing\"]\nconst X: u8 = 1;\n",
        )
        .expect_err("no fn");
        assert!(err[0].contains("no test function"), "{err:?}");
        // Other ignore reasons are not timing markers.
        assert_eq!(
            markers_in("p", "src/lib.rs", "#[ignore = \"slow\"]\nfn a() {}\n"),
            Ok(Vec::new())
        );
    }

    /// The pass selects packages as the main pass does (`--workspace`, never `-p`), so cargo resolves the same
    /// features and rebuilds nothing (ADR-0076), and names every listed test exactly.
    #[test]
    fn timing_pass_uses_the_main_pass_package_selection() {
        let args = cargo_args(TESTS);
        assert_eq!(&args[..2], ["test", "--workspace"]);
        assert!(!args.iter().any(|a| a == "-p" || a == "--package"));
        let sep = args.iter().position(|a| a == "--").expect("--");
        for t in TESTS {
            assert!(
                args[..sep]
                    .windows(2)
                    .any(|w| w[0] == "--test" && w[1] == t.target),
                "{}",
                t.target
            );
            assert!(args[sep..].iter().any(|a| a == t.name), "{}", t.name);
        }
        for flag in ["--ignored", "--exact", "--test-threads=1"] {
            assert!(args[sep..].iter().any(|a| a == flag), "{flag}");
        }
        // Each target once.
        let targets = args[..sep].iter().filter(|a| *a == "--test").count();
        let mut unique: Vec<&str> = TESTS.iter().map(|t| t.target).collect();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(targets, unique.len());
    }

    #[test]
    fn ci_runs_the_timing_pass_after_the_tests() {
        let main = std::fs::read_to_string(root().join("xtask/src/main.rs")).expect("main.rs");
        let ci = main.split("fn ci(").nth(1).expect("fn ci");
        let tests = ci
            .find("\"--workspace\", \"--no-fail-fast\"")
            .expect("cargo test");
        let timing = ci.find("timing::run(").expect("the timing pass");
        assert!(tests < timing);
    }

    /// The `-p` packages of a `cargo test` command line.
    fn packages(line: &str) -> Vec<&str> {
        let words: Vec<&str> = line.split_whitespace().collect();
        words
            .windows(2)
            .filter(|w| w[0] == "-p")
            .map(|w| w[1])
            .collect()
    }

    /// The macOS and Windows jobs test the engine crates only; they run those crates' timing tests alone, after
    /// their main tests, with the same packages (the same features) and the same test scale.
    #[test]
    fn platform_jobs_run_the_engine_timing_tests_alone() {
        let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).expect("ci.yml");
        let build = ci.split("\n  build:\n").nth(1).expect("the build job");
        let steps: Vec<&str> = build.split("\n      - ").collect();
        let main = steps
            .iter()
            .position(|s| s.contains("cargo test") && !s.contains("--ignored"))
            .expect("the main test step");
        let timing = steps
            .iter()
            .position(|s| s.contains("cargo test") && s.contains("--ignored"))
            .expect("the timing step");
        assert!(main < timing, "the timing step runs after the main tests");
        let (main, timing) = (steps[main], steps[timing]);
        let cmd = |step: &str| {
            step.lines()
                .find(|l| l.contains("cargo test"))
                .expect("cargo test line")
                .to_owned()
        };
        let (main_cmd, timing_cmd) = (cmd(main), cmd(timing));
        let engine = packages(&main_cmd);
        assert!(!engine.is_empty());
        assert_eq!(
            packages(&timing_cmd),
            engine,
            "same packages, same features"
        );
        for step in [main, timing] {
            assert!(step.contains("PHOTOSQUIRREL_TEST_SCALE: reduced"), "{step}");
        }
        let sep = timing_cmd.find(" -- ").expect("test harness args");
        let (cargo, harness) = timing_cmd.split_at(sep);
        for flag in ["--ignored", "--exact", "--test-threads=1"] {
            assert!(harness.contains(flag), "{flag} missing: {timing_cmd}");
        }
        let words: Vec<&str> = harness.split_whitespace().collect();
        for t in TESTS {
            let listed = words.contains(&t.name);
            if engine.contains(&t.package) {
                assert!(listed, "{} missing from the timing step", t.name);
                assert!(
                    cargo.contains(&format!("--test {}", t.target)),
                    "{} missing from the timing step",
                    t.target
                );
            } else {
                assert!(!listed, "{} is not an engine test", t.name);
            }
        }
    }
}
