//! `cli` scenarios (change `headless-cli`) plus the CLI scenarios of `export`, `completion-report` and
//! `ai-queue`: every test drives the built binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

fn cli(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
    c.env("PHOTOSQUIRREL_DATA_DIR", data)
        .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
        .stdin(Stdio::null());
    c
}

fn run(data: &Path, args: &[&str]) -> Output {
    cli(data).args(args).output().expect("run cli")
}

fn ok(data: &Path, args: &[&str]) -> Output {
    let o = run(data, args);
    assert!(
        o.status.success(),
        "{args:?} exited {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

fn json(o: &Output) -> Value {
    let s = String::from_utf8_lossy(&o.stdout);
    let v: Value = serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("{e}: {s}"));
    assert_eq!(v["schema_version"], 1, "{s}");
    v
}

/// The small corpus (seed 42), generated once per test binary.
fn corpus() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        ps_fixtures::shared_corpus(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            "cli-corpus-small-42",
            42,
            ps_fixtures::Preset::Small,
        )
        .canonicalize()
        .unwrap()
    })
}

/// A data folder with the small corpus's three drives added and scanned.
fn scanned() -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    for d in ["drive-a", "drive-b", "drive-c"] {
        ok(
            data.path(),
            &["sources", "add", corpus().join(d).to_str().unwrap()],
        );
    }
    ok(data.path(), &["scan"]);
    data
}

#[test]
fn scenario_cli_usage_error_exits_2() {
    let data = tempfile::tempdir().unwrap();
    let o = run(data.path(), &["sources", "list", "--no-such-flag"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--no-such-flag"));
    assert!(o.stdout.is_empty());
}

#[test]
fn scenario_cli_failure_exits_1() {
    let data = tempfile::tempdir().unwrap();
    let o = run(data.path(), &["scan", "--source", "999"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("unknown source 999"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn scenario_cli_data_dir_override() {
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let o = cli(data.path())
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("share"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .args([
            "--json",
            "sources",
            "add",
            corpus().join("drive-a").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(data.path().join("catalog.db").is_file());
    let leftovers: Vec<_> = std::fs::read_dir(home.path()).unwrap().collect();
    assert!(
        leftovers.is_empty(),
        "default folders untouched: {leftovers:?}"
    );
    // The flag wins over the environment.
    let other = tempfile::tempdir().unwrap();
    let o = ok(
        data.path(),
        &[
            "--data-dir",
            other.path().to_str().unwrap(),
            "--json",
            "sources",
            "list",
        ],
    );
    assert_eq!(json(&o)["data"].as_array().unwrap().len(), 0);
}

#[test]
fn scenario_cli_non_interactive_needs_yes() {
    let data = tempfile::tempdir().unwrap();
    ok(
        data.path(),
        &["sources", "add", corpus().join("drive-a").to_str().unwrap()],
    );
    let o = run(data.path(), &["sources", "remove", "1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--yes"));
    let list = json(&ok(data.path(), &["--json", "sources", "list"]));
    assert_eq!(list["data"].as_array().unwrap().len(), 1, "nothing removed");
    ok(data.path(), &["sources", "remove", "1", "--yes"]);
    let list = json(&ok(data.path(), &["--json", "sources", "list"]));
    assert!(list["data"].as_array().unwrap().is_empty());
    assert!(corpus().join("drive-a").is_dir(), "nothing on disk touched");
}

#[test]
fn scenario_cli_scan_streams_json_snapshots() {
    let data = tempfile::tempdir().unwrap();
    for d in ["drive-a", "drive-b", "drive-c"] {
        ok(
            data.path(),
            &["sources", "add", corpus().join(d).to_str().unwrap()],
        );
    }
    let mut child = cli(data.path())
        .args(["--json", "scan"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = Vec::new();
    for l in BufReader::new(child.stdout.take().unwrap()).lines() {
        lines.push((Instant::now(), l.unwrap()));
    }
    assert!(child.wait().unwrap().success());
    assert!(lines.len() >= 2, "{lines:?}");
    let parsed: Vec<Value> = lines
        .iter()
        .map(|(_, l)| serde_json::from_str(l).unwrap())
        .collect();
    for w in lines.windows(2).take(lines.len().saturating_sub(2)) {
        assert!(w[1].0 - w[0].0 >= Duration::from_millis(100) - Duration::from_millis(15));
    }
    for v in &parsed[..parsed.len() - 1] {
        assert_eq!(v["type"], "snapshot");
        assert_eq!(v["schema_version"], 1);
    }
    let last = parsed.last().unwrap();
    assert_eq!(last["type"], "summary");
    assert_eq!(last["data"]["status"], "completed");
}

#[test]
fn status_json_snapshot_of_the_small_corpus() {
    let data = scanned();
    let v = json(&ok(data.path(), &["--json", "status"]));
    assert_eq!(v["data"]["sources"], 3);
    assert!(v["data"]["groups"]["groups"].as_u64().unwrap() > 50);
    assert_eq!(v["data"]["runs"][0]["status"], "completed");
    insta::assert_json_snapshot!("status_json_shape", shape(&v));
}

/// The JSON's structure (keys and value types; arrays by their first element), stable across runs. Keys are
/// sorted so the snapshot does not depend on serde_json's `preserve_order` feature (enabled workspace-wide by
/// other crates' dependencies).
fn shape(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(&String, &Value)> = m.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), shape(v)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.first().map(shape).into_iter().collect()),
        Value::String(_) => "string".into(),
        Value::Number(_) => "number".into(),
        Value::Bool(_) => "bool".into(),
        Value::Null => "null".into(),
    }
}

fn first_group_with_choice(data: &Path) -> (i64, Value) {
    let list = json(&ok(data, &["--json", "groups", "list", "--limit", "200"]));
    for g in list["data"]["groups"].as_array().unwrap() {
        let id = g["id"].as_i64().unwrap();
        let d = json(&ok(data, &["--json", "groups", "show", &id.to_string()]));
        let members = d["data"]["members"].as_array().unwrap();
        if let Some(m) = members
            .iter()
            .find(|m| m["edge"] == "derivative" && !m["in_keeper_set"].as_bool().unwrap())
        {
            return (id, m.clone());
        }
    }
    panic!("no group with a derivative member");
}

#[test]
fn scenario_cli_groups_explain_keeper() {
    let data = scanned();
    let (id, _) = first_group_with_choice(data.path());
    let d = json(&ok(
        data.path(),
        &["--json", "groups", "show", &id.to_string()],
    ));
    for m in d["data"]["members"].as_array().unwrap() {
        assert!(
            m["path"]
                .as_str()
                .unwrap()
                .starts_with(corpus().to_str().unwrap())
        );
        assert!(m["edge"].is_string());
        if m["edge"] == "derivative" {
            assert!(m["phash_distance"].is_number() && m["dhash_distance"].is_number());
        }
    }
    assert!(!d["data"]["keeper_reasons"].as_array().unwrap().is_empty());
    let human =
        String::from_utf8_lossy(&ok(data.path(), &["groups", "show", &id.to_string()]).stdout)
            .into_owned();
    assert!(
        human.contains("why this copy:") && human.contains("pHash distance"),
        "{human}"
    );
}

#[test]
fn scenario_cli_set_keeper_from_cli() {
    let data = scanned();
    let (id, member) = first_group_with_choice(data.path());
    let f = member["file"].as_i64().unwrap();
    ok(
        data.path(),
        &["groups", "set-keeper", &id.to_string(), &f.to_string()],
    );
    let d = json(&ok(
        data.path(),
        &["--json", "groups", "show", &id.to_string()],
    ));
    assert_eq!(d["data"]["keeper"].as_i64().unwrap(), f);
    assert_eq!(d["data"]["keeper_reasons"][0], "chosen by you");
}

fn help_paths() -> Vec<Vec<&'static str>> {
    let mut v: Vec<Vec<&str>> = vec![vec![]];
    for top in [
        "sources",
        "scan",
        "status",
        "report",
        "groups",
        "export",
        "cleanup",
        "ai",
        "doctor",
        "fixtures",
        "eval-corpus",
        "demo",
    ] {
        v.push(vec![top]);
    }
    for (top, subs) in [
        ("sources", &["add", "list", "relink", "remove"][..]),
        (
            "scan",
            &["start", "resume", "pause", "cancel", "status"][..],
        ),
        (
            "groups",
            &[
                "list",
                "show",
                "set-keeper",
                "clear-keeper",
                "separate",
                "merge",
                "suggestions",
                "explain",
                "regroup",
            ][..],
        ),
        ("export", &["dry-run", "run", "check"][..]),
        ("cleanup", &["plan", "run", "list", "show", "undo"][..]),
        (
            "ai",
            &["providers", "test", "start", "pause", "status", "results"][..],
        ),
        ("fixtures", &["generate", "evaluate"][..]),
    ] {
        for s in subs {
            v.push(vec![top, s]);
        }
    }
    v
}

#[test]
fn scenario_cli_every_subcommand_has_help() {
    let data = tempfile::tempdir().unwrap();
    for path in help_paths() {
        let mut args = path.clone();
        args.push("--help");
        let o = run(data.path(), &args);
        assert_eq!(o.status.code(), Some(0), "{args:?}");
        let s = String::from_utf8_lossy(&o.stdout);
        assert!(s.contains("Usage:"), "{args:?}: {s}");
    }
    assert!(
        !data.path().join("catalog.db").exists(),
        "help creates nothing"
    );
}

#[test]
fn scenario_cli_json_output_parses() {
    let data = scanned();
    let list = json(&ok(data.path(), &["--json", "groups", "list"]));
    let id = list["data"]["groups"][0]["id"]
        .as_i64()
        .unwrap()
        .to_string();
    for args in [
        vec!["--json", "sources", "list"],
        vec!["--json", "status"],
        vec!["--json", "groups", "list"],
        vec!["--json", "groups", "show", &id],
        vec!["--json", "groups", "suggestions", "--limit", "3"],
        vec!["--json", "doctor"],
        vec!["--json", "ai", "status"],
        vec!["--json", "ai", "providers"],
    ] {
        let o = ok(data.path(), &args);
        let v = json(&o);
        assert_eq!(
            v["command"].as_str().unwrap(),
            args[1..]
                .iter()
                .take_while(|a| !a.starts_with('-') && a.parse::<i64>().is_err())
                .copied()
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}

#[test]
fn scenario_cli_doctor_lists_decoders() {
    let data = tempfile::tempdir().unwrap();
    // A provider that would record any request: doctor must not contact it.
    let server = ps_llm::testing::MockServer::with_handler(|_| {
        ps_llm::testing::Reply::json(200, &serde_json::json!({}))
    });
    std::fs::create_dir_all(data.path().join("config")).unwrap();
    std::fs::write(
        data.path().join("config/settings.toml"),
        format!(
            "[ai]\nprovider = \"lm_studio\"\nbase_url = \"{}/v1\"\n",
            server.url()
        ),
    )
    .unwrap();
    ok(
        data.path(),
        &["sources", "add", corpus().join("drive-a").to_str().unwrap()],
    );
    let o = cli(data.path())
        .env("PHOTOSQUIRREL_HEIF_DECODER", "none")
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = json(&o);
    let d = &v["data"];
    assert_eq!(d["heic"], false);
    let entry = |name: &str| {
        d["decoders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == name)
            .unwrap_or_else(|| panic!("{name} listed: {d}"))
            .clone()
    };
    let heic = entry("heic");
    assert_eq!(heic["available"], false);
    assert!(
        heic["reason"]
            .as_str()
            .unwrap()
            .contains("PHOTOSQUIRREL_HEIF_DECODER=none")
    );
    assert!(heic["fix"]["summary"].is_string(), "{heic}");
    assert!(heic["fix"]["steps"].is_array());
    assert_eq!(d["decoder_override"][0], "PHOTOSQUIRREL_HEIF_DECODER=none");
    let avif = entry("avif");
    assert_eq!(avif["available"], true);
    assert!(avif["backend"].as_str().unwrap().contains("rav1d"));
    assert_eq!(avif["assembly"], cfg!(feature = "avif-asm"));
    assert!(d["os"]["family"].is_string());
    assert!(entry("poster_frames")["available"].is_boolean());
    assert_eq!(
        d["data_dir"].as_str().unwrap(),
        data.path().to_str().unwrap()
    );
    assert_eq!(
        d["catalog"]["schema_version"],
        ps_catalog::latest_schema_version()
    );
    assert!(d["keychain_available"].is_boolean());
    assert_eq!(d["llm_provider"]["name"], "LM Studio (local)");
    assert_eq!(d["network_used"], false);
    assert_eq!(server.request_count(), 0, "no network request");
}

#[test]
fn scenario_cli_evaluate_fails_below_gate() {
    let data = scanned();
    // The real truth passes.
    ok(
        data.path(),
        &["fixtures", "evaluate", corpus().to_str().unwrap()],
    );
    // Every file its own truth group: every result pair is wrong, precision 0.
    let mut t: Value =
        serde_json::from_slice(&std::fs::read(corpus().join("truth.json")).unwrap()).unwrap();
    for (i, f) in t["files"].as_array_mut().unwrap().iter_mut().enumerate() {
        f["group"] = format!("solo{i}").into();
    }
    let altered = data.path().join("altered.json");
    std::fs::write(&altered, serde_json::to_vec(&t).unwrap()).unwrap();
    let o = run(
        data.path(),
        &["fixtures", "evaluate", "--truth", altered.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(1));
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("precision") && s.contains("recall") && s.contains("keeper accuracy"),
        "{s}"
    );
    assert!(s.contains("gate: FAIL"));
}

/// headless-cli task 4.2: the end-to-end script, every step `--json` and exit 0.
#[test]
fn gate_script_runs_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let c = tmp.path().join("corpus");
    let v = json(&ok(
        &data,
        &[
            "--json",
            "fixtures",
            "generate",
            "--seed",
            "42",
            "--preset",
            "small",
            "--out",
            c.to_str().unwrap(),
        ],
    ));
    assert!(v["data"]["files"].as_u64().unwrap() > 500);
    for d in ["drive-a", "drive-b", "drive-c"] {
        json(&ok(
            &data,
            &["--json", "sources", "add", c.join(d).to_str().unwrap()],
        ));
    }
    let o = ok(&data, &["--json", "scan"]);
    let s = String::from_utf8_lossy(&o.stdout);
    let last: Value = serde_json::from_str(s.lines().last().unwrap()).unwrap();
    assert_eq!(last["data"]["status"], "completed");
    json(&ok(&data, &["--json", "groups", "list"]));
    let e = json(&ok(
        &data,
        &["--json", "fixtures", "evaluate", c.to_str().unwrap()],
    ));
    assert_eq!(e["data"]["passes_gate"], true);
    let e = json(&ok(&data, &["--json", "eval-corpus", c.to_str().unwrap()]));
    assert_eq!(e["data"]["score"]["precision"], 1.0);
}

#[test]
fn scenario_export_cli_dry_run_prints_plan() {
    let data = scanned();
    let lib = data.path().join("Library");
    let v = json(&ok(
        data.path(),
        &[
            "--json",
            "export",
            "dry-run",
            "--library",
            lib.to_str().unwrap(),
        ],
    ));
    let files = v["data"]["files"].as_array().unwrap();
    assert!(!files.is_empty());
    assert!(
        files
            .iter()
            .all(|f| f["target"].is_string() && f["source_path"].is_string())
    );
    assert_eq!(
        v["data"]["totals"]["files"].as_u64().unwrap() as usize,
        files.iter().filter(|f| f["action"] == "copy").count()
    );
    assert!(!lib.exists(), "the dry run writes nothing to the library");
}

#[test]
fn scenario_completion_report_cli_report_matches_ui() {
    let data = scanned();
    let lib = data.path().join("Library");
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
    let v = json(&ok(data.path(), &["--json", "report", "--source", "2"]));
    // What the Reports view shows comes from the same service call.
    let svc = ps_service::scan::ScanService::open(data.path(), Default::default()).unwrap();
    let c = svc.exports().completion(None, false).unwrap();
    let s = c.report.sources.iter().find(|s| s.source.0 == 2).unwrap();
    let cli = &v["data"]["sources"][0];
    assert_eq!(cli["source"], 2);
    for d in ps_model::Disposition::ALL {
        assert_eq!(
            cli["counts"]["files"][d.as_str()].as_u64().unwrap(),
            s.counts.files_of(d),
            "{d:?}"
        );
        assert_eq!(
            cli["counts"]["bytes"][d.as_str()].as_u64().unwrap(),
            s.counts.bytes[d as usize]
        );
    }
    assert_eq!(cli["counts"]["fully_accounted"], true);
}

fn recording_settings(data: &Path, url: &str) {
    std::fs::create_dir_all(data.join("config")).unwrap();
    std::fs::write(
        data.join("config/settings.toml"),
        format!("[ai]\nprovider = \"lm_studio\"\nbase_url = \"{url}/v1\"\nmodel = \"recording-vlm\"\nmax_concurrent_requests = 1\n"),
    )
    .unwrap();
}

fn insight_reply(delay: Duration) -> ps_llm::testing::Reply {
    ps_llm::testing::Reply::json(
        200,
        &ps_llm::testing::openai_reply(
            &ps_llm::testing::insight_json("people", 2, 1, &[]).to_string(),
        ),
    )
    .with_delay(delay)
}

#[test]
fn scenario_ai_queue_cli_starts_queue() {
    let data = scanned();
    let server = ps_llm::testing::MockServer::with_handler(|_| insight_reply(Duration::ZERO));
    recording_settings(data.path(), &server.url());
    let o = ok(data.path(), &["--json", "ai", "start"]);
    let s = String::from_utf8_lossy(&o.stdout);
    let last: Value = serde_json::from_str(s.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "summary");
    let done = last["data"]["done"].as_u64().unwrap();
    assert!(done > 0 && done as usize == server.request_count());
    let r = json(&ok(
        data.path(),
        &["--json", "ai", "results", "--limit", "1000"],
    ));
    let results = r["data"].as_array().unwrap();
    // A RAW+JPEG pair is described once and its result stored for both contents, so there can be more results
    // than requests, never fewer.
    assert!(
        results.len() as u64 >= done,
        "{} results for {done}",
        results.len()
    );
    assert_eq!(results[0]["insight"]["category"], "people");
    assert!(results[0]["model"].as_str().is_some_and(|m| !m.is_empty()));
    assert_eq!(results[0]["provider"], "LM Studio (local)");
}

#[test]
fn scenario_ai_queue_queue_progress_reported() {
    let data = scanned();
    let server =
        ps_llm::testing::MockServer::with_handler(|_| insight_reply(Duration::from_millis(150)));
    recording_settings(data.path(), &server.url());
    let mut child = cli(data.path())
        .args(["--json", "ai", "start"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    let mut seen_running = false;
    while t.elapsed() < Duration::from_secs(60) {
        let v = json(&ok(data.path(), &["--json", "ai", "status"]));
        let d = &v["data"];
        let total: u64 = ["done", "pending", "in_flight", "failed", "deferred"]
            .iter()
            .map(|k| d[*k].as_u64().unwrap())
            .sum();
        if d["state"] == "running" && total > 0 {
            seen_running = true;
            assert!(d["done"].as_u64().unwrap() <= server.request_count() as u64);
            assert_eq!(
                d["results"], d["done"],
                "counts consistent with the catalog"
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(seen_running);
    // Pause from this terminal: the running command stops sending and exits.
    ok(data.path(), &["ai", "pause"]);
    assert!(child.wait().unwrap().success());
    let v = json(&ok(data.path(), &["--json", "ai", "status"]));
    assert_eq!(v["data"]["state"], "paused");
}

#[test]
fn scenario_export_cli_run_needs_yes() {
    let data = scanned();
    let lib = data.path().join("Library");
    let o = run(
        data.path(),
        &["export", "run", "--library", lib.to_str().unwrap()],
    );
    assert!(!o.status.success(), "an unconfirmed export does not run");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("--yes"), "{err}");
    assert!(
        !err.contains("Some(") && !err.contains("PlanError"),
        "friendly: {err}"
    );
    assert!(!lib.exists(), "nothing was written");
}

/// Export the small corpus, then make a derivative member the keeper of one exported group.
fn exported_then_keeper_changed() -> (tempfile::TempDir, PathBuf, i64) {
    let data = scanned();
    let lib = data.path().join("Library");
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
    let (id, member) = first_group_with_choice(data.path());
    let f = member["file"].as_i64().unwrap();
    ok(
        data.path(),
        &["groups", "set-keeper", &id.to_string(), &f.to_string()],
    );
    (data, lib, id)
}

fn library_tree(lib: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![lib.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if !p.to_string_lossy().contains(".photosquirrel") {
                out.push((p.clone(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn scenario_export_cli_dry_run_lists_better_copies() {
    let (data, lib, id) = exported_then_keeper_changed();
    let before = library_tree(&lib);
    let l = lib.to_str().unwrap();
    let text = String::from_utf8_lossy(
        &ok(
            data.path(),
            &[
                "export",
                "dry-run",
                "--library",
                l,
                "--profile",
                "lightroom",
            ],
        )
        .stdout,
    )
    .into_owned();
    assert!(
        text.contains("1 better copy available (add --upgrade"),
        "{text}"
    );
    let line = text
        .lines()
        .find(|l| l.starts_with(&format!("  group {id}: ")))
        .unwrap_or_else(|| panic!("no line for group {id}: {text}"));
    assert!(line.contains("in the library; better copy "), "{line}");
    let v = json(&ok(
        data.path(),
        &[
            "--json",
            "export",
            "dry-run",
            "--library",
            l,
            "--profile",
            "lightroom",
        ],
    ));
    assert_eq!(v["data"]["totals"]["better_copies"], 1);
    assert!(v["data"]["totals"]["better_copy_bytes"].as_u64().unwrap() > 0);
    let b = &v["data"]["better_copies"][0];
    assert_eq!(b["group"], id);
    let library_copy = b["library"][0]["library_path"].as_str().unwrap();
    assert!(line.contains(library_copy), "{line} vs {library_copy}");
    let keeper = b["keeper"][0]["source_path"].as_str().unwrap();
    assert!(line.contains(keeper), "{line} vs {keeper}");
    assert_eq!(v["data"]["totals"]["files"], 0);
    assert_eq!(library_tree(&lib), before, "the dry run writes nothing");
}

#[test]
fn scenario_export_manual_keeper_change_after_export_copies_nothing() {
    let (data, lib, id) = exported_then_keeper_changed();
    let before = library_tree(&lib);
    let l = lib.to_str().unwrap();
    let v = json(&ok(
        data.path(),
        &[
            "--json",
            "export",
            "run",
            "--library",
            l,
            "--profile",
            "lightroom",
            "--yes",
        ],
    ));
    assert_eq!(v["data"]["copied"], 0, "{v}");
    assert_eq!(v["data"]["better_copies"][0]["group"], id);
    assert_eq!(library_tree(&lib), before);
    // With --upgrade the better copy lands beside the earlier copy, which stays as it was.
    let v = json(&ok(
        data.path(),
        &[
            "--json",
            "export",
            "run",
            "--library",
            l,
            "--profile",
            "lightroom",
            "--upgrade",
            "--yes",
        ],
    ));
    assert!(v["data"]["copied"].as_u64().unwrap() >= 1, "{v}");
    let after = library_tree(&lib);
    for f in &before {
        assert!(after.contains(f), "{} changed or removed", f.0.display());
    }
    let v = json(&ok(
        data.path(),
        &[
            "--json",
            "export",
            "dry-run",
            "--library",
            l,
            "--profile",
            "lightroom",
        ],
    ));
    assert_eq!(v["data"]["totals"]["better_copies"], 0);
    assert_eq!(v["data"]["totals"]["files"], 0);
}
