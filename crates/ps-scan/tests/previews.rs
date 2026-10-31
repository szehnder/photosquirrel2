//! `incremental-rescan` scenarios of `preview-placeholders` (ADR-0109): the Fill in previews job re-decodes only
//! the placeholders that can become previews now, reading each once and listing no folder; placeholders of an older
//! decoder generation are re-queued by a rescan, current-generation failures are not.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use ps_fixtures::CorpusWriter as _;
use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::{
    DecodeFamily, FillScope, Format, PreviewReason, RelPath, SourceId, Stage, ThumbKind,
};
use ps_scan::ScanOptions;

const HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

fn no_decoder() -> ScanOptions {
    ScanOptions {
        decoders: Arc::new(Decoders::none()),
        ..ScanOptions::default()
    }
}

fn with(fake: FakeHeif) -> ScanOptions {
    ScanOptions {
        decoders: Arc::new(Decoders::with_heif(Arc::new(fake))),
        ..ScanOptions::default()
    }
}

/// Five different HEIC files (the sample re-boxed with different transformations).
fn heics() -> Vec<Vec<u8>> {
    [(0, None), (1, None), (2, None), (3, None), (0, Some(0))]
        .into_iter()
        .map(|(a, m)| ps_fixtures::heif::heif_with_transforms(HEIC, a, m).expect("rebox"))
        .collect()
}

struct Source {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

fn source(name: &str, files: &[(String, Vec<u8>)]) -> Source {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join(name);
    let w = ps_fixtures::DirWriter::new(&root).expect("writer");
    for (rel, b) in files {
        w.write(rel, b, 1_600_000_000).expect("write");
    }
    Source { tmp, root }
}

/// `jpegs` small JPEGs and 20 HEICs (the five variants in four folders).
fn jpegs_and_twenty_heics(jpegs: u64) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = (0..jpegs)
        .map(|i| {
            (
                format!("DCIM/{:03}/IMG_{i:04}.JPG", i / 100),
                ps_fixtures::media::jpeg(320, 240, 500 + i, 80),
            )
        })
        .collect();
    let h = heics();
    for folder in 0..4 {
        for (i, b) in h.iter().enumerate() {
            v.push((format!("Phone/{folder}/IMG_{:04}.HEIC", 100 + i), b.clone()));
        }
    }
    v
}

fn under(root: &std::path::Path, rel: &str) -> PathBuf {
    RelPath::from_bytes(rel.as_bytes().to_vec())
        .expect("rel")
        .under(root)
}

fn fill(env: &Env, scope: FillScope) -> ps_scan::RunSummary {
    env.s().start_fill(scope, &[]).expect("start fill");
    env.s().wait().expect("summary")
}

fn heic_rows(env: &Env, id: SourceId) -> Vec<(String, ps_catalog::FileRecord)> {
    env.files(id)
        .into_iter()
        .filter(|(p, _)| p.ends_with(".HEIC"))
        .collect()
}

#[test]
fn scenario_incremental_rescan_fill_in_reads_only_waiting_files() {
    let _g = shared();
    let src = source("card", &jpegs_and_twenty_heics(480));
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    assert_eq!(env.files(id).len(), 500);
    assert_eq!(heic_rows(&env, id).len(), 20);
    // The decoder becomes available (installed, app restarted).
    env.reopen(with(FakeHeif::new()));
    let status = env.s().preview_status(&[]).expect("status");
    let heic = status
        .iter()
        .find(|r| r.format == "heic")
        .expect("heic row");
    assert_eq!((heic.count, heic.fillable), (20, 20));
    let s = fill(&env, FillScope::all());
    completed(&s);
    let f = s.fill.expect("a fill summary");
    assert_eq!(f.filled, 20);
    assert_eq!((f.skipped_offline, f.failed, f.still_waiting), (0, 0, 0));
    assert_eq!(env.instr.read_dirs(), 0, "no directory listed");
    let root = env.root(id);
    let per = env.instr.bytes_per_file();
    assert_eq!(per.len(), 20, "only the waiting files are opened: {per:?}");
    assert_eq!(env.instr.open_count(), 20, "each opened once");
    for (rel, r) in heic_rows(&env, id) {
        assert_eq!(
            per.get(&under(&root, &rel)),
            Some(&r.stat.size),
            "{rel} read once in full"
        );
        assert!(r.hashes.is_some(), "{rel}");
        assert_eq!(r.preview, None, "{rel}: no placeholder any more");
        assert_eq!(r.decoder_missing, None);
        let t = env.s().thumbs().get(&r.identity.unwrap()).unwrap().unwrap();
        assert_eq!(t.kind, ThumbKind::Image, "{rel}");
    }
    let status = env.s().preview_status(&[]).expect("status");
    assert!(status.iter().all(|r| r.format != "heic"), "{status:?}");
}

#[test]
fn scenario_incremental_rescan_filled_in_heic_joins_its_jpeg_group() {
    let _g = shared();
    let rgb = ps_fixtures::media::photo_rgb(1280, 960, 4243);
    let small = {
        let img = ps_media::RgbImage {
            width: 1280,
            height: 960,
            data: rgb.clone(),
        };
        let r = ps_media::decode::resize_to_long_edge(&img, 800);
        ps_fixtures::media::encode_jpeg(&r.data, r.width, r.height, 85)
    };
    let src = source(
        "phone",
        &[
            ("DCIM/IMG_0001.HEIC".into(), HEIC.to_vec()),
            ("Web/IMG_0001.jpg".into(), small),
        ],
    );
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    assert!(env.readers().group_memberships().unwrap().is_empty());
    let fake = FakeHeif::new();
    fake.give(
        HEIC,
        ps_media::RgbImage {
            width: 1280,
            height: 960,
            data: rgb,
        },
    );
    env.reopen(with(fake));
    let s = fill(&env, FillScope::format(Format::Heic));
    completed(&s);
    assert_eq!(s.fill.expect("fill").filled, 1);
    let groups = env.readers().group_memberships().unwrap();
    assert_eq!(groups.len(), 1, "one duplicate group");
    let rows = env.files(id);
    let members: Vec<_> = groups[0].1.clone();
    assert!(members.contains(&rows["DCIM/IMG_0001.HEIC"].id));
    assert!(members.contains(&rows["Web/IMG_0001.jpg"].id));
}

#[test]
fn scenario_incremental_rescan_offline_files_are_skipped_and_counted() {
    let _g = shared();
    let h = heics();
    let a = source(
        "laptop",
        &(0..3)
            .map(|i| (format!("IMG_{i}.HEIC"), h[i].clone()))
            .collect::<Vec<_>>(),
    );
    let b = source(
        "usb",
        &(0..5)
            .map(|i| (format!("x/IMG_{i}.HEIC"), h[i].clone()))
            .collect::<Vec<_>>(),
    );
    let mut env = env_with(no_decoder());
    let ia = env.add(&a.root);
    let ib = env.add(&b.root);
    completed(&env.scan(&[]));
    // The USB drive is unplugged.
    ps_fixtures::scratch::rename(&b.root, b.tmp.path().join("unplugged")).expect("unplug");
    env.reopen(with(FakeHeif::new()));
    let s = fill(&env, FillScope::all());
    completed(&s);
    let f = s.fill.clone().expect("fill");
    assert_eq!(f.skipped_offline, 5);
    assert_eq!(f.filled, 3);
    assert!(s.incomplete_sources.contains(&ib), "{s:?}");
    for (_, r) in heic_rows(&env, ia) {
        assert!(r.hashes.is_some());
    }
    for (rel, r) in heic_rows(&env, ib) {
        assert!(r.hashes.is_none(), "{rel} still waits");
        assert_eq!(r.decoder_missing, Some(ps_model::DecoderKey::Heif));
    }
    let status = env.s().preview_status(&[ib]).expect("status");
    assert_eq!(status.iter().map(|r| r.fillable).sum::<u64>(), 5);
}

#[test]
fn scenario_incremental_rescan_fill_in_can_be_cancelled() {
    let _g = shared();
    let src = source("card", &jpegs_and_twenty_heics(0));
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    // A slow device: one reader, a pause before every file.
    env.reopen(ScanOptions {
        max_readers: 1,
        read_delay: Some(Duration::from_millis(40)),
        ..with(FakeHeif::new())
    });
    env.s().start_fill(FillScope::all(), &[]).expect("start");
    let snap = wait_snapshot(env.s(), Duration::from_secs(60), "10 filled", |s| {
        s.fill && s.diff.redecoded >= 10
    });
    assert!(snap.fill, "the dashboard sees a fill job");
    env.s().cancel().expect("cancel");
    let s = env.s().wait().expect("summary");
    assert_eq!(s.status, ps_model::RunStatus::Cancelled);
    let done = heic_rows(&env, id)
        .iter()
        .filter(|(_, r)| r.hashes.is_some())
        .count() as u64;
    assert!((10..20).contains(&done), "{done} filled before the cancel");
    // The filled ones keep their previews; the rest still wait, and only they are read next time.
    let waiting = env.s().preview_status(&[]).expect("status");
    assert_eq!(waiting.iter().map(|r| r.fillable).sum::<u64>(), 20 - done);
    env.reopen(with(FakeHeif::new()));
    let s = fill(&env, FillScope::all());
    completed(&s);
    assert_eq!(s.fill.expect("fill").filled, 20 - done);
    assert_eq!(env.instr.open_count() as u64, 20 - done);
    assert!(heic_rows(&env, id).iter().all(|(_, r)| r.hashes.is_some()));
}

/// TIFFs a version with TIFF generation 0 catalogued with placeholders (0.2.0): their rows as migration 24 leaves
/// them (generation 0, no perceptual hashes) and placeholder thumbnails.
fn age_tiffs(env: &mut Env, id: SourceId, names: &[&str]) {
    let rows = env.files(id);
    for n in names {
        let r = &rows[*n];
        let probe = ps_media::probe_bytes(&[], 0, None);
        let mut ph = ps_media::thumb::placeholder(&probe, None, "preview unavailable");
        if let Some(p) = ph.placeholder.as_mut() {
            p.generation = 0;
            p.format = Some(Format::Tiff);
        }
        env.s()
            .thumbs()
            .put_many([(&r.identity.unwrap(), &ph)])
            .unwrap();
    }
    let dir = env.dir.clone();
    env.close();
    let c = rusqlite::Connection::open(dir.join(ps_catalog::CATALOG_FILE)).unwrap();
    for n in names {
        c.execute(
            "UPDATE file SET phash = NULL, dhash = NULL, hash_version = NULL, preview_gen = 0,
                             preview_reason = 'unreadable'
             WHERE rel_path = CAST(?1 AS BLOB)",
            [n],
        )
        .unwrap();
    }
}

fn tiff(seed: u64) -> Vec<u8> {
    ps_fixtures::tiff_image::tiff_image(&ps_fixtures::tiff_image::TiffSpec::new(640, 480, seed))
}

#[test]
fn scenario_incremental_rescan_rescan_after_an_update_fills_in_old_tiff_placeholders() {
    let _g = shared();
    let mut files: Vec<(String, Vec<u8>)> = (0..3)
        .map(|i| (format!("scans/scan_{i}.tif"), tiff(70 + i)))
        .collect();
    files.extend((0..12).map(|i| {
        (
            format!("DCIM/IMG_{i:04}.JPG"),
            ps_fixtures::media::jpeg(320, 240, 900 + i, 80),
        )
    }));
    let src = source("scans", &files);
    let mut env = env_with(ScanOptions::default());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let names = ["scans/scan_0.tif", "scans/scan_1.tif", "scans/scan_2.tif"];
    age_tiffs(&mut env, id, &names);
    env.reopen(ScanOptions::default());
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.outdated_previews, 3);
    assert_eq!(s.diff.redecoded, 3);
    let root = env.root(id);
    let per = env.instr.bytes_per_file();
    assert_eq!(
        per.len(),
        3,
        "the rest of the source reads no file bytes: {per:?}"
    );
    let rows = env.files(id);
    for n in names {
        let r = &rows[n];
        assert_eq!(per.get(&under(&root, n)), Some(&r.stat.size), "{n} once");
        assert!(r.hashes.is_some(), "{n}");
        assert_eq!(r.preview, None, "{n}");
        let t = env.s().thumbs().get(&r.identity.unwrap()).unwrap().unwrap();
        assert_eq!(t.kind, ThumbKind::Image, "{n}");
    }
    // Once: the next rescan reads nothing.
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(env.instr.total_bytes(), 0);
    assert_eq!(s.diff.outdated_previews, 0);
}

#[test]
fn scenario_incremental_rescan_current_generation_failures_are_not_retried() {
    let _g = shared();
    let src = source(
        "scans",
        &[
            (
                "big/scan.tif".into(),
                ps_fixtures::tiff_image::bigtiff_rgb(640, 480, 5),
            ),
            ("ok/scan.tif".into(), tiff(6)),
        ],
    );
    let mut env = env_with(ScanOptions::default());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let big = env.files(id)["big/scan.tif"].clone();
    assert_eq!(big.stage, Stage::Thumbed, "catalogued");
    assert_eq!(
        big.preview.map(|p| (p.generation, p.reason)),
        Some((1, PreviewReason::UnsupportedVariant))
    );
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    let root = env.root(id);
    assert!(
        env.instr
            .opens()
            .iter()
            .all(|o| o.path != under(&root, "big/scan.tif")),
        "the BigTIFF is not opened"
    );
    assert_eq!(env.instr.total_bytes(), 0);
    // A version with a newer TIFF generation (simulated) decodes it again, once; still unsupported, it then
    // carries that generation and is left alone.
    let newer = ps_media::decoders::GENERATIONS.with(DecodeFamily::Tiff, 2);
    env.reopen(ScanOptions {
        generations: newer,
        ..ScanOptions::default()
    });
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.outdated_previews, 1);
    assert_eq!(
        env.files(id)["big/scan.tif"]
            .preview
            .map(|p| (p.generation, p.reason)),
        Some((2, PreviewReason::UnsupportedVariant))
    );
    env.instr.reset();
    completed(&env.scan(&[id]));
    assert_eq!(env.instr.total_bytes(), 0, "not read again");
}

/// A re-decode that still cannot decode a file never replaces stored pixels with its placeholder (the same guard
/// as a full analysis): when another copy of the content already has a preview, the row shows it.
#[test]
fn redecode_failure_keeps_stored_pixels() {
    let _g = shared();
    let src = source(
        "scans",
        &[
            (
                "big/scan.tif".into(),
                ps_fixtures::tiff_image::bigtiff_rgb(640, 480, 5),
            ),
            ("ok/scan.tif".into(), tiff(6)),
        ],
    );
    let mut env = env_with(ScanOptions::default());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let files = env.files(id);
    let big = files["big/scan.tif"].identity.unwrap();
    let ok = files["ok/scan.tif"].identity.unwrap();
    // Pixels stored for the BigTIFF's content (as if another copy had been decoded).
    let pixels = env.s().thumbs().get(&ok).unwrap().unwrap();
    assert_eq!(pixels.kind, ThumbKind::Image);
    env.s().thumbs().put(&big, &pixels).unwrap();
    // A newer TIFF generation re-decodes the BigTIFF; it is still not supported.
    let newer = ps_media::decoders::GENERATIONS.with(DecodeFamily::Tiff, 2);
    env.reopen(ScanOptions {
        generations: newer,
        ..ScanOptions::default()
    });
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.outdated_previews, 1);
    let t = env.s().thumbs().get(&big).unwrap().unwrap();
    assert_eq!(t.kind, ThumbKind::Image, "the stored pixels are kept");
    assert_eq!(
        env.files(id)["big/scan.tif"].preview,
        None,
        "the row shows the stored preview"
    );
}

/// `video-posters-after-ffmpeg` (ADR-0114): videos catalogued without a poster-frame tool get their frames once a
/// tool is available, from Fill in previews or a rescan, with one call of the tool each and no byte of them read by
/// PhotoSquirrel; a failed call is recorded and not repeated until the user asks. The tools are shell scripts in a
/// temp dir that log every call (Unix only, as `ps-media/tests/thumbnails.rs`).
#[cfg(unix)]
mod posters {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A fake poster-frame tool: logs the `-i` argument of every call to `calls.log`, then runs `body`
    /// (`$src` and `$ss` are set).
    struct Tool {
        _dir: tempfile::TempDir,
        path: PathBuf,
        log: PathBuf,
        frame: PathBuf,
    }

    impl Tool {
        fn new(body: &str) -> Tool {
            let dir = tempfile::tempdir().expect("tool dir");
            let frame = dir.path().join("frame.jpg");
            ps_fixtures::scratch::write(&frame, ps_fixtures::media::jpeg(640, 360, 9, 85))
                .expect("frame");
            let log = dir.path().join("calls.log");
            let path = dir.path().join("fake-ffmpeg");
            let script = format!(
                "#!/bin/sh\nss=; src=\nwhile [ $# -gt 0 ]; do case \"$1\" in -ss) ss=$2;; -i) src=$2;; esac; \
                 shift; done\necho \"$src\" >> '{}'\n{}\n",
                log.display(),
                body.replace("FRAME", &frame.display().to_string())
            );
            ps_fixtures::scratch::write(&path, script).expect("tool");
            ps_fixtures::scratch::set_mode(&path, 0o755).expect("chmod");
            Tool {
                _dir: dir,
                path,
                log,
                frame,
            }
        }

        /// A tool that gives a frame for every video.
        fn working() -> Tool {
            Tool::new("cat 'FRAME'")
        }

        /// The source paths of every call so far.
        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        fn options(&self) -> ScanOptions {
            ScanOptions {
                poster_tool: Some(self.path.clone()),
                ..ScanOptions::default()
            }
        }
    }

    /// `n` different videos (the iPhone clip with different creation times).
    fn videos(n: usize) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                ps_fixtures::media::movie(&ps_fixtures::media::MovieSpec {
                    creation_unix: Some(1_562_265_000 + i as i64 * 60),
                    ..ps_fixtures::media::MovieSpec::iphone()
                })
            })
            .collect()
    }

    /// 4 videos and 10 photos.
    fn card() -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<(String, Vec<u8>)> = (0..10u64)
            .map(|i| {
                (
                    format!("DCIM/IMG_{i:04}.JPG"),
                    ps_fixtures::media::jpeg(320, 240, 900 + i, 80),
                )
            })
            .collect();
        for (i, m) in videos(4).into_iter().enumerate() {
            v.push((format!("DCIM/IMG_{:04}.MOV", 100 + i), m));
        }
        v
    }

    fn video_rows(env: &Env, id: SourceId) -> Vec<(String, ps_catalog::FileRecord)> {
        env.files(id)
            .into_iter()
            .filter(|(p, _)| p.ends_with(".MOV"))
            .collect()
    }

    fn kind(env: &Env, r: &ps_catalog::FileRecord) -> ThumbKind {
        env.s()
            .thumbs()
            .get(&r.identity.unwrap())
            .unwrap()
            .unwrap()
            .kind
    }

    fn reason(env: &Env, r: &ps_catalog::FileRecord) -> Option<ps_model::PlaceholderReason> {
        env.s()
            .thumbs()
            .get(&r.identity.unwrap())
            .unwrap()
            .unwrap()
            .placeholder
            .and_then(|p| p.reason)
    }

    fn mov_fillable(env: &Env) -> u64 {
        env.s()
            .preview_status(&[])
            .expect("status")
            .iter()
            .filter(|r| r.format == "mov")
            .map(|r| r.fillable)
            .sum()
    }

    /// A card scanned without a poster-frame tool: its videos wait for one.
    fn scanned_without_tool(files: &[(String, Vec<u8>)]) -> (Source, Env, SourceId) {
        let src = source("card", files);
        let env = env_with(ScanOptions::default());
        let id = env.add(&src.root);
        completed(&env.scan(&[id]));
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(
                r.preview.map(|p| p.reason),
                Some(PreviewReason::NoPosterTool),
                "{rel}"
            );
        }
        (src, env, id)
    }

    /// Two of the videos as 0.2.0 left them, for migration 25's task (ADR-0111): rows without a reason, placeholder
    /// records without one.
    fn as_older_catalog(env: &mut Env, id: SourceId, names: &[&str]) {
        let rows = env.files(id);
        for n in names {
            let r = &rows[*n];
            let id = r.identity.unwrap();
            let mut t = env.s().thumbs().get(&id).unwrap().unwrap();
            if let Some(p) = t.placeholder.as_mut() {
                p.reason = None;
                p.generation = 0;
            }
            env.s().thumbs().put_many([(&id, &t)]).unwrap();
        }
        let dir = env.dir.clone();
        env.close();
        let c = rusqlite::Connection::open(dir.join(ps_catalog::CATALOG_FILE)).unwrap();
        for n in names {
            c.execute(
                "UPDATE file SET preview_gen = NULL, preview_reason = NULL WHERE rel_path = CAST(?1 AS BLOB)",
                [n],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO catalog_task (name) VALUES ('video_placeholder_reasons')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn scenario_incremental_rescan_fill_in_gives_old_placeholder_videos_their_poster_frames() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        let before = video_rows(&env, id);
        assert_eq!(before.len(), 4);
        as_older_catalog(&mut env, id, &["DCIM/IMG_0100.MOV", "DCIM/IMG_0101.MOV"]);
        let tool = Tool::working();
        env.reopen(tool.options());
        // Marked at open (ADR-0111), and every waiting video can be filled in now.
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(
                r.preview,
                Some(ps_model::PreviewStamp {
                    generation: 0,
                    reason: PreviewReason::NoPosterTool
                }),
                "{rel}"
            );
        }
        assert_eq!(mov_fillable(&env), 4);
        let s = fill(&env, FillScope::all());
        completed(&s);
        let f = s.fill.expect("a fill summary");
        assert_eq!(
            (f.filled, f.failed, f.skipped_offline, f.still_waiting),
            (4, 0, 0, 0)
        );
        assert_eq!((s.diff.posters, s.diff.posters_made), (4, 4));
        let mut calls = tool.calls();
        calls.sort();
        let root = env.root(id);
        let want: Vec<String> = before
            .iter()
            .map(|(rel, _)| under(&root, rel).display().to_string())
            .collect();
        assert_eq!(calls, want, "the tool runs once per video");
        assert_eq!(env.instr.open_count(), 0, "no file opened by PhotoSquirrel");
        assert_eq!(env.instr.total_bytes(), 0);
        assert_eq!(env.instr.read_dirs(), 0, "no directory listed");
        let after = video_rows(&env, id);
        for ((rel, a), (_, b)) in after.iter().zip(&before) {
            assert_eq!(a.identity, b.identity, "{rel}: identity unchanged");
            assert_eq!(a.stat, b.stat, "{rel}");
            assert_eq!(a.capture_ms, b.capture_ms, "{rel}");
            assert_eq!(a.preview, None, "{rel}: no placeholder any more");
            assert_eq!(kind(&env, a), ThumbKind::PosterFrame, "{rel}");
        }
        assert_eq!(mov_fillable(&env), 0);
    }

    #[test]
    fn scenario_incremental_rescan_rescan_fills_them_in_once() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        let tool = Tool::working();
        env.reopen(tool.options());
        let s = env.scan(&[id]);
        completed(&s);
        assert_eq!(tool.calls().len(), 4, "one call per waiting video");
        assert_eq!((s.diff.posters, s.diff.posters_made), (4, 4));
        assert_eq!(s.diff.changed + s.diff.new, 0);
        assert_eq!(env.instr.open_count(), 0, "the videos are not read");
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(r.preview, None, "{rel}");
            assert_eq!(kind(&env, &r), ThumbKind::PosterFrame, "{rel}");
        }
        let s = env.scan(&[id]);
        completed(&s);
        assert_eq!(tool.calls().len(), 4, "the second rescan runs no tool");
        assert_eq!(s.diff.posters, 0);
    }

    #[test]
    fn scenario_incremental_rescan_nothing_waits_without_a_tool() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        // The tool is resolved again when the counts are read and when each run starts (ADR-0114); here nothing
        // is installed yet.
        let tool = Tool::working();
        let installed = Arc::new(AtomicBool::new(false));
        let resolver: ps_scan::PosterToolFn = {
            let (installed, path) = (installed.clone(), tool.path.clone());
            Arc::new(move || installed.load(Ordering::SeqCst).then(|| path.clone()))
        };
        env.reopen(ScanOptions {
            poster_tool_resolver: Some(resolver),
            ..ScanOptions::default()
        });
        assert_eq!(mov_fillable(&env), 0, "Fill in previews offers no video");
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 0);
        let s = env.scan(&[id]);
        completed(&s);
        assert_eq!(s.diff.posters, 0, "a rescan runs no tool");
        assert!(tool.calls().is_empty());
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(
                r.preview.map(|p| p.reason),
                Some(PreviewReason::NoPosterTool),
                "{rel}"
            );
            assert_eq!(
                reason(&env, &r),
                Some(ps_model::PlaceholderReason::NoPosterTool)
            );
        }
        // ffmpeg is installed while the app runs: the counts and the next run see it, no restart.
        installed.store(true, Ordering::SeqCst);
        assert_eq!(mov_fillable(&env), 4);
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 4);
        assert_eq!(tool.calls().len(), 4);
    }

    /// A tool that fails on `IMG_0102.MOV` and gives a frame for the others.
    fn failing_on_one() -> Tool {
        Tool::new(
            "case \"$src\" in *IMG_0102.MOV) echo \"$src: Invalid data found when processing input\" >&2; exit 1;; \
             esac\ncat 'FRAME'",
        )
    }

    #[test]
    fn scenario_incremental_rescan_failed_extraction_is_not_retried_forever() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        let tool = failing_on_one();
        env.reopen(tool.options());
        let s = fill(&env, FillScope::all());
        completed(&s);
        let f = s.fill.clone().expect("fill");
        assert_eq!((f.filled, f.failed, f.still_waiting), (3, 1, 0));
        assert_eq!((s.diff.posters_made, s.diff.posters_failed), (3, 1));
        // The failing video gets a second call at 0 s (ADR-0120), the others one each.
        assert_eq!(tool.calls().len(), 5);
        let rows = env.files(id);
        let bad = &rows["DCIM/IMG_0102.MOV"];
        assert_eq!(
            bad.preview,
            Some(ps_model::PreviewStamp {
                generation: 0,
                reason: PreviewReason::PosterFailed
            })
        );
        let Some(ps_model::PlaceholderReason::PosterFailed { message }) = reason(&env, bad) else {
            panic!("not a failed poster: {:?}", reason(&env, bad));
        };
        assert!(message.contains("exit status"), "{message}");
        assert!(message.contains("Invalid data found"), "{message}");
        assert_eq!(PreviewReason::PosterFailed.label(), "poster frame failed");
        assert!(
            s.errors > 0,
            "the failure is reported in the run's errors: {s:?}"
        );
        // Neither Fill in previews nor a rescan runs the tool on it again.
        assert_eq!(mov_fillable(&env), 0);
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 0);
        let s = env.scan(&[id]);
        completed(&s);
        assert_eq!(s.diff.posters, 0);
        assert_eq!(tool.calls().len(), 5, "not run on it again");
        assert_eq!(
            env.files(id)["DCIM/IMG_0102.MOV"].preview.map(|p| p.reason),
            Some(PreviewReason::PosterFailed)
        );
    }

    #[test]
    fn scenario_incremental_rescan_explicit_retry_of_failed_posters() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        let failing = failing_on_one();
        env.reopen(failing.options());
        completed(&fill(&env, FillScope::all()));
        assert_eq!(failing.calls().len(), 5);
        // The user asks: "poster frame failed", Try again. The tool runs once on each such video and records the new
        // error when it fails again.
        let retry = FillScope {
            format: None,
            reason: Some(PreviewReason::PosterFailed),
        };
        let s = fill(&env, retry.clone());
        completed(&s);
        assert_eq!(s.fill.clone().expect("fill").failed, 1);
        let calls = failing.calls();
        assert_eq!(calls.len(), 7, "{calls:?}");
        assert!(
            calls[5..].iter().all(|c| c.ends_with("IMG_0102.MOV")),
            "{calls:?}"
        );
        // A tool that reads it now (a newer ffmpeg): the explicit retry gives it its frame.
        let fixed = Tool::working();
        env.reopen(fixed.options());
        let s = fill(&env, retry);
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 1);
        assert_eq!(fixed.calls().len(), 1);
        let bad = &env.files(id)["DCIM/IMG_0102.MOV"];
        assert_eq!(bad.preview, None);
        assert_eq!(kind(&env, bad), ThumbKind::PosterFrame);
    }

    #[test]
    fn scenario_incremental_rescan_offline_videos_are_skipped() {
        let _g = shared();
        let v = videos(4);
        let a = source(
            "laptop",
            &[
                ("IMG_0.MOV".to_owned(), v[0].clone()),
                ("IMG_1.MOV".to_owned(), v[1].clone()),
            ],
        );
        let b = source(
            "usb",
            &[
                ("x/IMG_2.MOV".to_owned(), v[2].clone()),
                ("x/IMG_3.MOV".to_owned(), v[3].clone()),
            ],
        );
        let mut env = env_with(ScanOptions::default());
        let ia = env.add(&a.root);
        let ib = env.add(&b.root);
        completed(&env.scan(&[]));
        ps_fixtures::scratch::rename(&b.root, b.tmp.path().join("unplugged")).expect("unplug");
        let tool = Tool::working();
        env.reopen(tool.options());
        let s = fill(&env, FillScope::all());
        completed(&s);
        let f = s.fill.clone().expect("fill");
        assert_eq!((f.filled, f.skipped_offline), (2, 2));
        assert!(s.incomplete_sources.contains(&ib), "{s:?}");
        assert_eq!(tool.calls().len(), 2);
        for (rel, r) in video_rows(&env, ia) {
            assert_eq!(r.preview, None, "{rel}");
        }
        for (rel, r) in video_rows(&env, ib) {
            assert_eq!(
                r.preview.map(|p| p.reason),
                Some(PreviewReason::NoPosterTool),
                "{rel} still waits"
            );
        }
        let status = env.s().preview_status(&[ib]).expect("status");
        assert_eq!(status.iter().map(|r| r.fillable).sum::<u64>(), 2);
    }

    #[test]
    fn scenario_incremental_rescan_cancel_stops_a_running_extraction() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        // A tool that hangs: it records its process id, then sleeps far beyond the 10 s timeout.
        let hanging = Tool::new("echo $$ >> 'FRAME.pids'\nexec sleep 60");
        let pids = PathBuf::from(format!("{}.pids", hanging.frame.display()));
        env.reopen(hanging.options());
        env.s().start_fill(FillScope::all(), &[]).expect("start");
        wait_until(Duration::from_secs(30), "the tool running", || {
            std::fs::read_to_string(&pids).is_ok_and(|p| !p.trim().is_empty())
        });
        let t0 = std::time::Instant::now();
        env.s().cancel().expect("cancel");
        let s = env.s().wait().expect("summary");
        assert_eq!(s.status, ps_model::RunStatus::Cancelled);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "cancel did not wait for the tool's timeout: {:?}",
            t0.elapsed()
        );
        // Every tool process started is gone.
        for pid in std::fs::read_to_string(&pids).unwrap().lines() {
            let alive = Path::new(&format!("/proc/{pid}")).exists()
                || std::process::Command::new("kill")
                    .args(["-0", pid])
                    .status()
                    .is_ok_and(|s| s.success());
            assert!(!alive, "tool process {pid} still runs");
        }
        // That video and the rest still wait; the next run fills them in.
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(
                r.preview.map(|p| p.reason),
                Some(PreviewReason::NoPosterTool),
                "{rel} still waits"
            );
        }
        let tool = Tool::working();
        env.reopen(tool.options());
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 4);
        assert_eq!(tool.calls().len(), 4);
    }

    /// The run's limit of two tool calls at once (one on a rotational or network source) holds while new videos are
    /// analysed on the CPU workers too (ADR-0120). The tool counts the calls running when it starts.
    #[test]
    fn scenario_incremental_rescan_tool_calls_are_limited_during_analysis_too() {
        let _g = shared();
        let files: Vec<(String, Vec<u8>)> = videos(6)
            .into_iter()
            .enumerate()
            .map(|(i, m)| (format!("DCIM/IMG_{i:04}.MOV"), m))
            .collect();
        for (device, limit) in [
            (ps_model::DeviceClass::SolidState, 2),
            (ps_model::DeviceClass::Network, 1),
        ] {
            let running = tempfile::tempdir().expect("running dir");
            let counts = running.path().join("counts.log");
            let run = running.path().join("now");
            std::fs::create_dir(&run).expect("mkdir");
            let tool = Tool::new(&format!(
                "touch '{run}/'$$\nls '{run}' | wc -l >> '{counts}'\nsleep 0.3\nrm -f '{run}/'$$\ncat 'FRAME'",
                run = run.display(),
                counts = counts.display()
            ));
            let src = source("nas", &files);
            let env = env_with(ScanOptions {
                cpu_threads: 6,
                device_class: Some(Arc::new(move |_: &Path| device)),
                ..tool.options()
            });
            let id = env.add(&src.root);
            let s = env.scan(&[id]);
            completed(&s);
            assert_eq!(tool.calls().len(), 6, "{device:?}: one call per new video");
            let seen: Vec<usize> = std::fs::read_to_string(&counts)
                .unwrap()
                .lines()
                .map(|l| l.trim().parse().unwrap())
                .collect();
            let max = seen.iter().copied().max().unwrap_or(0);
            assert!(
                (1..=limit).contains(&max),
                "{device:?}: at most {limit} calls at once, saw {seen:?}"
            );
            for (rel, r) in video_rows(&env, id) {
                assert_eq!(kind(&env, &r), ThumbKind::PosterFrame, "{rel}");
            }
        }
    }

    /// A failure that says nothing about the file leaves the video waiting (ADR-0120): a timeout (tried once per
    /// run, then again by the next run) and a source that goes offline during the call.
    #[test]
    fn scenario_incremental_rescan_temporary_failure_leaves_the_video_waiting() {
        let _g = shared();
        let (src, mut env, id) = scanned_without_tool(&card());
        let hanging = Tool::new("exec sleep 30");
        env.reopen(ScanOptions {
            poster_timeout: Duration::from_millis(300),
            ..hanging.options()
        });
        let s = fill(&env, FillScope::all());
        completed(&s);
        let f = s.fill.clone().expect("fill");
        assert_eq!((f.filled, f.failed), (0, 0), "{f:?}");
        assert_eq!(
            hanging.calls().len(),
            4,
            "one call per video, no retry after a timeout"
        );
        assert!(s.errors > 0, "the timeouts are reported: {s:?}");
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(
                r.preview.map(|p| p.reason),
                Some(PreviewReason::NoPosterTool),
                "{rel} still waits"
            );
            assert_eq!(
                reason(&env, &r),
                Some(ps_model::PlaceholderReason::NoPosterTool)
            );
        }
        assert_eq!(mov_fillable(&env), 4);
        // The next run tries again, once per video.
        completed(&fill(&env, FillScope::all()));
        assert_eq!(hanging.calls().len(), 8);

        // The source is unplugged while the tool reads a video: the call fails, the video keeps waiting.
        let gone = src.tmp.path().join("unplugged");
        let unplugging = Tool::new(&format!(
            "mv '{}' '{}' 2>/dev/null\necho \"$src: No such file or directory\" >&2\nexit 1",
            src.root.display(),
            gone.display()
        ));
        env.reopen(unplugging.options());
        let s = fill(&env, FillScope::all());
        assert!(s.incomplete_sources.contains(&id), "{s:?}");
        assert_eq!(s.fill.clone().expect("fill").failed, 0);
        ps_fixtures::scratch::rename(&gone, &src.root).expect("plug back in");
        for (rel, r) in video_rows(&env, id) {
            assert_eq!(
                r.preview.map(|p| p.reason),
                Some(PreviewReason::NoPosterTool),
                "{rel} still waits"
            );
        }
        let tool = Tool::working();
        env.reopen(tool.options());
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 4);
    }

    /// A tool that cannot be started (here a file without the execute permission) is no tool (ADR-0120): the video
    /// keeps, or gets, "needs a poster-frame tool", never "poster frame failed".
    #[test]
    fn a_tool_that_cannot_be_started_is_no_tool() {
        let _g = shared();
        let (_src, mut env, id) = scanned_without_tool(&card());
        let failing = failing_on_one();
        env.reopen(failing.options());
        completed(&fill(&env, FillScope::all()));
        assert_eq!(
            env.files(id)["DCIM/IMG_0102.MOV"].preview.map(|p| p.reason),
            Some(PreviewReason::PosterFailed)
        );
        let unrunnable = Tool::working();
        ps_fixtures::scratch::set_mode(&unrunnable.path, 0o644).expect("chmod");
        env.reopen(unrunnable.options());
        let s = fill(
            &env,
            FillScope {
                format: None,
                reason: Some(PreviewReason::PosterFailed),
            },
        );
        completed(&s);
        let f = s.fill.expect("fill");
        assert_eq!((f.filled, f.failed), (0, 0), "{f:?}");
        assert!(s.errors > 0);
        let bad = &env.files(id)["DCIM/IMG_0102.MOV"];
        assert_eq!(
            bad.preview.map(|p| p.reason),
            Some(PreviewReason::NoPosterTool)
        );
        assert_eq!(
            reason(&env, bad),
            Some(ps_model::PlaceholderReason::NoPosterTool)
        );
        assert!(unrunnable.calls().is_empty());
    }

    /// The thumbnail store cannot be written when the frame comes in (another connection holds its write lock longer
    /// than the store waits): the rows are left as they were, still waiting, and the next run fills them in
    /// (ADR-0120). The test holds the lock through the same SQLite library as the store, while the tool runs.
    #[test]
    fn a_store_error_leaves_the_rows_waiting() {
        let _g = shared();
        let v = videos(1);
        let (_src, mut env, id) =
            scanned_without_tool(&[("DCIM/IMG_0100.MOV".to_owned(), v[0].clone())]);
        let thumbs = env.s().thumbs().path().to_path_buf();
        let marks = tempfile::tempdir().expect("marks");
        let (started, ready, released) = (
            marks.path().join("started"),
            marks.path().join("ready"),
            marks.path().join("released"),
        );
        // The tool signals that it runs, then waits until the lock is held before it prints the frame.
        let tool = Tool::new(&format!(
            "touch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\ncat 'FRAME'",
            started.display(),
            ready.display()
        ));
        let locker = {
            let (started, ready, released) = (started.clone(), ready.clone(), released.clone());
            std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                while !started.exists() {
                    assert!(t0.elapsed() < Duration::from_secs(30), "the tool never ran");
                    std::thread::sleep(Duration::from_millis(10));
                }
                let c = rusqlite::Connection::open(&thumbs).expect("open thumbs.db");
                c.busy_timeout(Duration::from_secs(10))
                    .expect("busy timeout");
                c.execute_batch("BEGIN EXCLUSIVE").expect("lock");
                ps_fixtures::scratch::write(&ready, "").expect("ready");
                // Longer than the store's 5 s busy timeout.
                std::thread::sleep(Duration::from_secs(7));
                c.execute_batch("ROLLBACK").expect("unlock");
                drop(c);
                ps_fixtures::scratch::write(&released, "").expect("released");
            })
        };
        env.reopen(tool.options());
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.clone().expect("fill").filled, 0, "{s:?}");
        assert!(s.errors > 0, "the store error is reported: {s:?}");
        let r = &env.files(id)["DCIM/IMG_0100.MOV"];
        assert_eq!(
            r.preview.map(|p| p.reason),
            Some(PreviewReason::NoPosterTool),
            "the row is untouched"
        );
        locker.join().expect("the locker");
        assert!(released.exists());
        assert_eq!(kind(&env, r), ThumbKind::Placeholder);
        let working = Tool::working();
        env.reopen(working.options());
        let s = fill(&env, FillScope::all());
        completed(&s);
        assert_eq!(s.fill.expect("fill").filled, 1);
        assert_eq!(
            kind(&env, &env.files(id)["DCIM/IMG_0100.MOV"]),
            ThumbKind::PosterFrame
        );
    }

    /// Copies of a video on two sources get one tool call between them in a rescan (one poster item per content
    /// per run, ADR-0120), and both rows get its frame.
    #[test]
    fn copies_on_two_sources_get_one_call_in_a_rescan() {
        let _g = shared();
        let v = videos(1);
        let a = source("laptop", &[("IMG_0.MOV".to_owned(), v[0].clone())]);
        let b = source("usb", &[("backup/IMG_0.MOV".to_owned(), v[0].clone())]);
        let mut env = env_with(ScanOptions::default());
        let ia = env.add(&a.root);
        let ib = env.add(&b.root);
        completed(&env.scan(&[]));
        let tool = Tool::working();
        env.reopen(tool.options());
        let s = env.scan(&[]);
        completed(&s);
        assert_eq!(tool.calls().len(), 1, "{:?}", tool.calls());
        assert_eq!(s.diff.posters, 1);
        for id in [ia, ib] {
            for (rel, r) in video_rows(&env, id) {
                assert_eq!(r.preview, None, "{rel}");
                assert_eq!(kind(&env, &r), ThumbKind::PosterFrame, "{rel}");
            }
        }
    }
}
