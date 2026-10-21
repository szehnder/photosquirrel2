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
//! (more than [`REGRESSION`] slower, by more than [`NOISE_FLOOR`] of the bound, fails); a selected budget that printed no measurement fails too. `--ci`
//! checks only the relative regressions and skips app measurements: CI runners are noisy, differ from the
//! reference machine and have no display. In CI a budget that looks regressed is measured again
//! ([`CI_REMEASURES`] times) and its best run counts; `--best-of <n>` measures every budget `n` times and keeps
//! the best, which CI uses for the base commit so base and head get the same best-of-three policy (a noisy-high
//! base would otherwise hide a regression). `--update-baseline` merges the measurements into the baseline
//! (budgets not measured keep their values); with `--check` it writes only when the check passed, so the
//! comparison is always against the previous baseline. `--baseline <file>` uses another baseline file (CI
//! compares with the parent commit measured on the same runner, not with the committed reference-machine
//! `perf-baseline.json`). A check with `--ci` or `--baseline` fails when that file is missing, and lists the
//! selected budgets it has no value for (a `::warning::` on GitHub Actions): those are compared with nothing.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// A measurement this much above its baseline is a regression...
pub const REGRESSION: f64 = 0.15;
/// ...when it is also above the baseline by more than this share of the budget's bound (ADR-0057): runners
/// vary run to run by more than 15% on budgets far below their bound (control latency measured 53-84 ms on
/// the same code against 250 ms), and a difference that small is noise, not a regression.
pub const NOISE_FLOOR: f64 = 0.05;

/// Whether `value` regressed from `base` for budget `b`: see [`REGRESSION`] and [`NOISE_FLOOR`].
pub fn is_regression(b: &Budget, base: f64, value: f64) -> bool {
    base > 0.0 && value > base * (1.0 + REGRESSION) && value - base > b.max * NOISE_FLOOR
}
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

/// Compare measurements with the selected budgets and the baseline. Pure, so it is tested without running
/// anything. A budget without a measurement is a violation: a source that silently stops printing must fail.
pub fn evaluate(
    budgets: &[&Budget],
    measured: &BTreeMap<String, Measurement>,
    baseline: &Baseline,
    mode: Mode,
) -> Vec<Violation> {
    let mut out = Vec::new();
    for b in budgets {
        let Some(m) = measured.get(&b.name) else {
            out.push(Violation(format!(
                "{}: no measurement (its {} source printed no `measure {}=` line)",
                b.name,
                b.source.kind(),
                b.name
            )));
            continue;
        };
        if mode == Mode::Local && m.value > b.max {
            out.push(Violation(format!(
                "{}: {:.2} {} exceeds the budget of {} {} ({})",
                b.name, m.value, b.unit, b.max, b.unit, b.metric
            )));
        }
        if let Some(&base) = baseline.values.get(&b.name)
            && is_regression(b, base, m.value)
        {
            out.push(Violation(format!(
                "{}: {:.2} {} is {:.0}% worse than the baseline {:.2} {} (limit {:.0}%, and more than {:.0}% of the budget)",
                b.name,
                m.value,
                b.unit,
                (m.value / base - 1.0) * 100.0,
                base,
                b.unit,
                REGRESSION * 100.0,
                NOISE_FLOOR * 100.0
            )));
        }
    }
    out
}

/// The budgets a run measures: those named by `only` (all when empty), without app (window) measurements in
/// CI mode. Unknown names are an error, so a typo does not pass as "all budgets met".
pub fn select<'a>(budgets: &'a [Budget], only: &[String], mode: Mode) -> Result<Vec<&'a Budget>> {
    if let Some(unknown) = only.iter().find(|n| !budgets.iter().any(|b| &b.name == *n)) {
        bail!("no budget named {unknown:?} in perf-budgets.toml (see `cargo xtask bench --list`)");
    }
    Ok(budgets
        .iter()
        .filter(|b| only.is_empty() || only.contains(&b.name))
        .filter(|b| !(mode == Mode::Ci && matches!(b.source, Source::App { .. })))
        .collect())
}

/// How many times `--check --ci` re-measures a budget that looks regressed before it counts as one.
pub const CI_REMEASURES: usize = 2;

/// The selected budgets that regressed from their baseline ([`is_regression`]).
pub fn regressed<'a>(
    budgets: &[&'a Budget],
    measured: &BTreeMap<String, Measurement>,
    baseline: &Baseline,
) -> Vec<&'a Budget> {
    budgets
        .iter()
        .copied()
        .filter(|b| {
            let base = baseline.values.get(&b.name).copied().unwrap_or(0.0);
            measured
                .get(&b.name)
                .is_some_and(|m| is_regression(b, base, m.value))
        })
        .collect()
}

/// Keep the better (lower) of the recorded and a repeated measurement.
pub fn best_of(measured: &mut BTreeMap<String, Measurement>, name: &str, value: f64) {
    let m = measured
        .entry(name.to_owned())
        .or_insert(Measurement { value });
    m.value = m.value.min(value);
}

/// The selected budgets the baseline has no value for: nothing to compare them with (newly added budgets, or a
/// base commit whose source printed nothing).
pub fn missing_base<'a>(budgets: &[&'a Budget], baseline: &Baseline) -> Vec<&'a str> {
    budgets
        .iter()
        .filter(|b| !baseline.values.get(&b.name).is_some_and(|v| *v > 0.0))
        .map(|b| b.name.as_str())
        .collect()
}

/// Merge `measured` into `baseline`: measured budgets get their new values, the others keep theirs.
pub fn merge_baseline(baseline: &mut Baseline, measured: &BTreeMap<String, Measurement>) {
    for (name, m) in measured {
        baseline.values.insert(name.clone(), m.value);
    }
}

/// One line per budget for `--list`: name, metric, bound, unit, owning change and how it is measured.
pub fn list_lines(budgets: &[Budget]) -> Vec<String> {
    budgets
        .iter()
        .map(|b| {
            format!(
                "{:<26} {:<6} <= {:>7} {:<2}  {:<17} {}",
                b.name,
                b.metric,
                b.max,
                b.unit,
                b.change,
                b.source.describe()
            )
        })
        .collect()
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Test { .. } => "test",
            Source::Criterion { .. } => "criterion",
            Source::App { .. } => "app",
        }
    }

    /// Where the measurement comes from, for `--list` and docs/performance.md.
    pub fn describe(&self) -> String {
        match self {
            Source::Test { package, test, .. } => format!("test {package}/{test}"),
            Source::Criterion { package, bench, id } => format!("criterion {package}/{bench} {id}"),
            Source::App { args, .. } => format!("app {}", args.join(" ")),
        }
    }
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
    /// The baseline file (default: [`BASELINE_FILE`] at the workspace root).
    pub baseline: Option<std::path::PathBuf>,
    /// Measure every selected budget this many times and keep the best (at least once).
    pub best_of: usize,
}

pub fn run(root: &Path, opts: Options) -> Result<()> {
    let text = std::fs::read_to_string(root.join("perf-budgets.toml"))?;
    let budgets: Budgets = toml::from_str(&text).context("parsing perf-budgets.toml")?;
    if opts.list || !(opts.check || opts.update_baseline) {
        for line in list_lines(&budgets.budget) {
            println!("{line}");
        }
        return Ok(());
    }
    let mode = if opts.ci { Mode::Ci } else { Mode::Local };
    let selected = select(&budgets.budget, &opts.only, mode)?;

    let mut measured = BTreeMap::new();
    let needs_app = selected
        .iter()
        .any(|b| matches!(b.source, Source::App { .. }));
    if needs_app {
        crate::run(
            "build release app",
            crate::cargo(root)?.args([
                "build",
                "--release",
                "-p",
                "photosquirrel",
                "-p",
                "photosquirrel-cli",
            ]),
        )?;
    }
    let baseline_path = opts
        .baseline
        .clone()
        .unwrap_or_else(|| root.join(BASELINE_FILE));
    // Read before measuring, so a missing base fails in seconds, not after every measurement.
    let mut baseline = if opts.check && (mode == Mode::Ci || opts.baseline.is_some()) {
        read_baseline_required(&baseline_path)?
    } else {
        read_baseline(&baseline_path)?
    };
    for b in &selected {
        for run in 0..opts.best_of.max(1) {
            let started = Instant::now();
            match measure(root, b) {
                Ok(Some(value)) => {
                    eprintln!(
                        "    {:<28} {:>10.2} {:<3}  (budget {} {}, {:.0} s{})",
                        b.name,
                        value,
                        b.unit,
                        b.max,
                        b.unit,
                        started.elapsed().as_secs_f32(),
                        if run > 0 { ", repeat" } else { "" }
                    );
                    best_of(&mut measured, &b.name, value);
                }
                Ok(None) => eprintln!("    {:<28} no measurement printed", b.name),
                Err(err) => eprintln!("    {:<28} failed: {err:#}", b.name),
            }
        }
    }

    if opts.check {
        let missing = missing_base(&selected, &baseline);
        if !missing.is_empty() {
            let msg = format!(
                "{} has no value for {}: not compared (a new budget, or the base printed no measurement)",
                baseline_path.display(),
                missing.join(", ")
            );
            if std::env::var_os("GITHUB_ACTIONS").is_some() {
                println!("::warning title=bench baseline::{msg}");
            }
            eprintln!("WARN {msg}");
        }
    }
    if opts.check && mode == Mode::Ci {
        // Shared runners are noisy: a budget that looks regressed is measured again, and its best run counts.
        for _ in 0..CI_REMEASURES {
            let suspects = regressed(&selected, &measured, &baseline);
            for b in suspects {
                if let Ok(Some(value)) = measure(root, b) {
                    eprintln!(
                        "    {:<28} {:>10.2} {:<3}  (re-measured)",
                        b.name, value, b.unit
                    );
                    best_of(&mut measured, &b.name, value);
                }
            }
        }
    }
    if opts.check {
        let violations = evaluate(&selected, &measured, &baseline, mode);
        for v in &violations {
            eprintln!("FAIL {}", v.0);
        }
        if !violations.is_empty() {
            bail!(
                "{} budget check(s) failed{}",
                violations.len(),
                if opts.update_baseline {
                    "; baseline not updated"
                } else {
                    ""
                }
            );
        }
        let scope = if mode == Mode::Ci {
            "within 15% of the baseline (or within the noise floor)"
        } else {
            "met"
        };
        println!("all {} budgets {scope}", measured.len());
    }
    if opts.update_baseline {
        merge_baseline(&mut baseline, &measured);
        std::fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&baseline)? + "\n",
        )?;
        println!("wrote {}", baseline_path.display());
    }
    Ok(())
}

/// The baseline at `path`; a missing file is an empty baseline (a first `--update-baseline`).
fn read_baseline(path: &Path) -> Result<Baseline> {
    if !path.exists() {
        return Ok(Baseline::default());
    }
    read_baseline_required(path)
}

/// The baseline at `path`; missing or unreadable is an error (a check against it would compare nothing).
pub fn read_baseline_required(path: &Path) -> Result<Baseline> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the baseline {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn measure(root: &Path, b: &Budget) -> Result<Option<f64>> {
    match &b.source {
        Source::Test {
            package,
            test,
            name,
        } => {
            let out = crate::cargo(root)?
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
                crate::cargo(root)?.args([
                    "bench", "-q", "-p", package, "--bench", bench, "--", "--noplot", id,
                ]),
            )?;
            let path = crate::target_dir(root)
                .join("criterion")
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
            let release = crate::screenshot::release_dir(root);
            let bin = release.join("photosquirrel");
            let data = crate::target_dir(root).join("perf-data");
            let mut values = Vec::new();
            for _ in 0..*runs {
                let _ = std::fs::remove_dir_all(&data);
                std::fs::create_dir_all(&data)?;
                if *seeded {
                    crate::run(
                        "seed the demo library",
                        Command::new(release.join("photosquirrel-cli"))
                            .arg("--data-dir")
                            .arg(&data)
                            .arg("demo"),
                    )?;
                }
                let child = Command::new(&bin)
                    .arg("--data-dir")
                    .arg(&data)
                    .arg("--no-tour")
                    .args(args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .context("launching the app")?;
                if *render_p95 {
                    // Wayland sends frames only to a visible window; a window launched from a terminal
                    // may open behind it, so raise it once it has had time to map (KDE; best effort).
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.contains("KDE")) {
                        crate::screenshot::kde_activate_app();
                    }
                }
                let out = child.wait_with_output().context("waiting for the app")?;
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
            source: Source::Test {
                package: "p".into(),
                test: "t".into(),
                name: "n".into(),
            },
        }
    }

    fn app_budget(name: &str, max: f64) -> Budget {
        Budget {
            source: Source::App {
                args: vec!["--measure-startup".into()],
                runs: 1,
                measure: None,
                seeded: false,
                render_p95: false,
            },
            ..budget(name, max)
        }
    }

    fn measured(pairs: &[(&str, f64)]) -> BTreeMap<String, Measurement> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), Measurement { value: *v }))
            .collect()
    }

    fn workspace_budgets() -> Vec<Budget> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let text = std::fs::read_to_string(root.join("perf-budgets.toml")).unwrap();
        toml::from_str::<Budgets>(&text).unwrap().budget
    }

    /// The eleven ADR-0026 budgets: name and upper bound, in the unit the budgets file uses (throughput as
    /// time per unit: 50k rows/s = 20 us/row, 150 files/s = 6.67 ms/file).
    const ADR_0026: [(&str, f64, &str); 11] = [
        ("gallery-scroll-p95", 8.3, "ms"),
        ("first-gallery-frame", 600.0, "ms"),
        ("cold-start", 400.0, "ms"),
        ("idle-rss", 250.0, "MB"),
        ("catalog-insert", 20.0, "us"),
        ("catalog-id-list-1m", 300.0, "ms"),
        ("rescan-nochange-100k", 4.0, "s"),
        ("scan-analysis-12mp", 6.67, "ms"),
        ("phash-dhash-per-image", 0.3, "ms"),
        ("near-dup-lookup-p95", 1.0, "ms"),
        ("control-latency-p95", 250.0, "ms"),
    ];

    #[test]
    fn scenario_performance_budgets_all_adr_budgets_defined() {
        let budgets = workspace_budgets();
        for (name, max, unit) in ADR_0026 {
            let found: Vec<&Budget> = budgets.iter().filter(|b| b.name == name).collect();
            assert_eq!(found.len(), 1, "exactly one entry for {name}");
            let b = found[0];
            assert!(
                b.max > 0.0 && (b.max - max).abs() < 1e-9,
                "{name}: bound {} != {max}",
                b.max
            );
            assert_eq!(b.unit, unit, "{name}");
            assert!(!b.change.is_empty(), "{name} names its owning change");
            match &b.source {
                Source::Test {
                    package,
                    test,
                    name,
                } => {
                    assert!(!package.is_empty() && !test.is_empty() && !name.is_empty())
                }
                Source::Criterion { package, bench, id } => {
                    assert!(!package.is_empty() && !bench.is_empty() && !id.is_empty())
                }
                Source::App { args, runs, .. } => assert!(!args.is_empty() && *runs > 0),
            }
        }
        let mut names: Vec<&str> = budgets.iter().map(|b| b.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), budgets.len(), "budget names are unique");
        assert!(budgets.iter().all(|b| b.max > 0.0));
    }

    #[test]
    fn scenario_performance_budgets_list_shows_every_budget() {
        let budgets = workspace_budgets();
        let lines = list_lines(&budgets);
        assert_eq!(lines.len(), budgets.len(), "one line per budget");
        for (line, b) in lines.iter().zip(&budgets) {
            assert!(line.starts_with(&b.name), "{line}");
            assert!(
                line.contains(&format!("<= {:>7} {:<2}", b.max, b.unit)),
                "bound and unit: {line}"
            );
            assert!(line.contains(&b.change), "owning change: {line}");
        }
    }

    #[test]
    fn scenario_performance_budgets_absolute_budget_exceeded_fails() {
        let b = budget("cold-start", 400.0);
        let m = measured(&[("cold-start", 450.0)]);
        let v = evaluate(&[&b], &m, &Baseline::default(), Mode::Local);
        assert_eq!(v.len(), 1, "{v:?}");
        let msg = &v[0].0;
        assert!(
            msg.contains("cold-start") && msg.contains("450") && msg.contains("400"),
            "{msg}"
        );
        let ok = measured(&[("cold-start", 380.0)]);
        assert!(evaluate(&[&b], &ok, &Baseline::default(), Mode::Local).is_empty());
    }

    #[test]
    fn scenario_performance_budgets_missing_measurement_fails() {
        let (a, b) = (budget("catalog-insert", 20.0), budget("idle-rss", 250.0));
        let m = measured(&[("catalog-insert", 15.0)]);
        for mode in [Mode::Local, Mode::Ci] {
            let v = evaluate(&[&a, &b], &m, &Baseline::default(), mode);
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].0.contains("idle-rss") && v[0].0.contains("no measurement"),
                "{v:?}"
            );
        }
    }

    #[test]
    fn scenario_performance_budgets_regression_detected() {
        let (slow, steady) = (budget("near-dup-index-build", 60.0), budget("steady", 60.0));
        let mut baseline = Baseline::default();
        baseline.values.insert("near-dup-index-build".into(), 40.0);
        baseline.values.insert("steady".into(), 40.0);
        // 20% slower than the baseline, still inside the absolute budget.
        let m = measured(&[("near-dup-index-build", 48.0), ("steady", 42.0)]);
        for mode in [Mode::Ci, Mode::Local] {
            let v = evaluate(&[&slow, &steady], &m, &baseline, mode);
            assert_eq!(v.len(), 1, "{v:?}");
            let msg = &v[0].0;
            assert!(
                msg.contains("near-dup-index-build"),
                "names the benchmark: {msg}"
            );
            assert!(
                msg.contains("48.00") && msg.contains("40.00"),
                "both values: {msg}"
            );
            assert!(msg.contains("20%"), "{msg}");
        }
    }

    #[test]
    fn scenario_performance_budgets_small_difference_is_noise() {
        // CI run 39644751071: base 53.11 ms, head 61.95 ms against a 250 ms bound. 17% worse, but 8.8 ms is
        // under 5% of the bound (12.5 ms).
        let b = budget("control-latency-p95", 250.0);
        let mut baseline = Baseline::default();
        baseline.values.insert("control-latency-p95".into(), 53.11);
        for mode in [Mode::Ci, Mode::Local] {
            let noisy = measured(&[("control-latency-p95", 61.95)]);
            assert!(evaluate(&[&b], &noisy, &baseline, mode).is_empty());
            assert!(regressed(&[&b], &noisy, &baseline).is_empty());
            let slow = measured(&[("control-latency-p95", 70.0)]);
            assert_eq!(evaluate(&[&b], &slow, &baseline, mode).len(), 1);
        }
    }

    #[test]
    fn ci_remeasures_only_regressed_budgets_and_keeps_the_best_run() {
        let (a, b, c) = (budget("a", 20.0), budget("b", 20.0), budget("c", 20.0));
        let mut baseline = Baseline::default();
        baseline.values.insert("a".into(), 10.0);
        baseline.values.insert("b".into(), 10.0);
        let mut m = measured(&[("a", 13.0), ("b", 11.0)]);
        let suspects: Vec<&str> = regressed(&[&a, &b, &c], &m, &baseline)
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(
            suspects,
            ["a"],
            "c has no measurement: reported as missing, not re-measured"
        );
        best_of(&mut m, "a", 11.2);
        best_of(&mut m, "a", 12.5);
        assert_eq!(m["a"].value, 11.2);
        assert!(evaluate(&[&a, &b], &m, &baseline, Mode::Ci).is_empty());
    }

    #[test]
    fn missing_base_values_are_listed_and_a_missing_baseline_file_is_an_error() {
        let (old, new) = (budget("catalog-insert", 20.0), budget("brand-new", 10.0));
        let mut baseline = Baseline::default();
        baseline.values.insert("catalog-insert".into(), 14.0);
        assert_eq!(missing_base(&[&old, &new], &baseline), ["brand-new"]);
        // A zero is no base either: regressions are relative to it.
        baseline.values.insert("brand-new".into(), 0.0);
        assert_eq!(missing_base(&[&old, &new], &baseline), ["brand-new"]);
        baseline.values.insert("brand-new".into(), 8.0);
        assert!(missing_base(&[&old, &new], &baseline).is_empty());

        let path =
            std::env::temp_dir().join(format!("perf-baseline-ci-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(read_baseline_required(&path).is_err(), "missing file");
        assert!(
            read_baseline(&path).unwrap().values.is_empty(),
            "update-only runs start empty"
        );
        std::fs::write(&path, "not json").unwrap();
        assert!(read_baseline_required(&path).is_err(), "unreadable file");
        std::fs::write(&path, r#"{"values":{"catalog-insert":14.0}}"#).unwrap();
        assert_eq!(read_baseline_required(&path).unwrap().values.len(), 1);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn scenario_performance_budgets_ci_mode_checks_regressions_only() {
        let b = budget("scan-analysis-12mp", 6.67);
        let mut baseline = Baseline::default();
        baseline.values.insert("scan-analysis-12mp".into(), 20.0);
        // A CI runner: far above the reference machine's absolute bound, 10% above its own baseline.
        let m = measured(&[("scan-analysis-12mp", 22.0)]);
        assert!(evaluate(&[&b], &m, &baseline, Mode::Ci).is_empty());
        assert_eq!(evaluate(&[&b], &m, &baseline, Mode::Local).len(), 1);

        let budgets = vec![
            budget("catalog-insert", 20.0),
            app_budget("cold-start", 400.0),
        ];
        let ci: Vec<&str> = select(&budgets, &[], Mode::Ci)
            .unwrap()
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(
            ci,
            ["catalog-insert"],
            "app window measurements are not run"
        );
        assert_eq!(select(&budgets, &[], Mode::Local).unwrap().len(), 2);
        let only = select(&budgets, &["cold-start".into()], Mode::Ci).unwrap();
        assert!(only.is_empty());
        assert!(select(&budgets, &["cold-strat".into()], Mode::Local).is_err());
    }

    #[test]
    fn scenario_performance_budgets_baseline_update_writes_values() {
        let mut baseline = Baseline::default();
        for (i, (name, _, _)) in ADR_0026.iter().enumerate() {
            baseline.values.insert((*name).into(), i as f64 + 1.0);
        }
        let before = baseline.values.clone();
        merge_baseline(
            &mut baseline,
            &measured(&[("cold-start", 210.0), ("idle-rss", 190.0)]),
        );
        assert_eq!(baseline.values.len(), 11);
        assert_eq!(baseline.values["cold-start"], 210.0);
        assert_eq!(baseline.values["idle-rss"], 190.0);
        let unchanged = before
            .iter()
            .filter(|(n, v)| baseline.values.get(*n) == Some(*v))
            .count();
        assert_eq!(unchanged, 9, "the other nine keep their values");
        // And it round-trips through the file format.
        let json = serde_json::to_string_pretty(&baseline).unwrap();
        let back: Baseline = serde_json::from_str(&json).unwrap();
        assert_eq!(back.values, baseline.values);
    }

    #[test]
    fn scenario_performance_budgets_all_budgets_within_limits() {
        // The committed baseline is the recorded output of `cargo xtask bench --check --update-baseline` on the
        // reference machine (docs/performance.md); `--update-baseline` writes it only after a passing check.
        // Every budget, window budgets included, is recorded (performance-budgets task 4.2) and within its bound.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let baseline = read_baseline_required(&root.join(BASELINE_FILE)).unwrap();
        let budgets = workspace_budgets();
        for b in &budgets {
            match baseline.values.get(&b.name) {
                Some(&v) => assert!(
                    v <= b.max,
                    "{}: recorded {v} {} exceeds {} {}",
                    b.name,
                    b.unit,
                    b.max,
                    b.unit
                ),
                None => panic!(
                    "{} ({} source) has no recorded measurement in {BASELINE_FILE}",
                    b.name,
                    b.source.kind()
                ),
            }
        }
        let recorded: Vec<&Budget> = budgets
            .iter()
            .filter(|b| baseline.values.contains_key(&b.name))
            .collect();
        let as_measured: BTreeMap<String, Measurement> = baseline
            .values
            .iter()
            .map(|(n, v)| (n.clone(), Measurement { value: *v }))
            .collect();
        assert!(evaluate(&recorded, &as_measured, &Baseline::default(), Mode::Local).is_empty());
        let names: Vec<&str> = budgets.iter().map(|b| b.name.as_str()).collect();
        assert!(
            baseline.values.keys().all(|k| names.contains(&k.as_str())),
            "no stale entries"
        );
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
}
