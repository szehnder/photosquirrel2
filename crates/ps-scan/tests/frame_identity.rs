//! `incremental-rescan` and `media-metadata` scenarios of burst-frame-identity (ADR-0105): RAW rows of a catalog
//! written before frame identity are re-read once (header only) and regrouped; frame identity costs no extra
//! read. A test binary of its own: the tests switch the process-wide frame-identity hook, one at a time.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use common::*;
use ps_fixtures::dng::{BurstFrameFiles, converted_burst_frame, converted_burst_frame_with};
use ps_model::{FileId, SourceId};
use ps_scan::ScanOptions;

/// One test at a time: the hook is process-wide.
static HOOK: Mutex<()> = Mutex::new(());

fn hook() -> std::sync::MutexGuard<'static, ()> {
    HOOK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Turns frame identity off while alive (the build before ADR-0105), back on when dropped.
struct Legacy;

impl Legacy {
    fn on() -> Self {
        ps_model::frame::set_frame_identity_enabled(false);
        Legacy
    }
}

impl Drop for Legacy {
    fn drop(&mut self) {
        ps_model::frame::set_frame_identity_enabled(true);
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    ps_fixtures::scratch::write(path, bytes).expect("write");
}

/// Frames 34 (sub-second 00) and 35 (140 ms) of a converted burst, as on the owner's card: CR2 and JPEG in
/// `DCIM/101CANON`, the DNG and a copy of the JPEG in `2020/2020-04-20`. Frame 34's DNG shows frame 35 the way
/// the converter rendered it (pHash distance 2 to frame 35's JPEG, 4 to its own), so without frame identity it
/// joins frame 35.
fn card(root: &Path) -> [BurstFrameFiles; 2] {
    let frames = [
        converted_burst_frame_with(3, 0, 3, 960, 1),
        converted_burst_frame(3, 1, 140, 960),
    ];
    for f in &frames {
        write(&root.join(format!("DCIM/101CANON/{}.CR2", f.stem)), &f.cr2);
        write(&root.join(format!("DCIM/101CANON/{}.JPG", f.stem)), &f.jpeg);
        write(
            &root.join(format!("2020/2020-04-20/{}.DNG", f.stem)),
            &f.dng,
        );
        write(
            &root.join(format!("2020/2020-04-20/{}.JPG", f.stem)),
            &f.jpeg,
        );
    }
    // Files whose frame cannot matter (ADR-0106): a Nikon JPEG of the burst's second, and a Canon JPEG of another
    // second. Migration 23 marks them current; the re-read never opens them.
    for (path, make, model, time, seed) in OTHERS {
        let spec = ps_fixtures::media::ExifSpec {
            make: Some(make.into()),
            model: Some(model.into()),
            date_time_original: Some(time.into()),
            sub_sec_original: Some("14".into()),
            maker_note: Some(ps_fixtures::media::canon_maker_note()),
            ..Default::default()
        };
        write(
            &root.join(path),
            &ps_fixtures::media::jpeg_with_exif(960, 640, seed, 90, &spec),
        );
    }
    frames
}

/// (path, make, model, capture time, scene seed) of the files [`card`] adds that the re-read must not open.
const OTHERS: [(&str, &str, &str, &str, u64); 2] = [
    (
        "friends/DSC_0001.JPG",
        "NIKON CORPORATION",
        "NIKON D750",
        "2019:07:04 12:00:05",
        71,
    ),
    (
        "DCIM/101CANON/IMG_0099.JPG",
        "Canon",
        "Canon EOS 5D Mark III",
        "2019:07:04 12:00:09",
        72,
    ),
];

/// The group (its id) of every file of `source`, by path.
fn groups(env: &Env, source: SourceId) -> std::collections::BTreeMap<String, Option<FileId>> {
    env.files(source)
        .into_iter()
        .map(|(p, r)| {
            let g = env.readers().group_of_file(r.id).expect("group of");
            (p, g)
        })
        .collect()
}

fn members_of(g: &std::collections::BTreeMap<String, Option<FileId>>, path: &str) -> Vec<String> {
    let id = g[path].expect("grouped");
    g.iter()
        .filter(|(_, x)| **x == Some(id))
        .map(|(p, _)| p.clone())
        .collect()
}

/// Scan `root` as the build before frame identity did, and leave the catalog as migration 23 leaves such a
/// catalog: RAW rows at metadata version 0 without a frame id, the others current.
fn old_catalog(root: &Path) -> (Env, SourceId) {
    let mut env = env_with(ScanOptions::default());
    let id = env.add(root);
    {
        let _legacy = Legacy::on();
        completed(&env.scan(&[id]));
    }
    env.close();
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&env.dir)).expect("db");
    c.execute_batch(&ps_catalog::db::frame_identity_reset_sql())
        .expect("downgrade");
    drop(c);
    env.reopen(ScanOptions::default());
    (env, id)
}

fn scratch_root(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("card")
}

#[test]
fn scenario_incremental_rescan_old_catalog_splits_merged_burst_frames_on_rescan() {
    let _g = hook();
    let _s = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = scratch_root(&tmp);
    card(&root);
    let (env, id) = old_catalog(&root);
    let before = groups(&env, id);
    let dng34 = "2020/2020-04-20/IMG_0034.DNG";
    assert!(
        members_of(&before, dng34).contains(&"DCIM/101CANON/IMG_0035.CR2".to_owned()),
        "the old catalog holds the merge: {before:?}"
    );

    env.instr.reset();
    let summary = env.scan(&[id]);
    completed(&summary);
    // Only the RAW and DNG files and the Canon JPEGs of their second were read, each at most its header.
    let read = env.instr.bytes_per_file();
    assert_eq!(read.len(), 8, "{read:?}");
    for (path, n) in &read {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        assert!(matches!(ext, "CR2" | "DNG" | "JPG"), "{path:?} read");
        assert!(
            OTHERS.iter().all(|o| !path.ends_with(o.0)),
            "{path:?}: not a file whose frame can matter"
        );
        assert!(*n <= ps_media::PROBE_LIMIT as u64, "{path:?}: {n} bytes");
    }
    assert!(
        env.instr.opens().iter().all(|o| !o.access.writes()),
        "read-only"
    );
    for o in env.instr.opens() {
        assert!(
            OTHERS.iter().all(|x| !o.path.ends_with(x.0)),
            "{:?} opened",
            o.path
        );
    }
    assert_eq!(summary.diff.frame_reread, 8);
    assert_eq!(summary.grouping.as_ref().map(|g| g.groups_split), Some(1));

    let after = groups(&env, id);
    let mut f34 = members_of(&after, dng34);
    f34.sort();
    assert_eq!(
        f34,
        vec![
            "2020/2020-04-20/IMG_0034.DNG",
            "2020/2020-04-20/IMG_0034.JPG",
            "DCIM/101CANON/IMG_0034.CR2",
            "DCIM/101CANON/IMG_0034.JPG",
        ]
    );
    let f35 = members_of(&after, "DCIM/101CANON/IMG_0035.CR2");
    assert_eq!(f35.len(), 4, "{f35:?}");
    assert!(f35.iter().all(|p| p.contains("IMG_0035")), "{f35:?}");
    // The stored metadata now records the frames.
    let files = env.files(id);
    assert!(
        files
            .values()
            .all(|r| r.meta_version == ps_model::META_LOGIC_VERSION)
    );
}

#[test]
fn scenario_incremental_rescan_the_re_read_happens_once() {
    let _g = hook();
    let _s = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = scratch_root(&tmp);
    card(&root);
    let (env, id) = old_catalog(&root);
    let first = env.scan(&[id]);
    completed(&first);
    assert_eq!(first.diff.frame_reread, 8);
    env.instr.reset();
    let second = env.scan(&[id]);
    completed(&second);
    assert_eq!(second.diff.frame_reread, 0);
    assert_eq!(env.instr.open_count(), 0, "no file opened");
    assert_eq!(env.instr.total_bytes(), 0);
    assert_eq!(second.grouping.as_ref().map_or(0, |g| g.groups_split), 0);
    // A fresh catalog never re-reads: analysis records frame identity already.
    let fresh = env_with(ScanOptions::default());
    let fid = fresh.add(&root);
    completed(&fresh.scan(&[fid]));
    fresh.instr.reset();
    let again = fresh.scan(&[fid]);
    assert_eq!(again.diff.frame_reread, 0);
    assert_eq!(fresh.instr.total_bytes(), 0);
}

#[test]
fn scenario_media_metadata_frame_identity_costs_no_extra_read() {
    let _g = hook();
    let _s = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("big");
    let f = converted_burst_frame(13, 0, 3, 1200);
    let dng = ps_fixtures::media::pad_to(&f.dng, 25 * 1024 * 1024);
    write(&root.join("IMG_0034.DNG"), &dng);
    let read = |legacy: bool| -> (u64, Option<ps_model::FrameId>) {
        let env = env_with(ScanOptions::default());
        let id = env.add(&root);
        {
            let _l = legacy.then(Legacy::on);
            completed(&env.scan(&[id]));
        }
        let n = env.instr.total_bytes();
        let rec = env.files(id).into_values().next().expect("file");
        let meta = env
            .readers()
            .metadata(rec.id)
            .expect("metadata")
            .expect("analysed");
        (n, meta.fields.frame_id)
    };
    let (before, none) = read(true);
    let (after, frame) = read(false);
    assert_eq!(none, None, "the hook reads no frame identity");
    assert!(frame.is_some_and(|f| f.raw_name.as_deref() == Some("IMG_0034")));
    assert_eq!(after, before, "the same bytes as before frame identity");
    assert!(after >= dng.len() as u64, "analysed in full: {after}");
}
