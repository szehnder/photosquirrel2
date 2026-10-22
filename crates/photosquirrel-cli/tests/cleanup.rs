//! `cleanup` CLI scenarios (change `quarantine-cleanup`): the built binary plans, refuses to move without
//! `--yes`, runs and undoes a cleanup of a fully exported corpus drive. Each test works on its own copy of the
//! small corpus under the target dir.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const QUARANTINE_DIR: &str = ".photosquirrel-quarantine";

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

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &t);
        } else {
            fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// Every file under `root` outside the quarantine folder (and every folder): path -> (size, BLAKE3).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<(u64, [u8; 32])>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if rel.starts_with(QUARANTINE_DIR) {
                continue;
            }
            if p.is_dir() {
                out.insert(rel, None);
                stack.push(p);
            } else {
                let b = fs::read(&p).unwrap();
                out.insert(rel, Some((b.len() as u64, *blake3::hash(&b).as_bytes())));
            }
        }
    }
    out
}

/// A scratch folder with a copy of the small corpus, a data folder where its drives are scanned, and a
/// library they are fully exported to.
struct Env {
    tmp: tempfile::TempDir,
}

impl Env {
    fn new(tag: &str) -> Env {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(base).unwrap();
        let tmp = tempfile::Builder::new()
            .prefix(&format!("cli-cleanup-{tag}-"))
            .tempdir_in(base)
            .unwrap();
        let shared = ps_fixtures::shared_corpus(
            base,
            "cli-cleanup-corpus-small-42",
            42,
            ps_fixtures::Preset::Small,
        );
        copy_tree(&shared, &tmp.path().join("corpus"));
        let env = Env { tmp };
        for d in ["drive-a", "drive-b", "drive-c"] {
            ok(
                &env.data(),
                &["sources", "add", env.corpus().join(d).to_str().unwrap()],
            );
        }
        ok(&env.data(), &["scan"]);
        ok(
            &env.data(),
            &[
                "export",
                "run",
                "--library",
                env.library().to_str().unwrap(),
                "--profile",
                "lightroom",
                "--yes",
            ],
        );
        env
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    fn corpus(&self) -> PathBuf {
        self.tmp.path().join("corpus")
    }

    fn library(&self) -> PathBuf {
        self.tmp.path().join("Library")
    }

    fn plan(&self, source: &str) -> Value {
        json(&ok(
            &self.data(),
            &["--json", "cleanup", "plan", "--source", source],
        ))
    }
}

#[test]
fn scenario_cleanup_no_move_without_confirmation() {
    let env = Env::new("confirm");
    let drive = env.corpus().join("drive-a");
    let before = snapshot(&drive);
    let plan = env.plan("1");
    assert_eq!(plan["command"], "cleanup plan");
    let id = plan["data"]["run"].as_i64().expect("a run id");
    let n = plan["data"]["moves"].as_array().unwrap().len();
    assert!(n > 0);
    assert_eq!(snapshot(&drive), before, "planning moves nothing");
    // Non-interactive (stdin is not a terminal) and no --yes: nothing moves, exit status 1.
    let o = run(&env.data(), &["cleanup", "run", &id.to_string()]);
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("--yes"), "{err}");
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("nothing was moved"),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let o = run(&env.data(), &["--json", "cleanup", "run", &id.to_string()]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["data"]["moved"], 0);
    assert_eq!(snapshot(&drive), before);
    assert!(!drive.join(QUARANTINE_DIR).exists());
    let list = json(&ok(&env.data(), &["--json", "cleanup", "list"]));
    assert_eq!(list["data"][0]["counts"]["planned"], n);
    assert_eq!(list["data"][0]["counts"]["quarantined"], 0);
    // Undo without --yes is refused the same way.
    let o = run(&env.data(), &["cleanup", "undo", &id.to_string()]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn scenario_cleanup_cli_undo_round_trip() {
    let env = Env::new("round-trip");
    let drive = env.corpus().join("drive-b");
    let before = snapshot(&drive);
    let plan = env.plan("2");
    let id = plan["data"]["run"].as_i64().unwrap().to_string();
    let n = plan["data"]["moves"].as_array().unwrap().len() as u64;
    let r = json(&ok(
        &env.data(),
        &["--json", "cleanup", "run", &id, "--yes"],
    ));
    assert_eq!(r["data"]["moved"], n, "{r}");
    assert_eq!(r["data"]["skipped"].as_array().unwrap().len(), 0);
    let first = &plan["data"]["moves"][0];
    assert!(!Path::new(first["path"].as_str().unwrap()).exists());
    assert!(Path::new(first["quarantine_path"].as_str().unwrap()).is_file());
    // The report counts them as quarantined, accounted for.
    let rep = json(&ok(&env.data(), &["--json", "report"]));
    assert_eq!(rep["data"]["totals"]["files"]["quarantined"], n);
    let show = json(&ok(&env.data(), &["--json", "cleanup", "show", &id]));
    assert_eq!(show["data"]["counts"]["quarantined"], n);
    // Undo the whole run: the drive's tree snapshot equals the one before the cleanup.
    let u = json(&ok(
        &env.data(),
        &["--json", "cleanup", "undo", &id, "--yes"],
    ));
    assert_eq!(u["data"]["restored"], n);
    assert_eq!(u["data"]["run_fully_undone"], true);
    assert_eq!(snapshot(&drive), before);
    let list = json(&ok(&env.data(), &["--json", "cleanup", "list"]));
    assert_eq!(list["data"][0]["run"]["state"], "undone");
    // The journal stays as the record of what happened.
    assert!(
        drive
            .join(QUARANTINE_DIR)
            .join(&id)
            .join("journal.jsonl")
            .is_file()
    );
}
