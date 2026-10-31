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
/// The CR2's sensor data: far more than its preview, as in a camera's file.
const CR2_SENSOR_BYTES: u32 = 8 * 1024 * 1024;

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
        cfx::canon_cr2_with_sensor(
            &ps_fixtures::media::jpeg(1600, 1066, 32, 92),
            (1600, 1066),
            2,
            CR2_SENSOR_BYTES,
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

    // Only the `_` files were opened, read-only; the sRGB one only within the probe window, the Adobe RGB JPEG whole,
    // the Adobe RGB CR2 its probe window and its embedded preview, not its sensor data.
    let read = env.instr.bytes_per_file();
    let root = env.root(id);
    assert_eq!(read.len(), 3, "{read:?}");
    assert!(env.instr.opens().iter().all(|o| !o.access.writes()));
    let n = read[&under(&root, SRGB_UNDERSCORE)];
    assert!(n <= ps_media::PROBE_LIMIT as u64, "{n} bytes");
    assert_eq!(
        read[&under(&root, ADOBE_JPG)],
        card.files[ADOBE_JPG].len() as u64,
        "the JPEG is read whole once"
    );
    let cr2 = &card.files[ADOBE_CR2];
    let preview = ps_media::probe_buffer(cr2, None).preview.unwrap().range;
    assert!(
        preview.offset < ps_media::PROBE_LIMIT as u64
            && preview.end() > ps_media::PROBE_LIMIT as u64,
        "the preview starts in the probe window and ends beyond it: {preview:?}"
    );
    assert_eq!(
        read[&under(&root, ADOBE_CR2)],
        preview.end(),
        "the CR2's probe window and the rest of its preview, no byte twice ({} bytes of {})",
        preview.len,
        cr2.len()
    );
    assert!(cr2.len() as u64 > CR2_SENSOR_BYTES as u64);
    // The run's byte total is what it read, so its progress and ETA were honest.
    let snap = env.s().snapshot().unwrap();
    assert_eq!(snap.bytes_to_read, snap.bytes_read, "{snap:?}");
    assert_eq!(snap.bytes_read, read.values().sum::<u64>());
    assert_eq!(
        s.diff.colour_bytes, snap.bytes_read,
        "the summary says what it read"
    );

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
    // Every byte the two checks read is in the run's total: the structure check's directory reads too.
    let snap = env.s().snapshot().unwrap();
    assert_eq!(snap.bytes_to_read, snap.bytes_read, "{snap:?}");
    assert_eq!(
        snap.bytes_read,
        env.instr.bytes_per_file().values().sum::<u64>()
    );
    assert_eq!(s.diff.colour_bytes, snap.bytes_read);
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

/// A card of `files` (name, bytes), each written with the same modification time.
fn card_of(files: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("card");
    let w = ps_fixtures::DirWriter::new(&root).expect("writer");
    for (rel, bytes) in files {
        w.write(rel, bytes, 1_700_000_000).expect("write");
    }
    (tmp, root)
}

fn sql(env: &mut Env, statement: &str) {
    env.close();
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&env.dir)).expect("db");
    c.execute_batch(statement).expect("sql");
    drop(c);
    env.reopen(ScanOptions::default());
}

/// Other bytes under an unchanged stat fingerprint: the JPEG's identity check catches it, the row alone is marked
/// unchecked and the run reports it; a copy of the catalogued content keeps the colour it recorded.
#[test]
fn an_identity_mismatch_marks_only_its_own_row() {
    let _h = hook();
    let _s = shared();
    let adobe = cfx::with_exif(
        &ps_fixtures::media::jpeg(2400, 1600, 41, 92),
        0xFFFF,
        Some("R03"),
        Some(2),
    );
    let (_tmp, root) = card_of(&[("IMG_0001.JPG", &adobe), ("_MG_0001.JPG", &adobe)]);
    let (mut env, id) = catalog_before_the_change(&root);
    // The copy not due the re-check had its colour recorded (by an analysis of this build, say).
    sql(
        &mut env,
        "UPDATE file SET colour = 'adobe_rgb' WHERE name = CAST('IMG_0001.JPG' AS BLOB)",
    );
    let mut other = adobe.clone();
    let mid = other.len() / 2;
    for b in &mut other[mid..mid + 64] {
        *b ^= 0x5A;
    }
    ps_fixtures::writer::rewrite_keeping_stat(&under(&env.root(id), "_MG_0001.JPG"), &other)
        .unwrap();
    let thumb_before = thumb(&env, id, "IMG_0001.JPG");
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.diff.colour_checked, s.diff.recoloured), (1, 0));
    assert_eq!(s.errors, 1, "reported");
    let snap = env.s().snapshot().unwrap();
    assert!(
        snap.recent_errors[0]
            .message
            .contains("differ from the catalogued content"),
        "{:?}",
        snap.recent_errors
    );
    let files = env.files(id);
    assert_eq!(
        files["_MG_0001.JPG"].colour.as_deref(),
        Some(COLOUR_UNCHECKED)
    );
    assert_eq!(
        files["IMG_0001.JPG"].colour.as_deref(),
        Some("adobe_rgb"),
        "the other copy keeps its colour"
    );
    assert_eq!(
        thumb(&env, id, "IMG_0001.JPG"),
        thumb_before,
        "thumbnail untouched"
    );
}

/// A RAW whose preview no longer decodes (other bytes under the unchanged fingerprint): its colour space is recorded
/// as not converted, the stored thumbnail kept, and the inspector can say so; the row is not re-checked again.
#[test]
fn a_preview_that_cannot_be_made_again_is_recorded_unconverted() {
    let _h = hook();
    let _s = shared();
    let cr2 = cfx::canon_cr2_with_sensor(
        &ps_fixtures::media::jpeg(1600, 1066, 42, 92),
        (1600, 1066),
        2,
        1024 * 1024,
    );
    let (_tmp, root) = card_of(&[("_MG_0002.CR2", &cr2)]);
    let (env, id) = catalog_before_the_change(&root);
    let before = thumb(&env, id, "_MG_0002.CR2");
    let range = ps_media::probe_buffer(&cr2, None).preview.unwrap().range;
    let mut broken = cr2.clone();
    for b in &mut broken[range.offset as usize + 2..range.end() as usize] {
        *b = 0;
    }
    ps_fixtures::writer::rewrite_keeping_stat(&under(&env.root(id), "_MG_0002.CR2"), &broken)
        .unwrap();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!((s.diff.colour_checked, s.diff.recoloured), (1, 0));
    let f = &env.files(id)["_MG_0002.CR2"];
    assert_eq!(f.colour.as_deref(), Some("unconverted:adobe_rgb"));
    assert_eq!(thumb(&env, id, "_MG_0002.CR2"), before, "kept");
    assert_eq!(
        ps_model::StoredColour::from_storage(f.colour.as_deref().unwrap())
            .and_then(|c| c.label())
            .as_deref(),
        Some("Adobe RGB (1998), preview not converted")
    );
    env.instr.reset();
    let again = env.scan(&[id]);
    assert_eq!((again.diff.colour_checked, env.instr.open_count()), (0, 0));
}

/// The hash upgrade (ADR-0012) does not hash a thumbnail converted to sRGB (ADR-0113): such rows keep their old
/// hash version and the next rescan analyses them again, from the decode; the others are upgraded from their
/// thumbnails as before.
#[test]
fn the_hash_upgrade_leaves_converted_thumbnails_to_a_new_analysis() {
    let _h = hook();
    let _s = shared();
    let adobe = cfx::with_exif(
        &ps_fixtures::media::jpeg(1200, 800, 43, 92),
        0xFFFF,
        Some("R03"),
        Some(2),
    );
    let srgb = cfx::with_exif(
        &ps_fixtures::media::jpeg(1200, 800, 44, 92),
        1,
        None,
        Some(1),
    );
    let (_tmp, root) = card_of(&[("_MG_0001.JPG", &adobe), ("IMG_0002.JPG", &srgb)]);
    let mut env = env_with(ScanOptions::default());
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    let hashes: BTreeMap<String, _> = env
        .files(id)
        .into_iter()
        .map(|(rel, f)| (rel, f.hashes.unwrap()))
        .collect();
    // Hashes of an older algorithm version.
    sql(&mut env, "UPDATE file SET hash_version = 0");
    let report = ps_scan::upgrade_hashes_current(env.s().catalog(), env.s().thumbs()).unwrap();
    assert_eq!(
        (report.stale, report.upgraded, report.left_for_analysis),
        (2, 1, 1),
        "{report:?}"
    );
    let files = env.files(id);
    assert_eq!(
        files["_MG_0001.JPG"].hashes.unwrap().version,
        0,
        "not hashed from its converted thumbnail"
    );
    assert_eq!(
        files["IMG_0002.JPG"].hashes.unwrap().version,
        ps_hash::PERCEPTUAL_HASH_VERSION
    );
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.reanalyzed, 1, "{:?}", s.diff);
    let read = env.instr.bytes_per_file();
    assert_eq!(read.len(), 1, "{read:?}");
    let f = &env.files(id)["_MG_0001.JPG"];
    assert_eq!(
        f.hashes,
        Some(hashes["_MG_0001.JPG"]),
        "hashed again from the unconverted decode"
    );
    assert_eq!(f.colour.as_deref(), Some("adobe_rgb"));
    // Done: nothing more to read.
    env.instr.reset();
    let again = env.scan(&[id]);
    assert_eq!((again.diff.reanalyzed, env.instr.open_count()), (0, 0));
}

/// A RAW placeholder of an older decoder generation filled in by a re-decode (ADR-0109) records the colour its new
/// thumbnail was converted from.
#[test]
fn a_filled_in_placeholder_records_its_colour() {
    let _h = hook();
    let _s = shared();
    let cr2 = cfx::canon_cr2_of(
        &ps_fixtures::media::jpeg(1600, 1066, 45, 92),
        (1600, 1066),
        2,
    );
    let (_tmp, root) = card_of(&[("_MG_0003.CR2", &cr2)]);
    let mut env = env_with(ScanOptions::default());
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    // As an older version left it: a generation-0 placeholder, no hashes, the colour not read.
    let r = env.files(id)["_MG_0003.CR2"].clone();
    let probe = ps_media::probe_buffer(&cr2, None);
    let mut ph = ps_media::thumb::placeholder(&probe, None, "preview unavailable");
    if let Some(p) = ph.placeholder.as_mut() {
        p.generation = 0;
    }
    env.s()
        .thumbs()
        .put_many([(&r.identity.unwrap(), &ph)])
        .unwrap();
    sql(
        &mut env,
        "UPDATE file SET phash = NULL, dhash = NULL, hash_version = NULL, preview_gen = 0,
                         preview_reason = 'unreadable', colour = 'unchecked'",
    );
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.redecoded, 1, "{:?}", s.diff);
    let f = &env.files(id)["_MG_0003.CR2"];
    assert_eq!(f.preview, None);
    assert_eq!(f.colour.as_deref(), Some("adobe_rgb"));
    assert_eq!(
        thumb(&env, id, "_MG_0003.CR2"),
        fresh_thumb(&cr2),
        "converted"
    );
}

/// A hash-version change re-reads only rows whose content's thumbnail was converted (the hash upgrade leaves those
/// alone); a row whose colour is unchecked, not read yet, sRGB or kept unconverted keeps a thumbnail the upgrade can
/// hash, so no rescan reads it for its hashes (ADR-0119).
#[test]
fn a_stale_hash_version_re_reads_only_converted_rows() {
    let _h = hook();
    let _s = shared();
    let adobe = |seed| {
        cfx::with_exif(
            &ps_fixtures::media::jpeg(1200, 800, seed, 92),
            0xFFFF,
            Some("R03"),
            Some(2),
        )
    };
    let srgb = |seed| {
        cfx::with_exif(
            &ps_fixtures::media::jpeg(1200, 800, seed, 92),
            1,
            None,
            Some(1),
        )
    };
    let files = [
        ("_MG_0001.JPG", adobe(51)),
        ("IMG_0002.JPG", srgb(52)),
        ("IMG_0003.JPG", srgb(53)),
        ("IMG_0004.JPG", srgb(54)),
        ("IMG_0005.JPG", adobe(55)),
    ];
    let named: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let (_tmp, root) = card_of(&named);
    let mut env = env_with(ScanOptions::default());
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    assert_eq!(
        env.files(id)["_MG_0001.JPG"].colour.as_deref(),
        Some("adobe_rgb")
    );
    sql(
        &mut env,
        "UPDATE file SET hash_version = 0;
         UPDATE file SET colour = 'unchecked' WHERE name = CAST('IMG_0003.JPG' AS BLOB);
         UPDATE file SET colour = NULL WHERE name = CAST('IMG_0004.JPG' AS BLOB);
         UPDATE file SET colour = 'unconverted:adobe_rgb' WHERE name = CAST('IMG_0005.JPG' AS BLOB);",
    );
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.reanalyzed, 1, "{:?}", s.diff);
    let read = env.instr.bytes_per_file();
    assert_eq!(
        read.keys().collect::<Vec<_>>(),
        [&under(&env.root(id), "_MG_0001.JPG")],
        "only the converted row is read"
    );
    let files = env.files(id);
    assert_eq!(
        files["_MG_0001.JPG"].hashes.unwrap().version,
        ps_hash::PERCEPTUAL_HASH_VERSION
    );
    for rel in [
        "IMG_0002.JPG",
        "IMG_0003.JPG",
        "IMG_0004.JPG",
        "IMG_0005.JPG",
    ] {
        assert_eq!(
            files[rel].hashes.unwrap().version,
            0,
            "{rel}: left to the hash upgrade"
        );
    }
}

/// A RAW replaced in place by another shot under the same size and modification time: the hashes of its new preview's
/// decode are not the row's, so its thumbnail, shared by every identical copy, is left as it was; only its row is
/// marked unchecked and the run reports it. The sensor data is still not read.
#[test]
fn a_raw_replaced_by_another_shot_keeps_its_thumbnail() {
    let _h = hook();
    let _s = shared();
    let raw = |seed| {
        cfx::canon_cr2_with_sensor(
            &ps_fixtures::media::jpeg(1600, 1066, seed, 92),
            (1600, 1066),
            2,
            2 * 1024 * 1024,
        )
    };
    let (mut first, mut other) = (raw(61), raw(62));
    // The same length (trailing bytes after the last directory's data are not read by anything).
    let len = first.len().max(other.len());
    first.resize(len, 0);
    other.resize(len, 0);
    assert_ne!(first, other);
    let (_tmp, root) = card_of(&[("_MG_0002.CR2", &first), ("copy/IMG_0002.CR2", &first)]);
    let (env, id) = catalog_before_the_change(&root);
    let before = thumb(&env, id, "_MG_0002.CR2");
    ps_fixtures::writer::rewrite_keeping_stat(&under(&env.root(id), "_MG_0002.CR2"), &other)
        .unwrap();
    env.instr.reset();
    let s = env.scan(&[id]);
    completed(&s);
    assert_eq!(
        (s.diff.colour_checked, s.diff.recoloured),
        (1, 0),
        "{:?}",
        s.diff
    );
    assert_eq!(s.errors, 1, "reported");
    let snap = env.s().snapshot().unwrap();
    assert!(
        snap.recent_errors[0]
            .message
            .contains("differ from the catalogued content"),
        "{:?}",
        snap.recent_errors
    );
    let files = env.files(id);
    assert_eq!(
        files["_MG_0002.CR2"].colour.as_deref(),
        Some(COLOUR_UNCHECKED)
    );
    assert_eq!(
        thumb(&env, id, "copy/IMG_0002.CR2"),
        before,
        "the shared thumbnail is untouched"
    );
    let preview = ps_media::probe_buffer(&other, None).preview.unwrap().range;
    let n = env.instr.bytes_per_file()[&under(&env.root(id), "_MG_0002.CR2")];
    assert_eq!(
        n,
        preview.end().max(ps_media::PROBE_LIMIT as u64),
        "the sensor data is not read"
    );
    assert!(n < len as u64 / 2, "{n} of {len}");
}
