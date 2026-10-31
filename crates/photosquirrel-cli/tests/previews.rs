//! `cli` scenarios of `preview-placeholders` (ADR-0109): `previews status` and `previews fill`, driving the built
//! binary. `PHOTOSQUIRREL_HEIF_DECODER=none` makes the machine lack a HEIC decoder; `=fake` (debug builds only)
//! gives it one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn cli(data: &Path, heif: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_photosquirrel-cli"));
    c.env("PHOTOSQUIRREL_DATA_DIR", data)
        .env("PHOTOSQUIRREL_HEIF_DECODER", heif)
        .env_remove("PHOTOSQUIRREL_CONFIG_DIR")
        .stdin(Stdio::null());
    c
}

fn ok(data: &Path, heif: &str, args: &[&str]) -> Output {
    let o = cli(data, heif).args(args).output().expect("run cli");
    assert!(
        o.status.success(),
        "{args:?} exited {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

/// Four HEIC photos and one JPEG whose data ends after its headers, in a temp folder.
fn photos() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let sample = include_bytes!("../../ps-sysdecode/data/selftest.heic");
    std::fs::create_dir_all(dir.path().join("DCIM")).unwrap();
    for i in 0..4u8 {
        let b =
            ps_fixtures::heif::heif_with_transforms(sample, i % 4, (i == 3).then_some(0)).unwrap();
        std::fs::write(dir.path().join(format!("DCIM/IMG_{i}.HEIC")), b).unwrap();
    }
    let full = ps_fixtures::media::jpeg(1600, 1200, 77, 90);
    let sos = full.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let len = u16::from_be_bytes([full[sos + 2], full[sos + 3]]) as usize;
    std::fs::write(dir.path().join("DCIM/IMG_9.JPG"), &full[..sos + 2 + len]).unwrap();
    dir
}

/// A data folder with `photos` added and scanned without a HEIC decoder.
fn scanned(photos: &Path) -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    ok(
        data.path(),
        "none",
        &["sources", "add", photos.to_str().unwrap()],
    );
    ok(data.path(), "none", &["scan"]);
    data
}

fn rows(o: &Output) -> Vec<Value> {
    let s = String::from_utf8_lossy(&o.stdout);
    let v: Value = serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("{e}: {s}"));
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["command"], "previews status");
    v["data"].as_array().unwrap().clone()
}

fn row<'a>(rows: &'a [Value], format: &str, reason: &str) -> Option<&'a Value> {
    rows.iter()
        .find(|r| r["format"] == format && r["reason"] == reason)
}

#[test]
fn scenario_cli_status_counts_by_reason() {
    let photos = photos();
    let data = scanned(photos.path());
    let o = ok(data.path(), "none", &["--json", "previews", "status"]);
    let rows = rows(&o);
    let heic = row(&rows, "heic", "decoder_missing").unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(
        (heic["count"].as_u64(), heic["fillable"].as_u64()),
        (Some(4), Some(0))
    );
    let jpeg = row(&rows, "jpeg", "unreadable").unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(
        (jpeg["count"].as_u64(), jpeg["fillable"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(rows.len(), 2, "{rows:?}");
    // The human table says the same.
    let text = String::from_utf8_lossy(&ok(data.path(), "none", &["previews", "status"]).stdout)
        .into_owned();
    assert!(text.contains("needs a library"), "{text}");
    assert!(text.contains("could not be read"), "{text}");
}

#[test]
fn scenario_cli_fill_one_format() {
    let photos = photos();
    let data = scanned(photos.path());
    // A HEIC decoder became available.
    let status = rows(&ok(data.path(), "fake", &["--json", "previews", "status"]));
    assert_eq!(
        row(&status, "heic", "decoder_missing").unwrap()["fillable"],
        4
    );
    let o = ok(
        data.path(),
        "fake",
        &["--json", "previews", "fill", "--format", "heic"],
    );
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let summary: Value = out
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "summary")
        .unwrap_or_else(|| panic!("{out}"));
    assert_eq!(summary["command"], "previews fill");
    let d = &summary["data"];
    assert_eq!(d["filled"], 4, "{d}");
    assert_eq!(d["skipped_offline"], 0, "{d}");
    assert_eq!(d["failed"], 0, "only HEIC placeholders were decoded: {d}");
    assert_eq!(d["still_waiting"], 0, "{d}");
    // The damaged JPEG was not touched.
    let after = rows(&ok(data.path(), "fake", &["--json", "previews", "status"]));
    assert!(
        row(&after, "heic", "decoder_missing").is_none(),
        "{after:?}"
    );
    assert_eq!(row(&after, "jpeg", "unreadable").unwrap()["count"], 1);
    // The human summary.
    let text = String::from_utf8_lossy(&ok(data.path(), "fake", &["previews", "fill"]).stdout)
        .into_owned();
    assert!(text.contains("0 previews filled in"), "{text}");
    // An unknown format is a usage mistake reported plainly.
    let bad = cli(data.path(), "fake")
        .args(["previews", "fill", "--format", "nope"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown format"));
}

/// `previews fill --reason poster_failed` (ADR-0114): a video whose poster frame failed is not retried by a plain
/// fill, and the explicit retry runs the tool once more on it. The tool is a fake `ffmpeg` first on `PATH`, logging
/// every call (Unix only).
#[cfg(unix)]
#[test]
fn previews_fill_retries_failed_posters() {
    use std::os::unix::fs::PermissionsExt;
    let photos = tempfile::tempdir().unwrap();
    for i in 0..2i64 {
        let m = ps_fixtures::media::movie(&ps_fixtures::media::MovieSpec {
            creation_unix: Some(1_562_265_000 + i * 60),
            ..ps_fixtures::media::MovieSpec::iphone()
        });
        std::fs::write(photos.path().join(format!("IMG_{i}.MOV")), m).unwrap();
    }
    let tools = tempfile::tempdir().unwrap();
    let frame = tools.path().join("frame.jpg");
    std::fs::write(&frame, ps_fixtures::media::jpeg(640, 360, 9, 85)).unwrap();
    let log = tools.path().join("calls.log");
    let ffmpeg = tools.path().join("ffmpeg");
    let write_tool = |fail: bool| {
        let body = if fail {
            "case \"$src\" in *IMG_1.MOV) echo \"$src: moov atom not found\" >&2; exit 1;; esac\n"
        } else {
            ""
        };
        std::fs::write(
            &ffmpeg,
            format!(
                "#!/bin/sh\nsrc=\nwhile [ $# -gt 0 ]; do [ \"$1\" = -i ] && src=$2; shift; done\necho \"$src\" >> \
                 '{}'\n{body}cat '{}'\n",
                log.display(),
                frame.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    write_tool(true);
    let calls = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count()
    };
    let config = tempfile::tempdir().unwrap();
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let data = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| -> String {
        let o = cli(data.path(), "none")
            .env("PATH", &path)
            .env("PHOTOSQUIRREL_CONFIG_DIR", config.path())
            .args(args)
            .output()
            .expect("run cli");
        assert!(
            o.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    run(&["sources", "add", photos.path().to_str().unwrap()]);
    run(&["scan"]);
    assert_eq!(calls(), 2, "the scan ran ffmpeg once per video");
    let status: Value =
        serde_json::from_str(run(&["--json", "previews", "status"]).trim()).unwrap();
    let rows = status["data"].as_array().unwrap().clone();
    let failed = row(&rows, "mov", "poster_failed").unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(
        (failed["count"].as_u64(), failed["fillable"].as_u64()),
        (Some(1), Some(0))
    );
    let text = run(&["previews", "status"]);
    assert!(text.contains("poster frame failed"), "{text}");
    // A plain fill does not run the tool on it again.
    let text = run(&["previews", "fill"]);
    assert!(text.contains("0 previews filled in"), "{text}");
    assert_eq!(calls(), 2);
    // The explicit retry runs it once more (still failing).
    run(&["previews", "fill", "--reason", "poster_failed"]);
    assert_eq!(calls(), 3);
    // After an ffmpeg update the retry gives it its frame.
    write_tool(false);
    let out = run(&["--json", "previews", "fill", "--reason", "poster_failed"]);
    let summary: Value = out
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "summary")
        .unwrap_or_else(|| panic!("{out}"));
    assert_eq!(summary["data"]["filled"], 1, "{summary}");
    assert_eq!(calls(), 4);
    let status: Value =
        serde_json::from_str(run(&["--json", "previews", "status"]).trim()).unwrap();
    assert!(
        status["data"].as_array().unwrap().is_empty(),
        "every video has its poster frame: {status}"
    );
}
