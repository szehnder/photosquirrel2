//! capture-dates from the CLI (tasks 5.5, 6.1): the export plan's dates to their precision, the warning for a
//! library another catalog wrote, and `scan --refresh-analysis`. Every test drives the built binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::Value;

fn cli(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
    c.env("PHOTOSQUIRREL_DATA_DIR", data)
        .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
        // File times are read in the machine's zone (ADR-0070): pin it where the OS honours TZ.
        .env("TZ", "UTC")
        .stdin(Stdio::null());
    c
}

fn ok(data: &Path, args: &[&str]) -> Output {
    let o = cli(data).args(args).output().expect("run cli");
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
    serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("{e}: {s}"))
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    // 2024-03-01T12:00:00Z
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_709_294_400);
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .unwrap();
}

fn photo(seed: u64) -> Vec<u8> {
    ps_fixtures::media::jpeg(800, 600, seed, 88)
}

fn scanned(src: &Path) -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    ok(data.path(), &["sources", "add", src.to_str().unwrap()]);
    ok(data.path(), &["scan"]);
    data
}

fn dry_run(data: &Path, lib: &Path) -> Value {
    json(&ok(
        data,
        &[
            "--json",
            "export",
            "dry-run",
            "--library",
            lib.to_str().unwrap(),
        ],
    ))
}

/// The CLI JSON half; the resolver half is in `ps-media`'s media-metadata tests.
#[test]
fn scenario_media_metadata_month_folder_stays_a_month() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src.join("2019-07 Italy/IMG_0001.JPG"), &photo(61));
    let data = scanned(&src);
    let v = dry_run(data.path(), &tmp.path().join("Library"));
    let files = v["data"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{v}");
    let f = &files[0];
    assert_eq!(f["date"], "2019-07", "{f}");
    assert_eq!(f["precision"], "month");
    assert_eq!(f["date_source"], "folder name");
    assert_eq!(f["target"], "2019/2019-07/IMG_0001.JPG");
    assert!(
        !f.to_string().contains("2019-07-01"),
        "no day or time is reported: {f}"
    );
}

/// Every file under `root` with its bytes and modification time.
fn files_under(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.clone(),
                    (std::fs::read(&p).unwrap(), m.modified().unwrap()),
                );
            }
        }
    }
    out
}

#[test]
fn scenario_export_unknown_library_warned_in_the_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    for i in 0..4u64 {
        write(
            &src.join(format!("2019-07-04 Beach/IMG_{i:04}.JPG")),
            &photo(70 + i * 101),
        );
    }
    write(&src.join("Misc/IMG_0100.JPG"), &photo(999));
    let lib = tmp.path().join("Library");

    // The first catalog writes the library; planning again it has its own ledger rows: no warning.
    let a = scanned(&src);
    ok(
        a.path(),
        &["export", "run", "--library", lib.to_str().unwrap(), "--yes"],
    );
    assert!(
        dry_run(a.path(), &lib)["data"]["warnings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let before = files_under(&lib);
    assert!(
        before
            .keys()
            .any(|p| p.ends_with(".photosquirrel/manifest.jsonl"))
    );

    // A second catalog of the same source chooses that library.
    let b = scanned(&src);
    let v = dry_run(b.path(), &lib);
    let warnings = v["data"]["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    assert_eq!(warnings[0]["kind"], "other_catalog_library");
    let text = v["data"]["warning_texts"][0].as_str().unwrap();
    assert!(text.contains("another catalog"), "{text}");
    // Text output says so too.
    let o = ok(
        b.path(),
        &["export", "dry-run", "--library", lib.to_str().unwrap()],
    );
    assert!(stdout(&o).contains("warning: this library was written by another catalog"));
    // The export still runs and never overwrites: every file already there keeps its bytes and time.
    ok(
        b.path(),
        &["export", "run", "--library", lib.to_str().unwrap(), "--yes"],
    );
    let after = files_under(&lib);
    for (p, (bytes, mtime)) in &before {
        if p.ends_with(".photosquirrel/manifest.jsonl") {
            // Append-only: the first catalog's lines are still its first lines.
            assert!(after[p].0.starts_with(bytes), "manifest only appended to");
            continue;
        }
        assert_eq!(&after[p].0, bytes, "{} unchanged", p.display());
        assert_eq!(after[p].1, *mtime, "{} unchanged", p.display());
    }
}

#[test]
fn scan_refresh_analysis_flag_refreshes_rows_without_class_facts() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    for i in 0..3u64 {
        write(&src.join(format!("IMG_{i:04}.JPG")), &photo(500 + i * 37));
    }
    let data = scanned(&src);
    // As if probed before catalog migration 13.
    let c = rusqlite::Connection::open(data.path().join("catalog.db")).unwrap();
    c.execute(
        "UPDATE file SET class_facts = NULL, classifier_hash = NULL",
        [],
    )
    .unwrap();
    drop(c);
    let o = ok(data.path(), &["scan"]);
    assert!(
        stdout(&o).contains("3 files were analysed by an older version"),
        "{}",
        stdout(&o)
    );
    let o = ok(data.path(), &["scan", "--refresh-analysis"]);
    assert!(stdout(&o).contains("completed"), "{}", stdout(&o));
    let c = rusqlite::Connection::open(data.path().join("catalog.db")).unwrap();
    let with_facts: i64 = c
        .query_row(
            "SELECT count(*) FROM file WHERE class_facts IS NOT NULL AND classifier_hash IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(with_facts, 3);
    drop(c);
    let o = ok(data.path(), &["scan"]);
    assert!(
        !stdout(&o).contains("analysed by an older version"),
        "{}",
        stdout(&o)
    );
}
