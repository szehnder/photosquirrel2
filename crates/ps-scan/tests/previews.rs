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
