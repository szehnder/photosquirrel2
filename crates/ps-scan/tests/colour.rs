//! Colour spaces in the scanner (adobe-rgb-color, ADR-0113): perceptual hashes come from the unconverted decode, and
//! a catalog written before the change has the thumbnails of its `_`-named Adobe RGB files made again, once, by the
//! next rescan, reading nothing else. A test binary of its own: the tests switch the process-wide conversion hook,
//! one at a time.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use common::*;
use ps_fixtures::CorpusWriter as _;
use ps_fixtures::colour as cfx;
use ps_model::{COLOUR_UNCHECKED, FileId, RelPath, SourceId};
use ps_scan::ScanOptions;

/// One test at a time: the hook is process-wide.
static HOOK: Mutex<()> = Mutex::new(());

fn hook() -> std::sync::MutexGuard<'static, ()> {
    HOOK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Turns the conversion of previews off while alive (the build before ADR-0113), back on when dropped.
struct Legacy;

impl Legacy {
    fn on() -> Self {
        ps_model::set_colour_conversion_enabled(false);
        Legacy
    }
}

impl Drop for Legacy {
    fn drop(&mut self) {
        ps_model::set_colour_conversion_enabled(true);
    }
}

const ADOBE_JPG: &str = "DCIM/100CANON/_MG_0001.JPG";
const ADOBE_CR2: &str = "DCIM/100CANON/_MG_0002.CR2";
const SRGB_UNDERSCORE: &str = "DCIM/100CANON/_MG_0003.JPG";
/// An Adobe RGB file renamed by an import tool: not re-checked (the owner's choice, D4).
const ADOBE_RENAMED: &str = "DCIM/100CANON/IMG_0004.JPG";
const SRGB_JPG: &str = "DCIM/100CANON/IMG_0005.JPG";

struct Card {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    files: BTreeMap<&'static str, Vec<u8>>,
}

/// Camera-sized pictures (larger than the probe window), recorded the ways a Canon body set to Adobe RGB and one set
/// to sRGB record them.
fn card() -> Card {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("card");
    let w = ps_fixtures::DirWriter::new(&root).expect("writer");
    let photo = |seed| ps_fixtures::media::jpeg(2400, 1600, seed, 92);
    let mut files = BTreeMap::new();
    files.insert(
        ADOBE_JPG,
        cfx::with_exif(&photo(31), 0xFFFF, Some("R03"), Some(2)),
    );
    files.insert(
        ADOBE_CR2,
        cfx::canon_cr2_of(
            &ps_fixtures::media::jpeg(1600, 1066, 32, 92),
            (1600, 1066),
            2,
        ),
    );
    files.insert(
        SRGB_UNDERSCORE,
        cfx::with_exif(&photo(33), 1, None, Some(1)),
    );
    files.insert(
        ADOBE_RENAMED,
        cfx::with_exif(&photo(34), 0xFFFF, Some("R03"), Some(2)),
    );
    files.insert(SRGB_JPG, cfx::with_exif(&photo(35), 1, None, Some(1)));
    for (rel, bytes) in &files {
        assert!(
            bytes.len() > ps_media::PROBE_LIMIT,
            "{rel}: {} bytes",
            bytes.len()
        );
        w.write(rel, bytes, 1_700_000_000).expect("write");
    }
    Card {
        _tmp: tmp,
        root,
        files,
    }
}

fn under(root: &Path, rel: &str) -> PathBuf {
    RelPath::from_bytes(rel.as_bytes().to_vec())
        .expect("rel")
        .under(root)
}

fn thumb(env: &Env, id: SourceId, rel: &str) -> Vec<u8> {
    let f = &env.files(id)[rel];
    env.s()
        .thumbs()
        .get(&f.identity.unwrap())
        .unwrap()
        .unwrap()
        .jpeg
}

/// What this build makes of `bytes` on the thumbnail path the re-check takes.
fn fresh_thumb(bytes: &[u8]) -> Vec<u8> {
    let p = ps_media::probe_buffer(bytes, None);
    ps_media::make_thumbnail(bytes, &p, &ps_media::Decoders::none())
        .unwrap()
        .0
        .jpeg
}

fn stamps(env: &Env) -> Vec<(FileId, u64)> {
    env.readers().dedup_stamps().unwrap()
}

/// Scan `root` as the build before this change did (no conversion; the thumbnails are as it stored them) and leave
/// the catalog as migration 26 leaves such a catalog.
fn catalog_before_the_change(root: &Path) -> (Env, SourceId) {
    let mut env = env_with(ScanOptions::default());
    let id = env.add(root);
    {
        let _legacy = Legacy::on();
        completed(&env.scan(&[id]));
    }
    env.close();
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&env.dir)).expect("db");
    c.execute_batch(&ps_catalog::db::colour_reset_sql())
        .expect("as before migration 26");
    drop(c);
    env.reopen(ScanOptions::default());
    (env, id)
}

#[test]
fn scenario_thumbnails_rescan_recolours_old_adobe_rgb_thumbnails_only() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let (env, id) = catalog_before_the_change(&card.root);
    let before: BTreeMap<&str, Vec<u8>> = card
        .files
        .keys()
        .map(|rel| (*rel, thumb(&env, id, rel)))
        .collect();
    let colour_of = |env: &Env, rel: &str| env.files(id)[rel].colour.clone();
    for rel in [ADOBE_JPG, ADOBE_CR2, SRGB_UNDERSCORE] {
        assert_eq!(colour_of(&env, rel), None, "{rel} is due the re-check");
    }
    for rel in [ADOBE_RENAMED, SRGB_JPG] {
        assert_eq!(colour_of(&env, rel).as_deref(), Some(COLOUR_UNCHECKED));
    }
    let hashes_before: Vec<_> = env.files(id).values().map(|f| f.hashes).collect();
    let stamps_before = stamps(&env);

    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.colour_checked, 3, "{:?}", s.diff);
    assert_eq!(s.diff.recoloured, 2);
    assert_eq!(s.errors, 0);

    // Only the `_` files were opened, read-only; the sRGB one only within the probe window, the Adobe RGB ones whole.
    let read = env.instr.bytes_per_file();
    let root = env.root(id);
    assert_eq!(read.len(), 3, "{read:?}");
    assert!(env.instr.opens().iter().all(|o| !o.access.writes()));
    let n = read[&under(&root, SRGB_UNDERSCORE)];
    assert!(n <= ps_media::PROBE_LIMIT as u64, "{n} bytes");
    for rel in [ADOBE_JPG, ADOBE_CR2] {
        assert_eq!(
            read[&under(&root, rel)],
            card.files[rel].len() as u64,
            "{rel}: read whole once"
        );
    }

    // The Adobe RGB thumbnails are made again, converted; the others are untouched.
    for rel in [ADOBE_JPG, ADOBE_CR2] {
        let now = thumb(&env, id, rel);
        assert_ne!(now, before[rel], "{rel}: recoloured");
        assert_eq!(
            now,
            fresh_thumb(&card.files[rel]),
            "{rel}: as this build makes it"
        );
    }
    for rel in [SRGB_UNDERSCORE, ADOBE_RENAMED, SRGB_JPG] {
        assert_eq!(thumb(&env, id, rel), before[rel], "{rel}: untouched");
    }
    assert_eq!(colour_of(&env, ADOBE_JPG).as_deref(), Some("adobe_rgb"));
    assert_eq!(colour_of(&env, ADOBE_CR2).as_deref(), Some("adobe_rgb"));
    assert_eq!(colour_of(&env, SRGB_UNDERSCORE).as_deref(), Some("srgb"));
    assert_eq!(
        colour_of(&env, ADOBE_RENAMED).as_deref(),
        Some(COLOUR_UNCHECKED)
    );

    // Nothing grouping reads moved.
    let hashes_after: Vec<_> = env.files(id).values().map(|f| f.hashes).collect();
    assert_eq!(hashes_after, hashes_before);
    assert_eq!(stamps(&env), stamps_before, "the dedup stamp is unchanged");
}

#[test]
fn scenario_thumbnails_the_re_check_happens_once() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let (env, id) = catalog_before_the_change(&card.root);
    completed(&env.scan(&[id]));
    env.instr.reset();
    let again = env.scan(&[id]);
    completed(&again);
    assert_eq!(again.diff.colour_checked, 0);
    assert_eq!(env.instr.open_count(), 0, "no file opened");
    // A fresh catalog never re-checks: the analysis records the colour.
    let fresh = env_with(ScanOptions::default());
    let fid = fresh.add(&card.root);
    completed(&fresh.scan(&[fid]));
    assert!(fresh.files(fid).values().all(|f| f.colour.is_some()));
    fresh.instr.reset();
    let s = fresh.scan(&[fid]);
    assert_eq!((s.diff.colour_checked, fresh.instr.open_count()), (0, 0));
}

#[test]
fn scenario_thumbnails_hashes_unchanged_by_conversion() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let scan = |legacy: bool| {
        let env = env_with(ScanOptions::default());
        let id = env.add(&card.root);
        {
            let _legacy = legacy.then(Legacy::on);
            completed(&env.scan(&[id]));
        }
        let files = env.files(id);
        let thumbs: BTreeMap<String, Vec<u8>> = files
            .keys()
            .map(|rel| (rel.clone(), thumb(&env, id, rel)))
            .collect();
        let hashes: BTreeMap<String, _> = files
            .into_iter()
            .map(|(rel, f)| (rel, (f.hashes, f.identity)))
            .collect();
        (hashes, thumbs)
    };
    let (converted, thumbs_on) = scan(false);
    let (unconverted, thumbs_off) = scan(true);
    assert!(converted.values().all(|(h, _)| h.is_some()));
    assert_eq!(converted, unconverted, "perceptual hashes are equal");
    // The hook did switch the conversion: the Adobe RGB thumbnails differ, the sRGB ones do not.
    for rel in [ADOBE_JPG, ADOBE_CR2, ADOBE_RENAMED] {
        assert_ne!(thumbs_on[rel], thumbs_off[rel], "{rel}");
    }
    for rel in [SRGB_UNDERSCORE, SRGB_JPG] {
        assert_eq!(thumbs_on[rel], thumbs_off[rel], "{rel}");
    }
}

/// A `_` RAW due both one-time checks (the ADR-0117 structure check and the colour re-check) is opened once, and
/// each stamp is written on its own.
#[test]
fn a_raw_due_both_checks_is_opened_once() {
    let _h = hook();
    let _s = shared();
    let card = card();
    let (mut env, id) = catalog_before_the_change(&card.root);
    env.close();
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&env.dir)).expect("db");
    c.execute_batch("UPDATE file SET meta_version = 1 WHERE format LIKE 'raw:%'")
        .expect("as 0.3.0");
    drop(c);
    env.reopen(ScanOptions::default());
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.diff.frame_reread, s.diff.colour_checked), (1, 3));
    let raw = under(&env.root(id), ADOBE_CR2);
    assert_eq!(
        env.instr.opens().iter().filter(|o| o.path == raw).count(),
        1,
        "one open serves both checks"
    );
    let f = &env.files(id)[ADOBE_CR2];
    assert_eq!(f.meta_version, ps_model::META_LOGIC_VERSION);
    assert_eq!(f.colour.as_deref(), Some("adobe_rgb"));
    assert!(f.hashes.is_some() && f.preview.is_none(), "not cut");
    // Both done: the next rescan opens nothing.
    env.instr.reset();
    let again = env.scan(&[id]);
    assert_eq!((again.diff.frame_reread, again.diff.colour_checked), (0, 0));
    assert_eq!(env.instr.open_count(), 0);
}
