//! The one-time RAW structure check of `damaged-raw-reason` (ADR-0117): a catalog written by 0.3.0, where a CR2 cut
//! after its intact embedded preview got preview pixels and hashes and joined its camera JPEG, is corrected by the
//! next rescan, which reads only the RAW files' directories, once. A test binary of its own: the tests switch the
//! process-wide structure-check hook, one at a time.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::sync::Mutex;

use common::*;
use ps_fixtures::CorpusWriter as _;
use ps_fixtures::dng::{Cut, cut};
use ps_model::{FileId, PlaceholderReason, PreviewReason, RelPath, SourceId, ThumbKind};
use ps_scan::ScanOptions;

/// One test at a time: the hook is process-wide.
static HOOK: Mutex<()> = Mutex::new(());

fn hook() -> std::sync::MutexGuard<'static, ()> {
    HOOK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Turns the structure check off while alive (the build before ADR-0115), back on when dropped.
struct Legacy;

impl Legacy {
    fn on() -> Self {
        ps_model::frame::set_structure_check_enabled(false);
        Legacy
    }
}

impl Drop for Legacy {
    fn drop(&mut self) {
        ps_model::frame::set_structure_check_enabled(true);
    }
}

const CUT_CR2: &str = "DCIM/100CANON/IMG_0034.CR2";
const CUT_JPG: &str = "DCIM/100CANON/IMG_0034.JPG";
const INTACT_CR2: &str = "DCIM/100CANON/IMG_0035.CR2";
const INTACT_JPG: &str = "DCIM/100CANON/IMG_0035.JPG";

struct Card {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// A card with a CR2 cut halfway through its sensor strip (its embedded preview intact) beside its camera JPEG,
/// an intact pair, and a few unrelated JPEGs.
fn card() -> Card {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("card");
    let w = ps_fixtures::DirWriter::new(&root).expect("writer");
    let shot = ps_fixtures::dng::converted_burst_frame(21, 0, 0, 2400);
    let other = ps_fixtures::dng::converted_burst_frame(22, 1, 0, 2400);
    let cut_cr2 = cut(&shot.cr2, Cut::AfterPreview).expect("a sensor strip");
    assert!(
        cut_cr2.len() > 4 * ps_media::raw_check::STRUCTURE_HEAD,
        "a CR2 much larger than the structure check's head: {} bytes",
        cut_cr2.len()
    );
    w.write(CUT_CR2, &cut_cr2, 1_600_000_000).expect("write");
    w.write(CUT_JPG, &shot.jpeg, 1_600_000_000).expect("write");
    w.write(INTACT_CR2, &other.cr2, 1_600_000_000)
        .expect("write");
    w.write(INTACT_JPG, &other.jpeg, 1_600_000_000)
        .expect("write");
    for i in 0..4u64 {
        w.write(
            &format!("DCIM/101CANON/IMG_{:04}.JPG", 7000 + i),
            &ps_fixtures::media::jpeg(320, 240, 900 + i, 80),
            1_600_000_000,
        )
        .expect("write");
    }
    Card { _tmp: tmp, root }
}

fn group_of(env: &Env, file: FileId) -> Option<(FileId, Vec<FileId>, Vec<FileId>)> {
    env.readers()
        .group_memberships()
        .unwrap()
        .into_iter()
        .find(|(_, m, _)| m.contains(&file))
}

/// Scan `root` as 0.3.0 did (no structure check, so the cut CR2 shows its intact preview) and leave the catalog as
/// 0.3.0 left it: every row at metadata version 1.
fn catalog_of_0_3_0(root: &std::path::Path) -> (Env, SourceId) {
    let mut env = env_with(ScanOptions::default());
    let id = env.add(root);
    {
        let _legacy = Legacy::on();
        completed(&env.scan(&[id]));
    }
    env.close();
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&env.dir)).expect("db");
    c.execute_batch("UPDATE file SET meta_version = 1")
        .expect("as 0.3.0");
    drop(c);
    env.reopen(ScanOptions::default());
    (env, id)
}

fn under(root: &std::path::Path, rel: &str) -> PathBuf {
    RelPath::from_bytes(rel.as_bytes().to_vec())
        .expect("rel")
        .under(root)
}

#[test]
fn scenario_incremental_rescan_rescan_finds_a_raw_cut_after_its_intact_preview() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let (env, id) = catalog_of_0_3_0(&card.root);

    // What 0.3.0 made of it: preview pixels, hashes, one group with its camera JPEG, the CR2 a keeper (with its JPEG,
    // by the RAW+JPEG companion rule).
    let files = env.files(id);
    let (raw, jpeg) = (&files[CUT_CR2], &files[CUT_JPG]);
    assert!(raw.hashes.is_some() && raw.preview.is_none(), "{raw:?}");
    let g = group_of(&env, raw.id).expect("grouped");
    assert!(g.1.contains(&jpeg.id), "with its camera JPEG: {g:?}");
    assert!(g.2.contains(&raw.id), "the cut CR2 is a keeper: {g:?}");
    assert!(
        !env.readers()
            .files_awaiting_meta_reread()
            .unwrap()
            .is_empty(),
        "cleanup waits for the rescan"
    );

    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.damaged, 1, "counted as damaged");
    assert_eq!(s.errors, 1, "reported once");
    assert_eq!(s.diff.frame_reread, 2, "both CR2s checked");

    // Only the two CR2s were opened, read-only, and only their first bytes and directories were read.
    let read = env.instr.bytes_per_file();
    assert_eq!(read.len(), 2, "{read:?}");
    for rel in [CUT_CR2, INTACT_CR2] {
        let n = read[&under(&card.root, rel)];
        let size = std::fs::metadata(under(&card.root, rel)).unwrap().len();
        assert!(
            n <= ps_media::raw_check::STRUCTURE_HEAD as u64 + 16 * 1024,
            "{rel}: {n} of {size} bytes"
        );
    }
    assert!(env.instr.opens().iter().all(|o| !o.access.writes()));

    let files = env.files(id);
    let (raw, jpeg) = (&files[CUT_CR2], &files[CUT_JPG]);
    assert!(raw.hashes.is_none(), "its hashes are dropped");
    assert_eq!(
        raw.preview.map(|p| (p.generation, p.reason)),
        Some((1, PreviewReason::Unreadable))
    );
    assert_eq!(raw.meta_version, ps_model::META_LOGIC_VERSION);
    let t = env
        .s()
        .thumbs()
        .get(&raw.identity.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(t.kind, ThumbKind::Placeholder, "no preview pixels");
    match t.placeholder.and_then(|p| p.reason) {
        Some(PlaceholderReason::Unreadable { message }) => {
            assert!(message.contains("cut short"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    // Regrouped: the CR2 is out of its JPEG's group, and the JPEG is its own keeper.
    let g_raw = group_of(&env, raw.id);
    assert!(
        g_raw.as_ref().is_none_or(|g| !g.1.contains(&jpeg.id)),
        "{g_raw:?}"
    );
    if let Some((_, _, keepers)) = group_of(&env, jpeg.id) {
        assert!(keepers.contains(&jpeg.id), "the JPEG is the keeper");
    }
    // The intact pair keeps its group and its preview.
    let (ir, ij) = (&files[INTACT_CR2], &files[INTACT_JPG]);
    assert!(ir.hashes.is_some() && ir.preview.is_none());
    assert!(group_of(&env, ir.id).expect("grouped").1.contains(&ij.id));
    assert!(
        env.readers()
            .files_awaiting_meta_reread()
            .unwrap()
            .is_empty()
    );

    // Once: the next rescan opens nothing and still counts the damaged file.
    env.instr.reset();
    let again = env.scan(&[id]);
    completed(&again);
    assert_eq!(again.diff.frame_reread, 0);
    assert_eq!(env.instr.open_count(), 0, "no file opened");
    assert_eq!((again.errors, again.damaged), (0, 1));
}

/// Rows a version without frame identity left (metadata version 0) get both checks from one read; non-RAW rows
/// at version 1 are not read.
#[test]
fn a_version_0_row_gets_both_checks_from_one_read() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let (env, id) = catalog_of_0_3_0(&card.root);
    let mut env = env;
    env.close();
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&env.dir)).expect("db");
    c.execute(
        "UPDATE file SET meta_version = 0 WHERE rel_path = CAST(?1 AS BLOB)",
        [CUT_CR2],
    )
    .expect("version 0");
    drop(c);
    env.reopen(ScanOptions::default());
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.damaged, s.diff.frame_reread), (1, 2));
    let read = env.instr.bytes_per_file();
    assert_eq!(read.len(), 2, "{read:?}");
    assert!(
        read[&under(&card.root, CUT_CR2)] <= ps_media::PROBE_LIMIT as u64,
        "the probe window and its directories"
    );
    let raw = &env.files(id)[CUT_CR2];
    assert_eq!(raw.meta_version, ps_model::META_LOGIC_VERSION);
    assert!(raw.hashes.is_none());
}

/// A fresh catalog never runs the pass: the analysis judged the whole file and stamps the current version.
#[test]
fn a_fresh_catalog_never_rechecks() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let env = env_with(ScanOptions::default());
    let id = env.add(&card.root);
    let first = env.scan(&[id]);
    completed(&first);
    assert_eq!(first.damaged, 1);
    env.instr.reset();
    let again = env.scan(&[id]);
    completed(&again);
    assert_eq!(again.diff.frame_reread, 0);
    assert_eq!(env.instr.total_bytes(), 0);
}
