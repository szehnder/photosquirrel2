//! The service's decoders (ADR-0089, ADR-0091): detection off the caller's thread, large previews with the
//! decoders detected now, and a re-decode that cannot read its file.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ps_media::Decoders;
use ps_media::decoders::fake::FakeHeif;
use ps_model::library::Preview;
use ps_model::{DecoderKey, ItemId, Stage};
use ps_service::decoders::DecoderHub;
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

const HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

fn fake() -> Decoders {
    Decoders::with_heif(Arc::new(FakeHeif::new()))
}

fn source(dir: &Path, files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let root = dir.join("phone");
    for (rel, b) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b).unwrap();
    }
    root
}

#[test]
fn scenario_thumbnails_large_heic_preview_after_the_decoder_appears() {
    let tmp = tempfile::tempdir().unwrap();
    let root = source(tmp.path(), &[("DCIM/IMG_0001.HEIC", HEIC)]);
    let data = tmp.path().join("data");
    {
        let svc = ScanService::open(&data, ScanOptions::default())
            .unwrap()
            .with_decoders(Decoders::none());
        svc.add_source(&root).unwrap();
        svc.start(&[]).unwrap();
        svc.wait().unwrap();
        let rec = svc
            .engine()
            .catalog()
            .read()
            .file_by_id(ps_model::FileId(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            rec.decoder_missing,
            Some(DecoderKey::Heif),
            "catalogued with a placeholder"
        );
    }
    // A decoder is available now; no rescan.
    let svc = ScanService::open(&data, ScanOptions::default())
        .unwrap()
        .with_decoders(fake());
    let lib = ps_service::library::Library::with_service(&data, svc);
    match lib.preview(ItemId(1)).unwrap() {
        Preview::Jpeg(j) => assert!(j.starts_with(&[0xFF, 0xD8])),
        other => panic!("no large preview: {other:?}"),
    }
}

/// A scan started before detection has finished waits for it and decodes with its result.
#[test]
fn a_scan_started_before_detection_finishes_waits_for_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = source(tmp.path(), &[("DCIM/IMG_0001.HEIC", HEIC)]);
    let hub = DecoderHub::detect_in_background(|| {
        std::thread::sleep(Duration::from_millis(300));
        fake()
    });
    assert!(hub.try_get().is_none());
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default())
        .unwrap()
        .with_decoder_hub(hub);
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    let s = svc.wait().unwrap();
    assert!(s.undecoded.is_empty(), "{:?}", s.undecoded);
    let rec = svc
        .engine()
        .catalog()
        .read()
        .file_by_id(ps_model::FileId(1))
        .unwrap()
        .unwrap();
    assert!(rec.hashes.is_some(), "decoded with the detected decoder");
}

#[test]
fn scenario_incremental_rescan_unreadable_file_keeps_its_analysis_during_a_re_decode() {
    let tmp = tempfile::tempdir().unwrap();
    // A HEIC and its exact copy: the keeper of an exact-copy group.
    let root = source(
        tmp.path(),
        &[("a/IMG_0001.HEIC", HEIC), ("b/IMG_0001.HEIC", HEIC)],
    );
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");
    let settings = profile_settings("google").unwrap();
    {
        let svc = ScanService::open(&data, ScanOptions::default())
            .unwrap()
            .with_decoders(Decoders::none());
        svc.add_source(&root).unwrap();
        svc.start(&[]).unwrap();
        svc.wait().unwrap();
        let (_, r) = svc
            .exports()
            .run(
                &library,
                settings,
                &Default::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(r.copied, 1, "{r:?}");
    }
    let (fs, instr) = ps_scan::SourceFs::instrumented();
    let svc = ScanService::open(
        &data,
        ScanOptions {
            fs,
            ..ScanOptions::default()
        },
    )
    .unwrap()
    .with_decoders(fake());
    let files = |svc: &ScanService| {
        let r = svc.engine().catalog().read();
        [1, 2].map(|i| r.file_by_id(ps_model::FileId(i)).unwrap().unwrap())
    };
    let before = files(&svc);
    let groups_before = svc.engine().catalog().read().group_memberships().unwrap();
    let keeper = groups_before[0].0;
    let keeper_rec = before.iter().find(|r| r.id == keeper).unwrap();
    let real_root = svc.sources().unwrap()[0].root.clone();
    instr.deny_open(&keeper_rec.key.path.under(&real_root));
    svc.start(&[]).unwrap();
    let s = svc.wait().unwrap();
    let after = files(&svc);
    let k = after.iter().find(|r| r.id == keeper).unwrap();
    assert_eq!(k.stage, Stage::Thumbed);
    assert_eq!(k.identity, keeper_rec.identity);
    assert_eq!(k.decoder_missing, Some(DecoderKey::Heif), "still waiting");
    assert!(k.hashes.is_none(), "keeps its analysis");
    assert_eq!(
        svc.engine().catalog().read().group_memberships().unwrap(),
        groups_before,
        "keeps its group"
    );
    let snap = svc.snapshot().unwrap();
    assert!(
        snap.recent_errors.iter().any(|e| e
            .message
            .contains("could not be read to fill in its preview")),
        "{:?}",
        snap.recent_errors
    );
    assert!(s.errors >= 1);
    let (plan, _) = svc.exports().plan(&library, settings).unwrap();
    assert_eq!(plan.totals.files, 0, "the export copies nothing new");
}

/// Scan and export a source holding one truncated HEIC with `decoders`; the number of files copied.
fn export_truncated_heic(decoders: Decoders) -> u64 {
    let tmp = tempfile::tempdir().unwrap();
    let truncated = &HEIC[..HEIC.len() * 3 / 4];
    let root = source(tmp.path(), &[("DCIM/IMG_0001.HEIC", truncated)]);
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default())
        .unwrap()
        .with_decoders(decoders);
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let rec = svc
        .engine()
        .catalog()
        .read()
        .file_by_id(ps_model::FileId(1))
        .unwrap()
        .unwrap();
    assert_eq!(rec.stage, Stage::Thumbed, "catalogued, not unreadable");
    let library = tmp.path().join("Library");
    let (_, r) = svc
        .exports()
        .run(
            &library,
            profile_settings("google").unwrap(),
            &Default::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    // The copy is the truncated file, byte for byte.
    let copied: Vec<_> = walk(&library)
        .into_iter()
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("heic"))
        })
        .collect();
    assert_eq!(copied.len() as u64, r.copied, "{copied:?}");
    for p in &copied {
        assert_eq!(std::fs::read(p).unwrap(), truncated);
    }
    r.copied
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// A truncated HEIC is still exported once a decoder is installed (ADR-0095): with a decoder that rejects it.
#[test]
fn scenario_export_truncated_heic_is_exported_with_a_decoder() {
    let failing = Decoders::with_heif(Arc::new(FakeHeif::failing("truncated HEVC data")));
    assert_eq!(export_truncated_heic(failing), 1);
}

/// The same with this machine's real HEIC decoder ([Linux-real], [macOS]; a visible skip elsewhere, a failure
/// with `PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1`).
#[test]
fn scenario_export_truncated_heic_is_exported_with_the_system_decoder() {
    let d = ps_sysdecode::detect(
        &ps_sysdecode::OsInfo::current(),
        &ps_sysdecode::Overrides::default(),
    );
    if d.heif().is_none() {
        let why = d
            .report()
            .entry(ps_model::DecoderFormat::Heic)
            .and_then(|e| e.reason.clone())
            .unwrap_or_default();
        ps_sysdecode::skip_test(&format!("no HEIC decoder on this machine: {why}"));
        return;
    }
    assert_eq!(export_truncated_heic(d), 1);
}
