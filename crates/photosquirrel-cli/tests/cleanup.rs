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

#[test]
fn scenario_cleanup_cli_select_and_discard() {
    let env = Env::new("select-discard");
    let drive = env.corpus().join("drive-b");
    let before = snapshot(&drive);
    let plan = env.plan("2");
    let id = plan["data"]["run"].as_i64().unwrap().to_string();
    let moves = plan["data"]["moves"].as_array().unwrap();
    // The folder of the first planned file, relative to the drive.
    let first = Path::new(moves[0]["path"].as_str().unwrap());
    let folder = first
        .parent()
        .unwrap()
        .strip_prefix(&drive)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let under = |m: &Value| Path::new(m["path"].as_str().unwrap()).starts_with(drive.join(&folder));
    let n_in = moves.iter().filter(|m| under(m)).count() as u64;
    assert!(n_in > 0 && n_in < moves.len() as u64);
    let sel = json(&ok(
        &env.data(),
        &["--json", "cleanup", "select", &id, "--folder", &folder],
    ));
    assert_eq!(sel["command"], "cleanup select");
    assert_eq!(sel["data"]["counts"]["planned"], n_in, "{sel}");
    assert_eq!(sel["data"]["totals"]["files"], n_in);
    let show = json(&ok(&env.data(), &["--json", "cleanup", "show", &id]));
    assert_eq!(show["data"]["counts"]["planned"], n_in);
    assert_eq!(
        show["data"]["counts"]["unselected"],
        moves.len() as u64 - n_in
    );
    for m in show["data"]["moves"].as_array().unwrap() {
        let want = if under(m) { "planned" } else { "unselected" };
        assert_eq!(m["state"], want, "{m}");
    }
    // A selection without any flag is a usage error; one that picks nothing is refused.
    assert_eq!(
        run(&env.data(), &["cleanup", "select", &id]).status.code(),
        Some(2)
    );
    let o = run(
        &env.data(),
        &["cleanup", "select", &id, "--folder", "No Such Folder"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("discard"));
    // Discard: exit 0, reported discarded; nothing on the drive changed.
    let d = json(&ok(&env.data(), &["--json", "cleanup", "discard", &id]));
    assert_eq!(d["command"], "cleanup discard");
    assert_eq!(d["data"]["run"]["state"], "discarded");
    assert_eq!(d["data"]["counts"]["planned"], 0);
    let list = json(&ok(&env.data(), &["--json", "cleanup", "list"]));
    assert_eq!(list["data"][0]["run"]["state"], "discarded");
    let text = String::from_utf8_lossy(&ok(&env.data(), &["cleanup", "list"]).stdout).into_owned();
    assert!(text.contains("discarded"), "{text}");
    // A second discard and a run of it exit 1.
    let o = run(&env.data(), &["cleanup", "discard", &id]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("already discarded"));
    let o = run(&env.data(), &["cleanup", "run", &id, "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("discarded"));
    assert_eq!(snapshot(&drive), before);
    assert!(!drive.join(QUARANTINE_DIR).exists());
}

/// Gate review of `cleanup-selection`: a selection term that matches nothing is warned about (stderr and
/// `unmatched` in `--json`), with the folder that differs only in case; matching stays case-sensitive; and the
/// help says that each select replaces the previous one.
#[test]
fn cleanup_cli_select_warns_about_terms_that_match_nothing() {
    let env = Env::new("select-unmatched");
    let drive = env.corpus().join("drive-b");
    let plan = env.plan("2");
    let id = plan["data"]["run"].as_i64().unwrap().to_string();
    let moves = plan["data"]["moves"].as_array().unwrap();
    let first = Path::new(moves[0]["path"].as_str().unwrap());
    let folder = first
        .parent()
        .unwrap()
        .strip_prefix(&drive)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let lower = folder.to_lowercase();
    assert_ne!(lower, folder, "the corpus folder has capitals");
    let n_in = moves
        .iter()
        .filter(|m| Path::new(m["path"].as_str().unwrap()).starts_with(drive.join(&folder)))
        .count() as u64;
    let o = ok(
        &env.data(),
        &[
            "--json",
            "cleanup",
            "select",
            &id,
            "--folder",
            &folder,
            "--except-folder",
            &lower,
            "--except-file",
            "999999",
        ],
    );
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        err.contains(&format!(
            "warning: --except-folder {lower} matches no file of the plan (cleanup run {id})"
        )) && err.contains(&format!("did you mean --except-folder {folder:?}?")),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "warning: --except-file 999999 matches no file of the plan (cleanup run {id})"
        )),
        "{err}"
    );
    let sel = json(&o);
    // The lower-case exclusion matched nothing, so the whole folder is selected.
    assert_eq!(sel["data"]["counts"]["planned"], n_in, "{sel}");
    let unmatched = sel["data"]["unmatched"].as_array().unwrap();
    assert_eq!(unmatched.len(), 2, "{sel}");
    assert_eq!(unmatched[0]["flag"], "except-folder");
    assert_eq!(unmatched[0]["term"], lower.as_str());
    assert_eq!(unmatched[0]["did_you_mean"], folder.as_str());
    assert_eq!(unmatched[1]["flag"], "except-file");
    assert!(unmatched[1]["did_you_mean"].is_null());
    // A selection whose terms all match warns about nothing.
    let o = ok(&env.data(), &["--json", "cleanup", "select", &id, "--all"]);
    assert!(!String::from_utf8_lossy(&o.stderr).contains("warning"));
    assert!(json(&o)["data"]["unmatched"].as_array().unwrap().is_empty());
    // The help says that a select replaces the previous selection.
    let help = String::from_utf8_lossy(&ok(&env.data(), &["cleanup", "select", "--help"]).stdout)
        .into_owned();
    assert!(
        help.contains("replaces the run's previous selection"),
        "{help}"
    );
}

/// Two drives: drive-a holds an 800x533 JPEG made from drive-b's 3000x2000 original. A data folder, a library
/// and the paths of both.
struct Pair {
    tmp: tempfile::TempDir,
}

impl Pair {
    fn new(tag: &str) -> Pair {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(base).unwrap();
        let tmp = tempfile::Builder::new()
            .prefix(&format!("cli-cleanup-{tag}-"))
            .tempdir_in(base)
            .unwrap();
        let (w, h) = (3000, 2000);
        let rgb = ps_fixtures::media::photo_rgb(w, h, 4243);
        let full = ps_fixtures::media::encode_jpeg(&rgb, w, h, 92);
        let small = ps_media::decode::resize_to_long_edge(
            &ps_media::RgbImage {
                width: w,
                height: h,
                data: rgb,
            },
            800,
        );
        let web = ps_fixtures::media::encode_jpeg(&small.data, small.width, small.height, 80);
        let p = Pair { tmp };
        for (rel, b) in [
            ("drive-a/Web/IMG_0001.jpg", &web),
            ("drive-b/DCIM/IMG_0001.jpg", &full),
        ] {
            let f = p.tmp.path().join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(&f, b).unwrap();
        }
        p
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    fn drive(&self, d: &str) -> PathBuf {
        self.tmp.path().join(d)
    }

    fn add_and_scan(&self, d: &str) {
        ok(
            &self.data(),
            &["sources", "add", self.drive(d).to_str().unwrap()],
        );
        ok(&self.data(), &["scan"]);
    }

    fn export(&self) {
        let lib = self.tmp.path().join("library");
        ok(
            &self.data(),
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
    }

    /// The only group's id and the file id of the member whose path ends with `suffix`.
    fn group_and_file(&self, suffix: &str) -> (i64, i64) {
        let list = json(&ok(&self.data(), &["--json", "groups", "list"]));
        let g = &list["data"]["groups"][0];
        let id = g["id"].as_i64().unwrap();
        let d = json(&ok(
            &self.data(),
            &["--json", "groups", "show", &id.to_string()],
        ));
        let m = d["data"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["path"].as_str().unwrap().ends_with(suffix))
            .unwrap()
            .clone();
        (id, m["file"].as_i64().unwrap())
    }
}

/// The gate re-review's repro with JPEGs: a lesser keeper chosen by the user keeps the better original out of
/// every cleanup plan (ADR-0096), in the text output and `--json`.
#[test]
fn cleanup_cli_keeps_a_copy_better_than_the_keeper_you_chose() {
    let p = Pair::new("chosen-keeper");
    p.add_and_scan("drive-a");
    p.export();
    p.add_and_scan("drive-b");
    let (g, web) = p.group_and_file("Web/IMG_0001.jpg");
    ok(
        &p.data(),
        &["groups", "set-keeper", &g.to_string(), &web.to_string()],
    );
    let o = ok(&p.data(), &["cleanup", "plan", "--source", "2"]);
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("kept ")
            && text.contains("DCIM/IMG_0001.jpg: better than the keeper you chose")
            && text.contains("nothing to clean up"),
        "{text}"
    );
    let v = json(&ok(
        &p.data(),
        &["--json", "cleanup", "plan", "--source", "2"],
    ));
    assert!(v["data"]["run"].is_null(), "{v}");
    assert_eq!(
        v["data"]["held"][0]["reason"]["kind"], "better_than_keeper",
        "{v}"
    );
    assert!(p.drive("drive-b").join("DCIM/IMG_0001.jpg").exists());
}

/// A plan re-checked when it runs (ADR-0096): drive-b's original is exported and its web copy on drive-a covered
/// and planned; then the user makes the web copy the keeper, which is not in the library itself. The run skips
/// it as "no longer safe" (text, `--json` of `cleanup show`) and leaves it in place.
#[test]
fn cleanup_cli_run_skips_a_file_that_became_unsafe_after_planning() {
    let p = Pair::new("stale-plan");
    p.add_and_scan("drive-a");
    p.add_and_scan("drive-b");
    p.export();
    let v = json(&ok(
        &p.data(),
        &["--json", "cleanup", "plan", "--source", "1"],
    ));
    let run = v["data"]["run"].as_i64().unwrap();
    assert_eq!(v["data"]["moves"][0]["disposition"], "covered", "{v}");
    let (g, web) = p.group_and_file("Web/IMG_0001.jpg");
    ok(
        &p.data(),
        &["groups", "set-keeper", &g.to_string(), &web.to_string()],
    );
    let o = ok(&p.data(), &["cleanup", "run", &run.to_string(), "--yes"]);
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("moved 0 files")
            && text.contains("1 skipped")
            && text.contains("not moved: Web/IMG_0001.jpg: no longer safe"),
        "{text}"
    );
    let v = json(&ok(
        &p.data(),
        &["--json", "cleanup", "show", &run.to_string()],
    ));
    let s = v.to_string();
    assert!(
        s.contains("\"skipped\"") && s.contains("no longer safe"),
        "{s}"
    );
    assert!(p.drive("drive-a").join("Web/IMG_0001.jpg").exists());
}
