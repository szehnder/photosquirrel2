//! Scenarios of `damaged-raw-reason` (ADR-0115) in the scanner: a RAW cut short is catalogued with an "could not be
//! read" placeholder, reported once, never hashed (so it cannot join or outrank its camera JPEG), counted in the run
//! summary from the catalog, and RAW placeholders of generation 0 are re-classified once.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::*;
use ps_fixtures::CorpusWriter as _;
use ps_fixtures::dng::{Cut, cut};
use ps_model::{
    DecodeFamily, FillScope, PlaceholderReason, PreviewReason, RelPath, SourceId, Stage, ThumbKind,
};
use ps_scan::ScanOptions;

struct Source {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

fn source(files: &[(String, Vec<u8>)]) -> Source {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("card");
    let w = ps_fixtures::DirWriter::new(&root).expect("writer");
    for (rel, b) in files {
        w.write(rel, b, 1_600_000_000).expect("write");
    }
    Source { _tmp: tmp, root }
}

fn under(root: &std::path::Path, rel: &str) -> PathBuf {
    RelPath::from_bytes(rel.as_bytes().to_vec())
        .expect("rel")
        .under(root)
}

/// The scanner of a version before this change: RAW decoder generation 0.
fn raw_generation_0() -> ScanOptions {
    ScanOptions {
        generations: ps_media::decoders::GENERATIONS.with(DecodeFamily::Raw, 0),
        ..ScanOptions::default()
    }
}

fn burst(seed: u64) -> ps_fixtures::dng::BurstFrameFiles {
    ps_fixtures::dng::converted_burst_frame(seed, 0, 0, 960)
}

/// Small JPEGs that look like nothing else here.
fn jpegs(n: u64) -> Vec<(String, Vec<u8>)> {
    (0..n)
        .map(|i| {
            (
                format!("DCIM/101CANON/IMG_{:04}.JPG", 5000 + i),
                ps_fixtures::media::jpeg(320, 240, 700 + i, 80),
            )
        })
        .collect()
}

fn reason_of(env: &Env, id: SourceId, rel: &str) -> Option<(u16, PreviewReason)> {
    env.files(id)[rel].preview.map(|p| (p.generation, p.reason))
}

/// The group (its id, members and keeper set) holding `file`, if any.
fn group_of(
    env: &Env,
    file: ps_model::FileId,
) -> Option<(
    ps_model::FileId,
    Vec<ps_model::FileId>,
    Vec<ps_model::FileId>,
)> {
    env.readers()
        .group_memberships()
        .unwrap()
        .into_iter()
        .find(|(_, m, _)| m.contains(&file))
}

#[test]
fn scenario_thumbnails_raw_cut_after_an_intact_preview_is_damaged() {
    let _g = shared();
    let cut_shot = burst(21);
    let intact = burst(22);
    let src = source(&[
        (
            "DCIM/100CANON/IMG_0034.CR2".into(),
            cut(&cut_shot.cr2, Cut::AfterPreview).unwrap(),
        ),
        ("DCIM/100CANON/IMG_0034.JPG".into(), cut_shot.jpeg.clone()),
        ("DCIM/200CANON/IMG_0034.CR2".into(), intact.cr2.clone()),
        ("DCIM/200CANON/IMG_0034.JPG".into(), intact.jpeg.clone()),
    ]);
    let env = env();
    let id = env.add(&src.root);
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.errors, 1, "the cut CR2 is reported");
    assert_eq!(s.damaged, 1);
    assert_eq!(s.stages.failed, 0);
    let files = env.files(id);
    let raw = &files["DCIM/100CANON/IMG_0034.CR2"];
    assert_eq!(raw.stage, Stage::Thumbed, "catalogued");
    assert!(raw.identity.is_some(), "with its identity");
    assert!(raw.hashes.is_none(), "no perceptual hashes");
    assert_eq!(
        raw.preview.map(|p| (p.generation, p.reason)),
        Some((1, PreviewReason::Unreadable))
    );
    let t = env
        .s()
        .thumbs()
        .get(&raw.identity.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(t.kind, ThumbKind::Placeholder, "no thumbnail pixels");
    match t.placeholder.and_then(|p| p.reason) {
        Some(PlaceholderReason::Unreadable { message }) => {
            assert!(message.contains("cut short"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    // Not grouped with its camera JPEG; both stay keepers of their own.
    let jpeg = &files["DCIM/100CANON/IMG_0034.JPG"];
    let (g_raw, g_jpeg) = (group_of(&env, raw.id), group_of(&env, jpeg.id));
    if let (Some(a), Some(b)) = (&g_raw, &g_jpeg) {
        assert_ne!(a.0, b.0, "different groups");
    }
    for (f, g) in [(raw.id, &g_raw), (jpeg.id, &g_jpeg)] {
        if let Some((_, _, keepers)) = g {
            assert!(keepers.contains(&f), "a keeper");
        }
    }
    // The intact pair, for contrast, is one shot: one group holding both.
    let (ir, ij) = (
        &files["DCIM/200CANON/IMG_0034.CR2"],
        &files["DCIM/200CANON/IMG_0034.JPG"],
    );
    let g = group_of(&env, ir.id).expect("the intact pair is grouped");
    assert!(g.1.contains(&ij.id), "{g:?}");
}

/// A cut CR2 whose probe finds no dimensions is catalogued with a placeholder, never failed (D3).
#[test]
fn cut_raw_without_probe_dimensions_is_catalogued() {
    let _g = shared();
    let preview = ps_fixtures::media::jpeg(640, 426, 3, 90);
    let mut ifd0 = ps_fixtures::tiff::Ifd::new();
    ifd0.short(0x0103, 6)
        .ascii(0x010F, "Canon")
        .ascii(0x0110, "Canon EOS 5D Mark III")
        .blob(0x0111, 0x0117, preview);
    let full = ps_fixtures::tiff::write_tiff(&ifd0, ps_fixtures::tiff::Endian::Little);
    let b = cut(&full, Cut::InPreview).unwrap();
    let probe = ps_media::probe_bytes(&b, b.len() as u64, None);
    assert!(probe.format.is_some_and(|f| f.is_raw()));
    assert_eq!(probe.display, None, "no probe dimensions");
    let src = source(&[("DCIM/100CANON/IMG_0099.CR2".into(), b)]);
    let env = env();
    let id = env.add(&src.root);
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.stages.failed, s.errors, s.damaged), (0, 1, 1));
    let r = &env.files(id)["DCIM/100CANON/IMG_0099.CR2"];
    assert_eq!(r.stage, Stage::Thumbed);
    assert!(r.identity.is_some());
    assert_eq!(r.preview.map(|p| p.reason), Some(PreviewReason::Unreadable));
}

#[test]
fn scenario_scan_progress_damaged_files_counted_again_on_an_unchanged_rescan() {
    let _g = shared();
    let b = burst(31);
    let src = source(&[(
        "DCIM/100CANON/IMG_0034.CR2".into(),
        cut(&b.cr2, Cut::InPreview).unwrap(),
    )]);
    let env = env();
    let id = env.add(&src.root);
    let first = env.scan(&[id]);
    completed(&first);
    assert_eq!((first.errors, first.damaged), (1, 1));
    env.instr.reset();
    let second = env.scan(&[id]);
    completed(&second);
    assert_eq!(
        (second.errors, second.damaged),
        (0, 1),
        "counted from the catalog"
    );
    assert_eq!(env.instr.total_bytes(), 0, "nothing read");
    // The stored stats carry it; stats stored before it existed read as 0.
    let mut v = second.to_json();
    assert_eq!(v["damaged"], 1);
    v.as_object_mut().unwrap().remove("damaged");
    assert_eq!(ps_scan::RunSummary::from_json(&v).unwrap().damaged, 0);
}

/// Rows a version with RAW generation 0 (0.3.0) left: `rel` gets an "unsupported variant" placeholder of generation
/// 0, as 0.3.0 called every RAW without a readable preview.
fn as_0_3_0(env: &mut Env, id: SourceId, rel: &str) {
    let r = env.files(id)[rel].clone();
    let thumbs = env.s().thumbs();
    let mut t = thumbs.get(&r.identity.unwrap()).unwrap().unwrap();
    let p = t.placeholder.as_mut().expect("a placeholder");
    p.generation = 0;
    p.reason = Some(PlaceholderReason::UnsupportedVariant {
        detail: "RAW files without an embedded preview".into(),
    });
    thumbs.put_many([(&r.identity.unwrap(), &t)]).unwrap();
    let dir = env.dir.clone();
    env.close();
    let c = rusqlite::Connection::open(dir.join(ps_catalog::CATALOG_FILE)).unwrap();
    c.execute(
        "UPDATE file SET preview_gen = 0, preview_reason = 'unsupported_variant'
         WHERE rel_path = CAST(?1 AS BLOB)",
        [rel],
    )
    .unwrap();
}

#[test]
fn scenario_incremental_rescan_rescan_re_classifies_an_old_damaged_raw() {
    let _g = shared();
    let mut files = jpegs(20);
    for i in 0..5u64 {
        files.push((
            format!("DCIM/100CANON/IMG_{:04}.CR2", 40 + i),
            burst(50 + i).cr2,
        ));
    }
    let cut_rel = "DCIM/100CANON/IMG_0034.CR2";
    files.push((cut_rel.into(), cut(&burst(60).cr2, Cut::InPreview).unwrap()));
    let src = source(&files);
    let mut env = env_with(raw_generation_0());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    let rows = env.files(id);
    assert_eq!(rows.len(), 26);
    assert_eq!(
        rows.values().filter(|r| r.preview.is_some()).count(),
        1,
        "only the cut CR2 has a placeholder"
    );
    as_0_3_0(&mut env, id, cut_rel);
    env.reopen(ScanOptions::default());
    assert_eq!(
        reason_of(&env, id, cut_rel),
        Some((0, PreviewReason::UnsupportedVariant))
    );
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.outdated_previews, 1);
    let root = env.root(id);
    let per = env.instr.bytes_per_file();
    assert_eq!(per.len(), 1, "only the cut CR2 is opened: {per:?}");
    assert!(per.contains_key(&under(&root, cut_rel)));
    assert_eq!(
        reason_of(&env, id, cut_rel),
        Some((1, PreviewReason::Unreadable)),
        "could not be read, at generation 1"
    );
    assert_eq!((s.errors, s.damaged), (1, 1), "damaged now: reported");
    env.instr.reset();
    let again = env.scan(&[id]);
    completed(&again);
    assert_eq!(env.instr.open_count(), 0, "a second rescan opens no file");
    assert_eq!((again.errors, again.damaged), (0, 1));
}

#[test]
fn scenario_incremental_rescan_re_read_unknown_camera_is_not_an_error() {
    let _g = shared();
    let rel = "DCIM/100CANON/IMG_0007.CR2";
    let src = source(&[(rel.into(), ps_fixtures::dng::cr2_unknown_camera(5))]);
    let mut env = env_with(raw_generation_0());
    let id = env.add(&src.root);
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.errors, 0, "not supported is not an error");
    assert_eq!(
        reason_of(&env, id, rel),
        Some((0, PreviewReason::UnsupportedVariant))
    );
    env.reopen(ScanOptions::default());
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.outdated_previews, 1);
    let root = env.root(id);
    let per = env.instr.bytes_per_file();
    assert_eq!(per.len(), 1, "{per:?}");
    assert!(per.contains_key(&under(&root, rel)), "read once");
    assert_eq!(
        reason_of(&env, id, rel),
        Some((1, PreviewReason::UnsupportedVariant))
    );
    assert_eq!((s.errors, s.damaged), (0, 0));
}

/// Fill in previews re-reads an old RAW placeholder too, and its summary counts the damaged file.
#[test]
fn fill_in_re_classifies_an_old_damaged_raw_and_counts_it() {
    let _g = shared();
    let rel = "DCIM/100CANON/IMG_0034.CR2";
    let mut files = jpegs(3);
    files.push((rel.into(), cut(&burst(70).cr2, Cut::AfterPreview).unwrap()));
    let src = source(&files);
    let mut env = env_with(raw_generation_0());
    let id = env.add(&src.root);
    completed(&env.scan(&[id]));
    as_0_3_0(&mut env, id, rel);
    env.reopen(ScanOptions::default());
    env.s().start_fill(FillScope::all(), &[]).expect("fill");
    let s = env.s().wait().expect("summary");
    completed(&s);
    assert_eq!(env.instr.open_count(), 1);
    assert_eq!(
        reason_of(&env, id, rel),
        Some((1, PreviewReason::Unreadable))
    );
    assert_eq!(s.damaged, 1);
    assert_eq!(s.fill.expect("fill").failed, 1, "no preview made");
}
