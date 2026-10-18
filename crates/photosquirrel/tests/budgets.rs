//! ADR-0026 app budgets: cold start and idle memory, measured on the release binary like
//! `cargo xtask bench --check` does. They open a real window, so they run only with
//! `PHOTOSQUIRREL_GUI_TESTS=1` and a display (otherwise they print why they skipped):
//!
//! ```sh
//! PHOTOSQUIRREL_GUI_TESTS=1 cargo test --release -p photosquirrel --test budgets -- --test-threads=1 --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

fn gui_tests_enabled() -> bool {
    let enabled = std::env::var_os("PHOTOSQUIRREL_GUI_TESTS").is_some();
    if !enabled {
        eprintln!("skipping: set PHOTOSQUIRREL_GUI_TESTS=1 to launch the app (opens a window)");
    }
    enabled
}

fn data_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("photosquirrel-budget-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run the app with `args` and read its `measure <name>=` line.
fn measure(args: &[&str], name: &str) -> f64 {
    let dir = data_dir(name);
    let out = Command::new(env!("CARGO_BIN_EXE_photosquirrel"))
        .arg("--data-dir")
        .arg(&dir)
        .arg("--no-tour")
        .args(args)
        .output()
        .expect("launch photosquirrel");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("measure {name}=")))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no `measure {name}` line in: {stdout}"))
}

#[test]
fn scenario_app_shell_cold_start_within_400_ms() {
    if !gui_tests_enabled() {
        return;
    }
    let mut runs: Vec<f64> = (0..10)
        .map(|_| measure(&["--measure-startup"], "cold-start"))
        .collect();
    runs.sort_by(f64::total_cmp);
    let median = runs[runs.len() / 2];
    println!("measure cold-start={median:.1} ms (runs {runs:?})");
    assert!(
        median <= 400.0,
        "median time to first frame {median:.1} ms exceeds 400 ms"
    );
}

#[test]
fn scenario_app_shell_idle_memory_within_250_mb() {
    if !gui_tests_enabled() {
        return;
    }
    let mb = measure(&["--measure-idle-memory", "5"], "idle-rss");
    println!("measure idle-rss={mb:.1} MB");
    assert!(
        mb <= 250.0,
        "idle resident memory {mb:.1} MB exceeds 250 MB"
    );
}
