//! Files analysed without a decoder remember it, and the next rescan with the decoder fills them in (ADR-0091):
//! `decoder-support` and `incremental-rescan` scenarios, with the fake HEIC decoder and the bundled AV1 decoder.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use ps_fixtures::CorpusWriter as _;
use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::{DecoderKey, SourceId, Stage, ThumbKind};
use ps_scan::ScanOptions;

const HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");
const AVIF: &[u8] = include_bytes!("../../ps-media/tests/data/photo.avif");

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

/// Five different HEIC files (the sample re-boxed with different transformations, same HEVC payload).
fn heics() -> Vec<Vec<u8>> {
    [(0, None), (1, None), (2, None), (3, None), (0, Some(0))]
        .into_iter()
        .map(|(a, m)| ps_fixtures::heif::heif_with_transforms(HEIC, a, m).expect("rebox"))
        .collect()
}

struct Source {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// A source folder holding `files` (relative path, bytes), written through the fixtures writer.
fn source(files: &[(String, Vec<u8>)]) -> Source {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("phone");
    let w = ps_fixtures::DirWriter::new(&root).expect("writer");
    for (rel, b) in files {
        w.write(rel, b, 1_600_000_000).expect("write");
    }
    Source { _tmp: tmp, root }
}

fn twenty_jpegs_and_five_heics() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = (0..20)
        .map(|i| {
            (
                format!("DCIM/IMG_{i:04}.JPG"),
                ps_fixtures::media::jpeg(640, 480, 300 + i, 85),
            )
        })
        .collect();
    for (i, h) in heics().into_iter().enumerate() {
        v.push((format!("DCIM/IMG_{:04}.HEIC", 100 + i), h));
    }
    v
}

fn heic_rows(env: &Env, id: SourceId) -> Vec<(String, ps_catalog::FileRecord)> {
    env.files(id)
        .into_iter()
        .filter(|(p, _)| p.ends_with(".HEIC"))
        .collect()
}

#[test]
fn scenario_decoder_support_heic_catalogued_without_a_decoder() {
    let _g = shared();
    let src = source(&[
        ("a/IMG_0001.HEIC".into(), HEIC.to_vec()),
        ("b/IMG_0001.HEIC".into(), HEIC.to_vec()),
    ]);
    let env = env_with(no_decoder());
    let id = env.add(&src.root);
    let s = env.scan(&[id]);
    completed(&s);
    let rows = env.files(id);
    assert_eq!(rows.len(), 2);
    for r in rows.values() {
        assert_eq!(r.stage, Stage::Thumbed);
        assert_eq!(r.decoder_missing, Some(DecoderKey::Heif));
        assert!(r.hashes.is_none());
        assert!(r.capture_ms.is_some(), "capture date read");
        assert_eq!(
            r.camera_model.as_deref().map(|_| ()),
            Some(()),
            "camera read"
        );
        let t = env
            .s()
            .thumbs()
            .get(&r.identity.expect("identity"))
            .expect("thumb")
            .expect("stored");
        assert_eq!(t.kind, ThumbKind::Placeholder);
        assert_eq!(
            t.placeholder.expect("info").note,
            "No HEIC decoder on this system"
        );
    }
    let gps = rows.values().next().map(|r| r.has_gps);
    let meta = ps_media::extract_metadata(
        HEIC,
        &ps_media::probe_bytes(HEIC, HEIC.len() as u64, None),
        None,
        &ps_media::DateContext {
            rel_path: None,
            zone: &ps_media::FixedZone(0),
        },
    );
    assert_eq!(
        gps,
        Some(meta.fields.gps.is_some()),
        "GPS read as the metadata has it"
    );
    // Exact copies: one group of both.
    let groups = env.readers().group_memberships().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].1.len(), 2);
    // The run summary names them.
    assert_eq!(s.undecoded.len(), 1, "{:?}", s.undecoded);
    assert_eq!(s.undecoded[0].count, 2);
    assert!(!s.undecoded[0].available);
}

#[test]
fn scenario_incremental_rescan_files_keep_waiting_while_the_decoder_is_missing() {
    let _g = shared();
    let src = source(&twenty_jpegs_and_five_heics());
    let env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(
        env.instr.total_bytes(),
        0,
        "zero bytes of file content read"
    );
    assert_eq!(s.diff.awaiting_decoder, 5);
    assert_eq!(s.diff.redecode, 0);
    assert_eq!(s.undecoded.len(), 1);
    assert_eq!(
        (s.undecoded[0].format.as_str(), s.undecoded[0].count),
        ("heic", 5)
    );
}

#[test]
fn scenario_incremental_rescan_rescan_decodes_only_files_that_waited_for_a_decoder() {
    let _g = shared();
    let src = source(&twenty_jpegs_and_five_heics());
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    env.reopen(with(FakeHeif::new()));
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.redecode, 5);
    assert_eq!(s.diff.redecoded, 5, "5 previews filled in");
    assert_eq!(s.diff.unchanged, 25);
    let root = env.root(id);
    let per = env.instr.bytes_per_file();
    for (rel, r) in heic_rows(&env, id) {
        let read = per.get(
            &ps_model::RelPath::from_bytes(rel.clone().into_bytes())
                .unwrap()
                .under(&root),
        );
        assert_eq!(read, Some(&r.stat.size), "{rel} read once in full");
        assert!(r.hashes.is_some(), "{rel} has perceptual hashes");
        assert_eq!(r.decoder_missing, None);
        assert_eq!(r.stage, Stage::Thumbed);
        let t = env.s().thumbs().get(&r.identity.unwrap()).unwrap().unwrap();
        assert_eq!(t.kind, ThumbKind::Image, "{rel}");
    }
    let jpeg_bytes: u64 = per
        .iter()
        .filter(|(p, _)| p.to_string_lossy().ends_with(".JPG"))
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(jpeg_bytes, 0, "no JPEG byte read");
    assert_eq!(
        env.instr.total_bytes(),
        heic_rows(&env, id)
            .iter()
            .map(|(_, r)| r.stat.size)
            .sum::<u64>()
    );
    assert!(s.undecoded.is_empty());
}

#[test]
fn scenario_incremental_rescan_heic_grouped_with_its_resized_jpeg_after_the_decoder_arrives() {
    let _g = shared();
    // The picture the HEIC holds, and a JPEG resized from it.
    let rgb = ps_fixtures::media::photo_rgb(1280, 960, 4242);
    let small = {
        let img = ps_media::RgbImage {
            width: 1280,
            height: 960,
            data: rgb.clone(),
        };
        let r = ps_media::decode::resize_to_long_edge(&img, 800);
        ps_fixtures::media::encode_jpeg(&r.data, r.width, r.height, 85)
    };
    let src = source(&[
        ("DCIM/IMG_0001.HEIC".into(), HEIC.to_vec()),
        ("Web/IMG_0001.jpg".into(), small),
    ]);
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    assert!(
        env.readers().group_memberships().unwrap().is_empty(),
        "separate without a decoder"
    );
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
    let s = env.scan(&[id]);
    completed(&s);
    let groups = env.readers().group_memberships().unwrap();
    assert_eq!(groups.len(), 1, "one duplicate group");
    let rows = env.files(id);
    let heic = rows["DCIM/IMG_0001.HEIC"].id;
    let g = env.readers().group(groups[0].0).unwrap().expect("group");
    assert_eq!(g.keeper, heic, "the HEIC is the keeper");
    let jpeg = g
        .members
        .iter()
        .find(|m| m.file != heic)
        .expect("the JPEG member");
    assert_eq!(
        jpeg.edge,
        ps_catalog::groups::MemberEdge::Derivative,
        "a look-based join: {jpeg:?}"
    );
    assert!(jpeg.phash_distance.is_some());
}

#[test]
fn scenario_incremental_rescan_corrupt_heic_keeps_its_exact_copy_analysis() {
    let _g = shared();
    let src = source(&[
        ("a/IMG_0001.HEIC".into(), HEIC.to_vec()),
        ("b/IMG_0001.HEIC".into(), HEIC.to_vec()),
    ]);
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let before = env.files(id);
    let groups_before = env.readers().group_memberships().unwrap();
    env.reopen(with(FakeHeif::failing("corrupt HEVC data")));
    let s = env.scan(&[id]);
    completed(&s);
    let after = env.files(id);
    for (rel, b) in &before {
        let a = &after[rel];
        assert_eq!(a.stage, b.stage, "{rel}");
        assert_eq!(a.identity, b.identity);
        assert_eq!(a.decoder_missing, None, "{rel} no longer waiting");
        assert!(a.hashes.is_none());
        let t = env.s().thumbs().get(&a.identity.unwrap()).unwrap().unwrap();
        assert_eq!(t.kind, ThumbKind::Placeholder, "keeps its placeholder");
    }
    assert_eq!(env.readers().group_memberships().unwrap(), groups_before);
    let snap = env.s().snapshot().expect("snapshot");
    assert!(
        snap.recent_errors
            .iter()
            .any(|e| e.message.contains("could not be decoded") && e.message.contains("corrupt")),
        "{:?}",
        snap.recent_errors
    );
    env.instr.reset();
    completed(&env.scan(&[id]));
    assert_eq!(
        env.instr.total_bytes(),
        0,
        "a further rescan reads none of its bytes"
    );
}

/// A catalog written before this change (schema 18): HEIC and AVIF rows with placeholders and a HEIC row with
/// hashes. Opening it marks the placeholder rows; the next rescan fills in the AVIF with the bundled decoder.
#[test]
fn scenario_incremental_rescan_old_catalog_marks_undecoded_heic_and_avif() {
    let _g = shared();
    let h = heics();
    let src = source(&[
        ("DCIM/IMG_0001.HEIC".into(), h[0].clone()),
        ("DCIM/IMG_0002.HEIC".into(), h[1].clone()),
        ("DCIM/IMG_0003.AVIF".into(), AVIF.to_vec()),
    ]);
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    // Make it the catalog an earlier version wrote: no marker column, the AVIF without hashes or preview (no
    // AVIF decoder then), and one HEIC that had hashes.
    let rows = env.files(id);
    let avif = rows["DCIM/IMG_0003.AVIF"].clone();
    let probe = ps_media::probe_bytes(AVIF, AVIF.len() as u64, None);
    let ph = ps_media::thumb::placeholder(
        &probe,
        None,
        "preview unavailable (HEIC decoder not built in)",
    );
    env.s()
        .thumbs()
        .put_many([(&avif.identity.unwrap(), &ph)])
        .unwrap();
    let dir = env.dir.clone();
    env.reopen(no_decoder());
    env.close();
    {
        let c = rusqlite::Connection::open(dir.join(ps_catalog::CATALOG_FILE)).unwrap();
        c.execute_batch(
            "UPDATE file SET phash = NULL, dhash = NULL, hash_version = NULL WHERE rel_path = CAST('DCIM/IMG_0003.AVIF' AS BLOB);
             UPDATE file SET phash = 7, dhash = 7, hash_version = 1 WHERE rel_path = CAST('DCIM/IMG_0002.HEIC' AS BLOB);
             DROP INDEX file_decoder_missing;
             ALTER TABLE file DROP COLUMN decoder_missing;
             PRAGMA user_version = 18;",
        )
        .unwrap();
    }
    env.reopen(no_decoder());
    let rows = env.files(id);
    assert_eq!(
        rows["DCIM/IMG_0001.HEIC"].decoder_missing,
        Some(DecoderKey::Heif)
    );
    assert_eq!(
        rows["DCIM/IMG_0002.HEIC"].decoder_missing, None,
        "it has hashes"
    );
    assert_eq!(
        rows["DCIM/IMG_0003.AVIF"].decoder_missing,
        Some(DecoderKey::Avif)
    );
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.redecoded, 1, "the AVIF, by the bundled decoder");
    assert_eq!(s.diff.awaiting_decoder, 1, "the HEIC waits for its decoder");
    let a = &env.files(id)["DCIM/IMG_0003.AVIF"];
    assert!(a.hashes.is_some());
    assert_eq!(a.decoder_missing, None);
    let t = env.s().thumbs().get(&a.identity.unwrap()).unwrap().unwrap();
    assert_eq!(t.kind, ThumbKind::Image, "the AVIF preview is filled in");
}
