//! Large previews of videos (video-previews-and-play): a 2048 px poster frame from the poster-frame tool, cached,
//! one tool call at a time, and the placeholder's reasons when there is no frame. The tools are shell scripts.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ps_model::ItemId;
use ps_model::library::Preview;
use ps_service::library::Library;
use ps_service::scan::{ScanOptions, ScanService};

/// A stand-in for ffmpeg: appends a line to `calls` (with the `-vf` filter), sleeps `delay`, then prints the
/// JPEG `frame`, or fails with `error` on stderr when it is set.
fn tool(dir: &Path, name: &str, frame: &Path, delay: &str, error: Option<&str>) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let calls = dir.join(format!("{name}.calls"));
    let tail = match error {
        Some(e) => format!("echo '{e}' >&2\nexit 1"),
        None => format!("cat '{}'", frame.display()),
    };
    let script = format!(
        "#!/bin/sh\nvf=\nwhile [ $# -gt 0 ]; do [ \"$1\" = -vf ] && vf=$2; shift; done\n\
         echo \"start $vf\" >> '{c}'\nsleep {delay}\necho end >> '{c}'\n{tail}\n",
        c = calls.display()
    );
    let p = dir.join(name);
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn calls(dir: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join(format!("{name}.calls")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    tools: PathBuf,
    data: PathBuf,
    frame: PathBuf,
}

/// Two iPhone MOVs, catalogued by a scan without a poster tool.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("phone");
    std::fs::create_dir_all(root.join("DCIM")).unwrap();
    for i in 0..2i64 {
        let m = ps_fixtures::media::movie(&ps_fixtures::media::MovieSpec {
            creation_unix: Some(1_562_265_000 + i * 60),
            ..ps_fixtures::media::MovieSpec::iphone()
        });
        std::fs::write(root.join(format!("DCIM/IMG_500{i}.MOV")), m).unwrap();
    }
    let tools = tmp.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let frame = tools.join("frame.jpg");
    std::fs::write(&frame, ps_fixtures::media::jpeg(1920, 1080, 40, 85)).unwrap();
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    Fixture {
        _tmp: tmp,
        tools,
        data,
        frame,
    }
}

fn library(f: &Fixture, poster_tool: Option<PathBuf>) -> Library {
    let svc = ScanService::open(
        &f.data,
        ScanOptions {
            poster_tool,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    Library::with_service(&f.data, svc)
}

#[test]
fn scenario_thumbnails_large_preview_of_a_mov() {
    let f = fixture();
    let ok = tool(&f.tools, "ffmpeg", &f.frame, "0", None);
    let lib = library(&f, Some(ok));
    let Preview::Jpeg(jpeg) = lib.preview(ItemId(1)).unwrap() else {
        panic!("no large preview");
    };
    let (img, _) = ps_media::decode::decode_jpeg(&jpeg, 4096).unwrap();
    assert!(img.width.max(img.height) > 384 && img.width <= 1920 && img.height <= 1080);
    let c = calls(&f.tools, "ffmpeg");
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(
        c[0].contains("min(2048,iw)"),
        "asked for the 2048 px edge: {c:?}"
    );
    // The second request is served from the preview cache.
    assert!(matches!(lib.preview(ItemId(1)).unwrap(), Preview::Jpeg(_)));
    assert_eq!(
        calls(&f.tools, "ffmpeg").len(),
        2,
        "the tool is not called again"
    );
}

#[test]
fn scenario_thumbnails_large_preview_without_the_tool() {
    let f = fixture();
    let lib = library(&f, None);
    match lib.preview(ItemId(1)).unwrap() {
        Preview::Unavailable(why) => {
            assert_eq!(why, ps_service::library::NO_POSTER_TOOL_REASON);
            assert_ne!(why, "video");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn scenario_thumbnails_large_preview_when_the_tool_fails() {
    let f = fixture();
    let bad = tool(
        &f.tools,
        "ffmpeg",
        &f.frame,
        "0",
        Some("moov atom not found"),
    );
    let lib = library(&f, Some(bad));
    match lib.preview(ItemId(1)).unwrap() {
        Preview::Unavailable(why) => {
            assert!(why.starts_with("poster frame failed"), "{why}");
            assert!(why.contains("moov atom not found"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

/// Two requests at once run the tool one after the other.
#[test]
fn concurrent_video_previews_run_the_tool_one_at_a_time() {
    let f = fixture();
    let slow = tool(&f.tools, "ffmpeg", &f.frame, "0.3", None);
    let lib = std::sync::Arc::new(library(&f, Some(slow)));
    let threads: Vec<_> = [ItemId(1), ItemId(2)]
        .into_iter()
        .map(|id| {
            let lib = lib.clone();
            std::thread::spawn(move || lib.preview(id).unwrap())
        })
        .collect();
    for t in threads {
        assert!(matches!(t.join().unwrap(), Preview::Jpeg(_)));
    }
    let c = calls(&f.tools, "ffmpeg");
    assert_eq!(c.len(), 4, "{c:?}");
    for pair in c.chunks(2) {
        assert!(
            pair[0].starts_with("start") && pair[1] == "end",
            "overlapping calls: {c:?}"
        );
    }
}
