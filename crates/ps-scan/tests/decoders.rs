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
             DROP INDEX file_decode_failure;
             ALTER TABLE file DROP COLUMN decode_failure;
             DROP INDEX file_exclusion;
             ALTER TABLE file DROP COLUMN exclusion;
             ALTER TABLE user_inclusion DROP COLUMN ai_kind;
             ALTER TABLE user_inclusion DROP COLUMN origin;
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

/// A new HEIC or AVIF the decoder rejects (truncated, corrupt, or declaring more pixels than the decode cap)
/// keeps its identity and metadata with a placeholder: catalogued, grouped by exact content, never
/// `unreadable` (ADR-0095). Without this, a truncated HEIC was exported in 0.2.0 (no decoder) but became
/// unreadable once a decoder was installed.
#[test]
fn scenario_decoder_support_corrupt_heic_or_avif_keeps_a_placeholder() {
    let _g = shared();
    let truncated = HEIC[..HEIC.len() * 3 / 4].to_vec();
    let huge = ps_fixtures::heif::heif_with_size(HEIC, 30_000, 30_000).expect("rewrite");
    let avif = AVIF[..AVIF.len() * 2 / 3].to_vec();
    let src = source(&[
        ("a/IMG_0001.HEIC".into(), truncated.clone()),
        ("b/IMG_0001.HEIC".into(), truncated),
        ("c/IMG_0002.HEIC".into(), huge),
        ("d/IMG_0003.avif".into(), avif),
    ]);
    // The fake decoder fails like a real one on corrupt data.
    let env = env_with(with(FakeHeif::failing("corrupt HEVC data")));
    let id = env.add(&src.root);
    let s = env.scan(&[id]);
    completed(&s);
    let rows = env.files(id);
    assert_eq!(rows.len(), 4);
    for (rel, r) in &rows {
        assert_eq!(r.stage, Stage::Thumbed, "{rel}: catalogued, not failed");
        assert!(r.identity.is_some(), "{rel}");
        assert_eq!(r.decoder_missing, None, "{rel}");
        assert!(r.hashes.is_none(), "{rel}");
        let t = env.s().thumbs().get(&r.identity.unwrap()).unwrap().unwrap();
        assert_eq!(t.kind, ThumbKind::Placeholder, "{rel}");
        assert!(
            t.placeholder
                .as_ref()
                .unwrap()
                .note
                .contains("could not be decoded"),
            "{rel}: {:?}",
            t.placeholder
        );
    }
    // The two truncated copies are exact duplicates of each other.
    let groups = env.readers().group_memberships().unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].1.len(), 2);
    let snap = env.s().snapshot().expect("snapshot");
    for want in ["corrupt HEVC data", "HEIF image", "AVIF"] {
        assert!(
            snap.recent_errors
                .iter()
                .any(|e| e.message.contains("could not be decoded") && e.message.contains(want)),
            "{want}: {:?}",
            snap.recent_errors
        );
    }
}

/// A HEIC decoder that counts how many decodes run at once.
struct Counting {
    now: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    calls: std::sync::atomic::AtomicUsize,
}

impl ps_media::StillDecoder for Counting {
    fn name(&self) -> &str {
        "counting"
    }

    fn decode(
        &self,
        _: &[u8],
        _: u32,
    ) -> Result<ps_media::RgbImage, ps_media::decode::DecodeError> {
        use std::sync::atomic::Ordering;
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(n, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(150));
        self.now.fetch_sub(1, Ordering::SeqCst);
        Ok(ps_media::RgbImage {
            width: 64,
            height: 64,
            data: vec![128; 64 * 64 * 3],
        })
    }

    fn applies_transforms(&self) -> bool {
        true
    }
}

/// HEIC and AVIF decodes count their working memory, estimated from the declared size, in the scan's decode
/// budget (ADR-0095): huge images (cheap fixtures that declare 12000x12000) decode one at a time, while small
/// ones run in parallel.
#[test]
fn scenario_decoder_support_huge_heic_decodes_are_throttled_by_the_memory_budget() {
    let _g = shared();
    use std::sync::atomic::Ordering;
    let run = |w: u32, h: u32| {
        let files: Vec<(String, Vec<u8>)> = heics()
            .into_iter()
            .take(4)
            .enumerate()
            .map(|(i, b)| {
                (
                    format!("DCIM/IMG_{i}.HEIC"),
                    ps_fixtures::heif::heif_with_size(&b, w, h).expect("size"),
                )
            })
            .collect();
        let src = source(&files);
        let dec = Arc::new(Counting {
            now: 0.into(),
            peak: 0.into(),
            calls: 0.into(),
        });
        let env = env_with(ScanOptions {
            decoders: Arc::new(Decoders::with_heif(dec.clone())),
            cpu_threads: 4,
            ..ScanOptions::default()
        });
        let id = env.add(&src.root);
        completed(&env.scan(&[id]));
        assert_eq!(dec.calls.load(Ordering::SeqCst), 4);
        dec.peak.load(Ordering::SeqCst)
    };
    // 12000 x 12000 x 6 bytes = 864 MB each: two do not fit in the 1 GiB budget.
    assert_eq!(run(12_000, 12_000), 1, "huge decodes one at a time");
    // 640 x 480: all four fit.
    assert!(run(640, 480) > 1, "small decodes in parallel");
    let p = ps_media::probe_bytes(
        &ps_fixtures::heif::heif_with_size(HEIC, 12_000, 12_000).unwrap(),
        HEIC.len() as u64,
        None,
    );
    assert_eq!(
        ps_media::decode::decode_working_bytes(&p),
        Some(12_000 * 12_000 * 6)
    );
}

/// A waiting HEIC whose content changed while its stat fingerprint did not (same size, time and inode) is not
/// re-decoded under its old identity: the re-decode's full read finds another identity and the file is
/// analysed again like a changed file (ADR-0091).
#[test]
fn scenario_incremental_rescan_content_changed_under_an_unchanged_fingerprint_is_analysed_again() {
    let _g = shared();
    let src = source(&[("DCIM/IMG_0001.HEIC".into(), HEIC.to_vec())]);
    let mut env = env_with(no_decoder());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let before = env.files(id)["DCIM/IMG_0001.HEIC"].clone();
    assert_eq!(before.decoder_missing, Some(DecoderKey::Heif));
    // Same length, one byte of the coded payload different; the fingerprint is restored.
    let mut other = HEIC.to_vec();
    let last = other.len() - 1;
    other[last] ^= 0x5A;
    let path = src.root.join("DCIM/IMG_0001.HEIC");
    let st = std::fs::metadata(&path).unwrap();
    ps_fixtures::writer::rewrite_keeping_stat(&path, &other).unwrap();
    let st2 = std::fs::metadata(&path).unwrap();
    assert_eq!(
        ps_scan::fs::fingerprint(&st),
        ps_scan::fs::fingerprint(&st2)
    );
    let fake = FakeHeif::new();
    env.reopen(with(fake));
    let s = env.scan(&[id]);
    completed(&s);
    let after = env.files(id)["DCIM/IMG_0001.HEIC"].clone();
    assert_ne!(
        after.identity, before.identity,
        "the new content's identity"
    );
    assert_eq!(
        after.identity,
        Some(ps_hash::identity_from_buffer(&other)),
        "identity of the bytes now on disk"
    );
    assert_eq!(after.decoder_missing, None);
    assert!(after.hashes.is_some(), "decoded as the changed file");
    assert_eq!(after.stage, Stage::Thumbed);
    // Its thumbnail is stored under the new identity.
    let t = env
        .s()
        .thumbs()
        .get(&after.identity.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(t.kind, ThumbKind::Image);
}

/// Wait (bounded) until no AVIF decode thread is alive.
fn settle_avif() {
    let t = std::time::Instant::now();
    while ps_media::av1::live_decodes() != (0, 0)
        && t.elapsed() < std::time::Duration::from_secs(10)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(ps_media::av1::live_decodes(), (0, 0));
}

/// A decode that did not complete is transient (ADR-0098): an AVIF whose decode hangs (test hook) is abandoned
/// by the watchdog and keeps a placeholder with a transient failure record; the next rescan decodes it again
/// (attempt 2, still hanging), and once the decoder completes, a further rescan fills in its preview and hashes and
/// clears the record.
#[test]
fn scenario_incremental_rescan_timed_out_avif_is_retried_on_rescan() {
    let _g = exclusive();
    let src = source(&[("DCIM/IMG_0001.avif".into(), AVIF.to_vec())]);
    let env = env();
    let id = env.add(&src.root);
    ps_media::av1::hooks::set_timeout(Some(std::time::Duration::from_millis(100)));
    ps_media::av1::hooks::hang();
    let row = |env: &Env| env.files(id)["DCIM/IMG_0001.avif"].clone();
    completed(&env.scan(&[id]));
    let r = row(&env);
    assert_eq!(r.stage, Stage::Thumbed, "catalogued with a placeholder");
    assert!(r.hashes.is_none());
    let f = r.decode_failure.clone().expect("a failure record");
    assert_eq!(
        (f.key, f.transient, f.attempts, f.backend.as_str()),
        (DecoderKey::Avif, true, 1, ps_media::av1::BACKEND)
    );
    // Still hanging: retried, a second attempt.
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.diff.redecode, s.diff.decode_retries), (1, 1));
    assert_eq!(env.instr.total_bytes(), r.stat.size, "read once in full");
    assert_eq!(row(&env).decode_failure.unwrap().attempts, 2);
    ps_media::av1::hooks::release();
    ps_media::av1::hooks::set_timeout(None);
    settle_avif();
    // The decoder completes now: filled in, the record cleared.
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.diff.decode_retries, s.diff.redecoded), (1, 1));
    let r = row(&env);
    assert!(r.hashes.is_some(), "perceptual hashes");
    assert_eq!(r.decode_failure, None);
    let t = env.s().thumbs().get(&r.identity.unwrap()).unwrap().unwrap();
    assert_eq!(t.kind, ThumbKind::Image);
    // Nothing more to do.
    env.instr.reset();
    let s = env.scan(&[id]);
    assert_eq!(s.diff.decode_retries, 0);
    assert_eq!(env.instr.total_bytes(), 0);
}

/// A permanent decode failure (the data) is not re-read on an unchanged rescan with the same decoder, but a new
/// decoder backend (a libheif upgrade; here the fake decoder under a new name) decodes it again once (ADR-0098).
#[test]
fn scenario_incremental_rescan_corrupt_heic_is_not_re_read_on_an_unchanged_rescan() {
    let _g = shared();
    let src = source(&[("DCIM/IMG_0001.HEIC".into(), HEIC.to_vec())]);
    let mut env = env_with(with(FakeHeif::failing("corrupt HEVC data")));
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let r = env.files(id)["DCIM/IMG_0001.HEIC"].clone();
    let f = r.decode_failure.clone().expect("a failure record");
    assert_eq!(
        (f.key, f.transient, f.attempts, f.backend.as_str()),
        (DecoderKey::Heif, false, 1, "fake HEIC decoder")
    );
    for _ in 0..2 {
        env.instr.reset();
        let s = env.scan(&[id]);
        completed(&s);
        assert_eq!(s.diff.decode_retries, 0);
        assert_eq!(env.instr.total_bytes(), 0, "not read again");
    }
    // An upgraded decoder that still fails: tried once, then not again.
    env.reopen(with(
        FakeHeif::failing("corrupt HEVC data").named("fake HEIC decoder 2"),
    ));
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.decode_retries, 1);
    assert_eq!(env.instr.total_bytes(), r.stat.size);
    let f = env.files(id)["DCIM/IMG_0001.HEIC"]
        .decode_failure
        .clone()
        .unwrap();
    assert_eq!((f.backend.as_str(), f.attempts), ("fake HEIC decoder 2", 1));
    env.instr.reset();
    completed(&env.scan(&[id]));
    assert_eq!(env.instr.total_bytes(), 0);
    // An upgrade that decodes it: filled in.
    env.reopen(with(FakeHeif::new().named("fake HEIC decoder 3")));
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.redecoded, 1);
    let r = &env.files(id)["DCIM/IMG_0001.HEIC"];
    assert!(r.hashes.is_some());
    assert_eq!(r.decode_failure, None);
}

/// A decode refused before it starts (AV1 decoding paused while abandoned decodes run) says nothing about the
/// file and uses up none of its attempts (ADR-0100): after five refused runs the file is still retried, and the
/// first run after the abandoned decodes end decodes it.
#[test]
fn scenario_incremental_rescan_refused_avif_decodes_use_up_no_attempt() {
    let _g = exclusive();
    let src = source(&[("DCIM/IMG_0001.avif".into(), AVIF.to_vec())]);
    let env = env();
    let id = env.add(&src.root);
    // Two decodes abandoned by the watchdog and still running: AV1 decoding is paused.
    ps_media::av1::hooks::set_timeout(Some(std::time::Duration::from_millis(50)));
    ps_media::av1::hooks::hang();
    for _ in 0..ps_media::av1::MAX_ABANDONED {
        assert!(matches!(
            ps_media::av1::decode(AVIF),
            Err(ps_media::DecodeError::Interrupted(_))
        ));
    }
    assert_eq!(
        ps_media::av1::live_decodes(),
        (0, ps_media::av1::MAX_ABANDONED)
    );
    let row = |env: &Env| env.files(id)["DCIM/IMG_0001.avif"].clone();
    for run in 0..5 {
        let s = env.scan(&[id]);
        completed(&s);
        if run > 0 {
            assert_eq!(s.diff.decode_retries, 1, "run {run}: read again");
        }
        let r = row(&env);
        assert!(r.hashes.is_none());
        let f = r.decode_failure.expect("a failure record");
        assert_eq!(
            (f.transient, f.attempts, f.backend.as_str()),
            (true, 0, ps_media::av1::BACKEND),
            "run {run}: no attempt used"
        );
    }
    ps_media::av1::hooks::release();
    ps_media::av1::hooks::set_timeout(None);
    settle_avif();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.diff.decode_retries, s.diff.redecoded), (1, 1));
    let r = row(&env);
    assert!(r.hashes.is_some(), "decoded");
    assert_eq!(r.decode_failure, None);
}

/// One decode budget per scanner (ADR-0100): the memory reserved by an AVIF decode that a cancelled run abandoned
/// stays counted, as orphaned, through the next run, until the decode thread really ends.
#[test]
fn abandoned_decode_memory_of_a_cancelled_run_is_counted_in_the_next_run() {
    let _g = exclusive();
    let avif = source(&[("DCIM/IMG_0001.avif".into(), AVIF.to_vec())]);
    let jpegs = source(&[(
        "DCIM/IMG_0002.JPG".into(),
        ps_fixtures::media::jpeg(640, 480, 5, 85),
    )]);
    let env = env();
    let a = env.add(&avif.root);
    let b = env.add(&jpegs.root);
    let budget = env.s().decode_budget().clone();
    ps_media::av1::hooks::hang();
    env.s().start_scan(&[a]).expect("start");
    wait_until(
        std::time::Duration::from_secs(60),
        "the AVIF decode",
        || ps_media::av1::live_decodes().0 == 1,
    );
    let held = budget.in_flight().0;
    assert!(held > 0, "the decode reserved its memory");
    env.s().cancel().expect("cancel");
    env.s().wait().expect("summary");
    assert_eq!(ps_media::av1::live_decodes(), (0, 1), "abandoned");
    assert_eq!(budget.in_flight(), (held, held), "counted as orphaned");
    // The next run shares the budget: the abandoned decode's bytes are still held while it runs.
    completed(&env.scan(&[b]));
    assert!(std::sync::Arc::ptr_eq(env.s().decode_budget(), &budget));
    assert_eq!(budget.in_flight(), (held, held));
    ps_media::av1::hooks::release();
    settle_avif();
    assert_eq!(budget.in_flight(), (0, 0), "released when the thread ended");
}
