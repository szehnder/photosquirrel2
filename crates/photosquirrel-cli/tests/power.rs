//! Pause on battery in CLI scans (change `cli-power-pause`, ADR-0072): every test drives the built binary with a
//! fake Linux `power_supply` folder (`PHOTOSQUIRREL_POWER_SUPPLY_DIR`), so no test needs a battery. The folder
//! holds `BAT0` (discharging) and `AC`, whose `online` the test rewrites to plug and unplug the machine.
//!
//! The interrupt tests run one at a time, as in `tests/interrupt.rs`; the one asserting the 1 s exit bound is a
//! timing test, run alone by `cargo xtask timing` (ADR-0076).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

/// The small corpus (seed 42), generated once per test binary.
fn corpus() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        ps_fixtures::shared_corpus(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            "cli-power-corpus-small-42",
            42,
            ps_fixtures::Preset::Small,
        )
        .canonicalize()
        .unwrap()
    })
}

/// A data folder (with the small corpus's drives added) and a fake power supply.
struct Machine {
    data: tempfile::TempDir,
    supply: tempfile::TempDir,
}

impl Machine {
    /// `pause_on_battery`: the shared setting, written to `<data-dir>/config/settings.toml`.
    fn new(pause_on_battery: bool) -> Machine {
        let m = Machine {
            data: tempfile::tempdir().unwrap(),
            supply: tempfile::tempdir().unwrap(),
        };
        let bat = m.supply.path().join("BAT0");
        std::fs::create_dir_all(&bat).unwrap();
        std::fs::write(bat.join("type"), "Battery\n").unwrap();
        std::fs::write(bat.join("status"), "Discharging\n").unwrap();
        std::fs::create_dir_all(m.supply.path().join("AC")).unwrap();
        std::fs::write(m.supply.path().join("AC/type"), "Mains\n").unwrap();
        m.set_mains(true);
        let config = m.data.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("settings.toml"),
            format!("[general]\npause_on_battery = {pause_on_battery}\n"),
        )
        .unwrap();
        for d in ["drive-a", "drive-b", "drive-c"] {
            m.ok(&["sources", "add", corpus().join(d).to_str().unwrap()]);
        }
        m
    }

    /// Plug in (`true`) or unplug. Written to a temporary file and renamed, so the CLI never reads half a file.
    fn set_mains(&self, online: bool) {
        let tmp = self.supply.path().join("AC/online.tmp");
        std::fs::write(&tmp, if online { "1\n" } else { "0\n" }).unwrap();
        std::fs::rename(&tmp, self.supply.path().join("AC/online")).unwrap();
    }

    fn cli(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
        c.env("PHOTOSQUIRREL_DATA_DIR", self.data.path())
            .env("PHOTOSQUIRREL_POWER_SUPPLY_DIR", self.supply.path())
            .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
            .stdin(Stdio::null());
        c
    }

    fn ok(&self, args: &[&str]) -> Output {
        let o = self.cli().args(args).output().unwrap();
        assert!(
            o.status.success(),
            "{args:?} exited {:?}\nstdout: {}\nstderr: {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        o
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.ok(args).stdout).unwrap()
    }

    /// Start `--json scan <extra>` with stdout and stderr piped.
    fn spawn_json_scan(&self, extra: &[&str]) -> Scan {
        let mut child = self
            .cli()
            .args(["--json", "scan"])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines() {
                let l = l.unwrap();
                let v: Value = serde_json::from_str(&l).unwrap_or_else(|e| panic!("{e}: {l}"));
                if tx.send(v).is_err() {
                    return;
                }
            }
        });
        let stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || std::io::read_to_string(stderr).unwrap());
        Scan {
            child,
            lines: rx,
            seen: Vec::new(),
            stderr: Some(stderr),
        }
    }
}

/// A running `--json scan`: its output lines as they arrive.
struct Scan {
    child: Child,
    lines: std::sync::mpsc::Receiver<Value>,
    seen: Vec<Value>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Scan {
    /// The next line, within a generous bound (only a hang fails it).
    fn next(&mut self) -> Option<Value> {
        let v = self
            .lines
            .recv_timeout(ps_fixtures::ci_slack(Duration::from_secs(60)))
            .ok()?;
        self.seen.push(v.clone());
        Some(v)
    }

    /// Read until a line matches `f`; that line.
    fn until(&mut self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        loop {
            let v = self
                .next()
                .unwrap_or_else(|| panic!("no line {what}: {:?}", self.seen));
            if f(&v) {
                return v;
            }
        }
    }

    /// Read every remaining line and wait for the exit; (exit code, stderr).
    fn finish(mut self) -> (Option<i32>, String, Vec<Value>) {
        while self.next().is_some() {}
        let code = self.child.wait().unwrap().code();
        let err = self.stderr.take().unwrap().join().unwrap();
        (code, err, self.seen)
    }
}

fn is_power(v: &Value, event: &str) -> bool {
    v["type"] == "power" && v["data"]["event"] == event
}

fn is_snapshot(v: &Value, status: &str) -> bool {
    v["type"] == "snapshot" && v["data"]["status"] == status
}

/// A scan started on battery and kept there for `hold` before mains power returns; every output line, stderr.
fn paused_then_mains(hold: Duration) -> (Vec<Value>, String) {
    let m = Machine::new(true);
    m.set_mains(false);
    let mut scan = m.spawn_json_scan(&[]);
    scan.until("paused on battery", |v| is_power(v, "paused_on_battery"));
    std::thread::sleep(hold);
    m.set_mains(true);
    let (code, err, lines) = scan.finish();
    assert_eq!(code, Some(0), "{err}");
    (lines, err)
}

/// The run id of every line, which must all be the same run.
fn one_run(lines: &[Value]) -> i64 {
    let run = lines[0]["data"]["run"].as_i64().unwrap();
    for v in lines {
        assert_eq!(v["data"]["run"].as_i64(), Some(run), "{v}");
    }
    run
}

#[test]
fn scenario_settings_cli_scan_started_on_battery_pauses_at_once() {
    let (lines, err) = paused_then_mains(Duration::from_millis(1500));
    assert!(
        is_power(&lines[0], "paused_on_battery"),
        "the first line is the power pause: {:?}",
        lines[0]
    );
    one_run(&lines);
    assert!(lines.iter().any(|v| is_snapshot(v, "paused")), "{lines:?}");
    assert!(
        lines.iter().any(|v| is_power(v, "resumed_on_mains")),
        "{lines:?}"
    );
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "summary");
    assert_eq!(last["data"]["status"], "completed");
    assert!(err.contains("mains power is back"), "{err}");
}

#[test]
fn scenario_cli_power_pause_json_lines() {
    let (lines, _) = paused_then_mains(Duration::from_millis(1500));
    for v in &lines {
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["command"], "scan");
        assert!(
            ["snapshot", "power", "summary"].contains(&v["type"].as_str().unwrap()),
            "{v}"
        );
    }
    let run = one_run(&lines);
    let paused_at = lines
        .iter()
        .position(|v| is_power(v, "paused_on_battery"))
        .expect("a paused_on_battery line");
    let resumed_at = lines
        .iter()
        .position(|v| is_power(v, "resumed_on_mains"))
        .expect("a resumed_on_mains line");
    let paused: Vec<usize> = (0..lines.len())
        .filter(|&i| is_snapshot(&lines[i], "paused"))
        .collect();
    assert!(!paused.is_empty(), "{lines:?}");
    assert!(
        paused.iter().all(|&i| paused_at < i && i < resumed_at),
        "the power lines bracket the paused snapshots: {lines:?}"
    );
    for i in [paused_at, resumed_at] {
        assert_eq!(lines[i]["data"]["run"], run);
        assert!(lines[i]["data"]["at_ms"].as_u64().unwrap() > 1_600_000_000_000);
    }
    assert_eq!(
        lines.iter().filter(|v| v["type"] == "power").count(),
        2,
        "one line per transition"
    );
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "summary");
    assert_eq!(last["data"]["status"], "completed");
}

#[test]
fn scenario_cli_paused_snapshots_throttled() {
    let (lines, _) = paused_then_mains(Duration::from_secs(12));
    let from = lines
        .iter()
        .position(|v| is_power(v, "paused_on_battery"))
        .unwrap();
    let to = lines
        .iter()
        .position(|v| is_power(v, "resumed_on_mains"))
        .unwrap();
    let during = &lines[from + 1..to];
    assert!(
        during.iter().all(|v| is_snapshot(v, "paused")),
        "{during:?}"
    );
    assert!(
        (1..=4).contains(&during.len()),
        "{} snapshot lines during a 12 s pause",
        during.len()
    );
}

#[test]
fn scenario_cli_power_pause_on_stderr() {
    let m = Machine::new(true);
    m.set_mains(false);
    // Neither `--json` nor a terminal: plain progress lines on stderr.
    let mut child = m
        .cli()
        .arg("scan")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdout = std::thread::spawn(move || std::io::read_to_string(stdout).unwrap());
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = Vec::new();
    loop {
        let l = err.next().expect("a stderr line").unwrap();
        seen.push(l.clone());
        if l.contains("on battery power") {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(2500));
    m.set_mains(true);
    seen.extend(err.map(|l| l.unwrap()));
    assert!(child.wait().unwrap().success(), "{seen:?}");
    let out = stdout.join().unwrap();
    assert!(out.contains("completed"), "{out}");

    let pauses: Vec<usize> = (0..seen.len())
        .filter(|&i| seen[i].starts_with("paused:"))
        .collect();
    assert_eq!(pauses.len(), 1, "said once while the pause lasts: {seen:?}");
    let line = &seen[pauses[0]];
    assert!(
        line.contains("on battery power") && line.contains("--ignore-battery"),
        "{line}"
    );
    // Printed in the loop iteration that paused the run, before its first progress line (so within a second of
    // the pause, without a wall-clock assertion).
    let first_progress = seen.iter().position(|l| l.starts_with("run ")).unwrap();
    assert!(pauses[0] < first_progress, "{seen:?}");
    assert!(
        seen[first_progress].contains("paused on battery"),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|l| l.contains("mains power is back: resuming")),
        "{seen:?}"
    );
}

#[test]
fn scenario_settings_cli_ignore_battery_flag() {
    let m = Machine::new(true);
    m.set_mains(false);
    let (code, err, lines) = m.spawn_json_scan(&["--ignore-battery"]).finish();
    assert_eq!(code, Some(0), "{err}");
    assert!(lines.iter().all(|v| v["type"] != "power"), "{lines:?}");
    assert!(!lines.iter().any(|v| is_snapshot(v, "paused")), "{lines:?}");
    assert_eq!(lines.last().unwrap()["data"]["status"], "completed");
    assert!(
        err.contains("on battery power") && err.contains("--ignore-battery"),
        "stderr says battery power is ignored: {err}"
    );
    // `scan`, `scan start` and `scan resume` list the flag.
    for args in [
        &["scan", "--help"][..],
        &["scan", "start", "--help"],
        &["scan", "resume", "--help"],
    ] {
        let help = String::from_utf8(m.ok(args).stdout).unwrap();
        assert!(help.contains("--ignore-battery"), "{args:?}: {help}");
    }
}

#[test]
fn setting_off_prints_no_power_line() {
    let m = Machine::new(false);
    m.set_mains(false);
    let (code, err, lines) = m.spawn_json_scan(&[]).finish();
    assert_eq!(code, Some(0), "{err}");
    assert!(lines.iter().all(|v| v["type"] != "power"), "{lines:?}");
    assert!(!lines.iter().any(|v| is_snapshot(v, "paused")), "{lines:?}");
    assert_eq!(lines.last().unwrap()["data"]["status"], "completed");
    assert!(!err.contains("battery"), "{err}");
}

/// Poll `--json scan status` until `f` holds for its `live` object (bounded; only a hang fails it).
fn live_until(m: &Machine, what: &str, f: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + ps_fixtures::ci_slack(Duration::from_secs(20));
    loop {
        let st = m.json(&["--json", "scan", "status"]);
        let live = &st["data"]["live"];
        if f(live) {
            return live.clone();
        }
        assert!(Instant::now() < deadline, "timed out: {what}: {st}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn scenario_cli_status_shows_power_pause() {
    let m = Machine::new(true);
    m.set_mains(false);
    let mut scan = m.spawn_json_scan(&[]);
    let paused = scan.until("paused on battery", |v| is_power(v, "paused_on_battery"));
    let run = paused["data"]["run"].clone();
    // Keep reading the scan's output so it never blocks on a full pipe.
    let drain = std::thread::spawn(move || scan.finish());

    let live = live_until(&m, "power pause in the heartbeat", |l| {
        l["power_pause"] == "on_battery"
    });
    assert_eq!(live["status"], "paused");
    assert_eq!(live["run"], run);
    let text = String::from_utf8(m.ok(&["scan", "status"]).stdout).unwrap();
    let line = text.lines().find(|l| l.starts_with("live:")).unwrap();
    assert!(line.contains("paused on battery"), "{text}");

    // `scan pause` from a second process makes the pause the user's; mains power then does not resume it.
    let o = String::from_utf8(m.ok(&["scan", "pause"]).stdout).unwrap();
    assert!(o.contains("scan is paused"), "{o}");
    m.set_mains(true);
    live_until(&m, "the power note cleared", |l| l["power_pause"].is_null());
    std::thread::sleep(Duration::from_millis(2500));
    let live = live_until(&m, "still live", |l| l.is_object());
    assert_eq!(live["status"], "paused", "a user pause is not auto-resumed");
    // `scan resume` from a second process continues it.
    let resumed = m.json(&["--json", "scan", "resume"]);
    assert_eq!(resumed["data"]["requested"], "resume");
    let (code, err, lines) = drain.join().unwrap();
    assert_eq!(code, Some(0), "{err}");
    assert!(
        !lines.iter().any(|v| is_power(v, "resumed_on_mains")),
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap()["data"]["status"], "completed");
}

#[test]
fn resume_request_on_battery_keeps_the_scan_running() {
    let m = Machine::new(true);
    m.set_mains(false);
    let mut scan = m.spawn_json_scan(&[]);
    scan.until("paused on battery", |v| is_power(v, "paused_on_battery"));
    let drain = std::thread::spawn(move || scan.finish());
    live_until(&m, "heartbeat", |l| l["power_pause"] == "on_battery");
    m.json(&["--json", "scan", "resume"]);
    // Still on battery: the run completes without another power pause.
    let (code, err, lines) = drain.join().unwrap();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        lines.iter().filter(|v| v["type"] == "power").count(),
        1,
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap()["data"]["status"], "completed");
}

#[cfg(unix)]
mod interrupt {
    use super::*;

    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A scan paused on battery, interrupted with SIGINT; checks exit 130 and a cancelled run, then resumes it on
    /// mains power with `scan --resume`. Returns the time from the interrupt to the exit.
    pub(super) fn interrupt_paused_scan() -> Duration {
        let _one = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let m = Machine::new(true);
        m.set_mains(false);
        let mut scan = m.spawn_json_scan(&[]);
        let first = scan.until("paused on battery", |v| is_power(v, "paused_on_battery"));
        let run = first["data"]["run"].as_i64().unwrap();
        scan.until("a paused snapshot", |v| is_snapshot(v, "paused"));
        let t = Instant::now();
        let kill = Command::new("kill")
            .args(["-INT", &scan.child.id().to_string()])
            .status()
            .unwrap();
        assert!(kill.success());
        let status = scan.child.wait().unwrap();
        let took = t.elapsed();
        assert_eq!(status.code(), Some(130));
        let (_, _, lines) = scan.finish();
        let last = lines.last().unwrap();
        assert_eq!(last["type"], "summary");
        assert_eq!(last["data"]["status"], "cancelled");
        assert!(
            !lines.iter().any(|v| is_power(v, "resumed_on_mains")),
            "nothing resumed the cancelled run: {lines:?}"
        );
        let st = m.json(&["--json", "status"]);
        assert_eq!(st["data"]["runs"][0]["status"], "cancelled");

        m.set_mains(true);
        let o = m.ok(&["--json", "scan", "--resume"]);
        let s = String::from_utf8_lossy(&o.stdout);
        let last: Value = serde_json::from_str(s.lines().last().unwrap()).unwrap();
        assert_eq!(last["data"]["status"], "completed");
        assert_eq!(last["data"]["run"].as_i64().unwrap(), run, "same run id");
        took
    }

    #[test]
    fn interrupt_while_paused_on_battery_cancels_resumably() {
        interrupt_paused_scan();
    }
}

#[test]
// A wall-clock bound (exit within 1 s): run alone, after the other tests, by `cargo xtask timing` (ADR-0076).
#[ignore = "timing"]
#[cfg(unix)]
fn scenario_cli_ctrl_c_while_paused_on_battery() {
    let took = interrupt::interrupt_paused_scan();
    println!("exit after the interrupt: {took:?}");
    assert!(took < Duration::from_secs(1), "exited after {took:?}");
}
