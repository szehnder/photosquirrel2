//! `cargo xtask bench [--list] [--check] [--ci] [--update-baseline]` (ADR-0026).
//!
//! Every budget in `perf-budgets.toml` names its `source`:
//! All budgets are lower-is-better (express throughput as time per batch, e.g. ms per 1,000 files).
//! - `test`: an integration test that prints `measure <budget>=<value> <unit>` (run in release);
//! - `criterion`: a criterion benchmark id, read from `target/criterion/<id>/new/estimates.json` (median);
//! - `app`: the release app with measurement flags (`--measure-startup`, `--measure-idle-memory`,
//!   `--render-bench`). These open windows, so they need a display and are skipped by `--ci`.
//!
//! `--check` compares each measurement with its absolute budget (locally) and with `perf-baseline.json`
//! (more than [`REGRESSION`] slower fails). `--ci` checks only the relative regressions and skips app
//! measurements: CI runners are noisy and have no display. `--update-baseline` writes the measurements as the
//! new baseline.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// A measurement this much above its baseline is a regression.
pub const REGRESSION: f64 = 0.15;
pub const BASELINE_FILE: &str = "perf-baseline.json";

#[derive(Debug, Deserialize)]
pub struct Budgets {
    #[serde(default)]
    pub budget: Vec<Budget>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Budget {
    pub name: String,
    pub metric: String,
    pub max: f64,
    pub unit: String,
    pub change: String,
    pub source: Source,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// `cargo test --release -p <package> --test <test> -- --exact <name>`.
    Test {
        package: String,
        test: String,
        name: String,
    },
    /// `cargo bench -p <package> --bench <bench>`, then the median of `<id>`.
    Criterion {
        package: String,
        bench: String,
        id: String,
    },
    /// The release app with `args`, `runs` times; the median of the `measure <budget>` lines. For the
    /// render benchmark, `p95` reads the frame-interval p95 instead.
    App {
        args: Vec<String>,
        #[serde(default = "one")]
        runs: usize,
        /// The `measure` line to read (default: the budget's name).
        #[serde(default)]
        measure: Option<String>,
        /// Seed the data directory with the demo library (`photosquirrel-cli demo`) first.
        #[serde(default)]
        seeded: bool,
        #[serde(default)]
        render_p95: bool,
    },
}

fn one() -> usize {
    1
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    pub value: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Baseline {
    /// Budget name → value, in the budget's unit.
    pub values: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Absolute budgets and baseline regressions (the reference machine).
    Local,
    /// Baseline regressions only.
    Ci,
}

/// One failed check, worded for the terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation(pub String);

/// Compare measurements with budgets and the baseline. Pure, so it is tested without running anything.
pub fn evaluate(
    budgets: &[Budget],
    measured: &BTreeMap<String, Measurement>,
    baseline: &Baseline,
    mode: Mode,
) -> Vec<Violation> {
    let mut out = Vec::new();
    for b in budgets {
        let Some(m) = measured.get(&b.name) else {
            continue;
        };
        if mode == Mode::Local && m.value > b.max {
            out.push(Violation(format!(
                "{}: {:.2} {} exceeds the budget of {} {} ({})",
                b.name, m.value, b.unit, b.max, b.unit, b.metric
            )));
        }
        if let Some(&base) = baseline.values.get(&b.name)
            && base > 0.0
            && m.value > base * (1.0 + REGRESSION)
        {
            out.push(Violation(format!(
                "{}: {:.2} {} is {:.0}% worse than the baseline {:.2} {} (limit {:.0}%)",
                b.name,
                m.value,
                b.unit,
                (m.value / base - 1.0) * 100.0,
                base,
                b.unit,
                REGRESSION * 100.0
            )));
        }
    }
    out
}

/// `measure <name>=<value> <unit>` lines in `output`.
pub fn parse_measure_lines(output: &str) -> BTreeMap<String, f64> {
    output
        .lines()
        .filter_map(|l| l.trim().strip_prefix("measure "))
        .filter_map(|rest| {
            let (name, value) = rest.split_once('=')?;
            let value: f64 = value.split_whitespace().next()?.parse().ok()?;
            Some((name.trim().to_owned(), value))
        })
        .collect()
}

/// The render benchmark's frame-interval p95, from `frame interval ms: p50=.. p95=..`.
pub fn parse_render_p95(output: &str) -> Option<f64> {
    output
        .lines()
        .find(|l| l.contains("frame interval ms"))?
        .split("p95=")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

pub fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

pub struct Options {
    pub list: bool,
    pub check: bool,
    pub ci: bool,
    pub update_baseline: bool,
    /// Only these budgets (all when empty).
    pub only: Vec<String>,
}

pub fn run(root: &Path, opts: Options) -> Result<()> {
    let text = std::fs::read_to_string(root.join("perf-budgets.toml"))?;
    let budgets: Budgets = toml::from_str(&text).context("parsing perf-budgets.toml")?;
    if opts.list || !(opts.check || opts.update_baseline) {
        for b in &budgets.budget {
            println!(
                "{:<28} {:<7} <= {:>8} {:<3} ({})",
                b.name, b.metric, b.max, b.unit, b.change
            );
        }
        return Ok(());
    }
    let mode = if opts.ci { Mode::Ci } else { Mode::Local };
    let selected: Vec<&Budget> = budgets
        .budget
        .iter()
        .filter(|b| opts.only.is_empty() || opts.only.contains(&b.name))
        .filter(|b| !(opts.ci && matches!(b.source, Source::App { .. })))
        .collect();

    let mut measured = BTreeMap::new();
    let needs_app = selected
        .iter()
        .any(|b| matches!(b.source, Source::App { .. }));
    if needs_app {
        crate::run(
            "build release app",
            Command::new("cargo").args([
                "build",
                "--release",
                "-p",
                "photosquirrel",
                "-p",
                "photosquirrel-cli",
            ]),
        )?;
    }
    for b in &selected {
        let started = Instant::now();
        match measure(root, b) {
            Ok(Some(value)) => {
                eprintln!(
                    "    {:<28} {:>10.2} {:<3}  (budget {} {}, {:.0} s)",
                    b.name,
                    value,
                    b.unit,
                    b.max,
                    b.unit,
                    started.elapsed().as_secs_f32()
                );
                measured.insert(b.name.clone(), Measurement { value });
            }
            Ok(None) => eprintln!("    {:<28} no measurement printed", b.name),
            Err(err) => eprintln!("    {:<28} failed: {err:#}", b.name),
        }
    }

    let baseline_path = root.join(BASELINE_FILE);
    if opts.update_baseline {
        let mut baseline: Baseline = read_baseline(&baseline_path)?;
        for (name, m) in &measured {
            baseline.values.insert(name.clone(), m.value);
        }
        std::fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&baseline)? + "\n",
        )?;
        println!("wrote {}", baseline_path.display());
        if !opts.check {
            return Ok(());
        }
    }
    let baseline = read_baseline(&baseline_path)?;
    let violations = evaluate(&budgets.budget, &measured, &baseline, mode);
    let missing: Vec<&str> = selected
        .iter()
        .filter(|b| !measured.contains_key(&b.name))
        .map(|b| b.name.as_str())
        .collect();
    for v in &violations {
        eprintln!("FAIL {}", v.0);
    }
    if !missing.is_empty() {
        eprintln!("FAIL no measurement for: {}", missing.join(", "));
    }
    if !violations.is_empty() || !missing.is_empty() {
        bail!(
            "{} budget check(s) failed",
            violations.len() + missing.len()
        );
    }
    println!("all {} budgets met", measured.len());
    Ok(())
}

fn read_baseline(path: &Path) -> Result<Baseline> {
    if !path.exists() {
        return Ok(Baseline::default());
    }
    serde_json::from_str(&std::fs::read_to_string(path)?)
        .with_context(|| format!("parsing {}", path.display()))
}

fn measure(root: &Path, b: &Budget) -> Result<Option<f64>> {
    match &b.source {
        Source::Test {
            package,
            test,
            name,
        } => {
            let out = Command::new("cargo")
                .args(["test", "--release", "-q", "-p", package, "--test", test])
                .args(["--", "--exact", name, "--nocapture", "--test-threads=1"])
                .stderr(Stdio::inherit())
                .output()?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !out.status.success() {
                // The test's own assertion may have failed; the measurement still counts.
                eprintln!("    ({name} failed)");
            }
            Ok(parse_measure_lines(&stdout).get(&b.name).copied())
        }
        Source::Criterion { package, bench, id } => {
            crate::run(
                &format!("bench {package}/{bench}"),
                Command::new("cargo").args([
                    "bench", "-q", "-p", package, "--bench", bench, "--", "--noplot", id,
                ]),
            )?;
            let path = root
                .join("target/criterion")
                .join(id)
                .join("new/estimates.json");
            let json: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
            )?;
            let ns = json["median"]["point_estimate"]
                .as_f64()
                .context("median in estimates.json")?;
            Ok(Some(ns / 1e6))
        }
        Source::App {
            args,
            runs,
            measure,
            seeded,
            render_p95,
        } => {
            let key = measure.as_deref().unwrap_or(&b.name);
            let bin = root.join("target/release/photosquirrel");
            let data = root.join("target/perf-data");
            let mut values = Vec::new();
            for _ in 0..*runs {
                let _ = std::fs::remove_dir_all(&data);
                std::fs::create_dir_all(&data)?;
                if *seeded {
                    crate::run(
                        "seed the demo library",
                        Command::new(root.join("target/release/photosquirrel-cli"))
                            .arg("--data-dir")
                            .arg(&data)
                            .arg("demo"),
                    )?;
                }
                let out = Command::new(&bin)
                    .arg("--data-dir")
                    .arg(&data)
                    .arg("--no-tour")
                    .args(args)
                    .stderr(Stdio::null())
                    .output()
                    .context("launching the app")?;
                let stdout = String::from_utf8_lossy(&out.stdout);
                let value = if *render_p95 {
                    parse_render_p95(&stdout)
                } else {
                    parse_measure_lines(&stdout).get(key).copied()
                };
                values.extend(value);
            }
            let _ = std::fs::remove_dir_all(&data);
            Ok(match b.metric.as_str() {
                "max" => values.into_iter().reduce(f64::max),
                _ => median(values),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(name: &str, max: f64) -> Budget {
        Budget {
            name: name.into(),
            metric: "median".into(),
            max,
            unit: "ms".into(),
            change: "x".into(),
            source: Source::App {
                args: vec![],
                runs: 1,
                measure: None,
                seeded: false,
                render_p95: false,
            },
        }
    }

    fn measured(pairs: &[(&str, f64)]) -> BTreeMap<String, Measurement> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), Measurement { value: *v }))
            .collect()
    }

    #[test]
    fn scenario_performance_budgets_regression_detected() {
        let budgets = [budget("graphson_decode_10mb", 60.0), budget("steady", 60.0)];
        let mut baseline = Baseline::default();
        baseline.values.insert("graphson_decode_10mb".into(), 40.0);
        baseline.values.insert("steady".into(), 40.0);
        // 20% slower than the baseline, still inside the absolute budget.
        let m = measured(&[("graphson_decode_10mb", 48.0), ("steady", 42.0)]);
        let v = evaluate(&budgets, &m, &baseline, Mode::Ci);
        assert_eq!(v.len(), 1, "{v:?}");
        let msg = &v[0].0;
        assert!(
            msg.contains("graphson_decode_10mb"),
            "names the benchmark: {msg}"
        );
        assert!(
            msg.contains("48.00") && msg.contains("40.00"),
            "both values: {msg}"
        );
        assert!(msg.contains("20%"), "{msg}");
    }

    #[test]
    fn absolute_budgets_apply_locally_only() {
        let budgets = [budget("cold_start_first_frame", 300.0)];
        let m = measured(&[("cold_start_first_frame", 450.0)]);
        let none = Baseline::default();
        assert_eq!(evaluate(&budgets, &m, &none, Mode::Local).len(), 1);
        assert!(evaluate(&budgets, &m, &none, Mode::Ci).is_empty());
    }

    #[test]
    fn parses_measure_and_render_lines() {
        let out = "noise\nmeasure first_frame_ms=212.4 ms\nmeasure idle_rss=98.0 MB\n";
        let m = parse_measure_lines(out);
        assert_eq!(m["first_frame_ms"], 212.4);
        assert_eq!(m["idle_rss"], 98.0);
        let render = "  frame interval ms: p50=6.94 p95=7.10 p99=7.5 max=9.0";
        assert_eq!(parse_render_p95(render), Some(7.10));
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
    }

    #[test]
    fn budgets_file_parses_with_sources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let text = std::fs::read_to_string(root.join("perf-budgets.toml")).unwrap();
        let budgets: Budgets = toml::from_str(&text).unwrap();
        assert!(budgets.budget.iter().all(|b| b.max > 0.0));
    }
}
