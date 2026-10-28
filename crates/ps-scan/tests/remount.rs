//! `stable-ids-on-fat` (ADR-0112): rescans of a source whose filesystem invents inode numbers per mount (FAT,
//! exFAT), and FAT's whole-hour time shifts. A real vfat or exFAT mount needs root, so the file layer's probe is
//! forced and a remount is simulated with the instrumentation's seams (new inode numbers, shifted times).

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use ps_model::{IdStability, SourceId, Stage};
use ps_scan::ScanOptions;

const HOUR_NS: i64 = 3_600_000_000_000;

/// New inode numbers, as a remount of a FAT or exFAT card hands out.
fn remounted(ino: u64) -> u64 {
    ino.wrapping_add(1 << 40)
}

struct Card {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: Env,
    id: SourceId,
}

fn jpeg(seed: u64) -> Vec<u8> {
    ps_fixtures::media::jpeg_with_exif(
        800,
        600,
        seed,
        88,
        &ps_fixtures::media::ExifSpec::canon_5d(),
    )
}

/// A small card (`DCIM/100CANON`, `DCIM/101CANON`) whose filesystem the probe reports as `ids` (FAT local time
/// when `fat`), added and scanned once.
fn card(ids: IdStability, fat: bool) -> Card {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("EOS_DIGITAL");
    for (i, dir) in [
        "DCIM/100CANON",
        "DCIM/100CANON",
        "DCIM/100CANON",
        "DCIM/101CANON",
    ]
    .iter()
    .enumerate()
    {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        let p = root.join(format!("{dir}/IMG_{:04}.JPG", 100 + i));
        ps_fixtures::scratch::write(&p, jpeg(500 + i as u64)).expect("write");
    }
    let env = env_with(ScanOptions::default());
    env.instr.force_ids(ids);
    env.instr.force_fat_time(fat);
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    env.instr.reset();
    Card {
        _tmp: tmp,
        root,
        env,
        id,
    }
}

fn paths_opened(env: &Env) -> BTreeSet<PathBuf> {
    env.instr.opens().into_iter().map(|o| o.path).collect()
}

#[test]
fn scenario_incremental_rescan_remounted_fat_card_reads_nothing() {
    let c = card(IdStability::Unstable, false);
    let files = c.env.files(c.id);
    assert_eq!(files.len(), 4);
    assert!(
        files.values().all(|r| r.stat.file_index.is_none()),
        "no file index stored where the filesystem does not keep one"
    );
    c.env.instr.remap_file_index(remounted);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!(c.env.instr.total_bytes(), 0, "zero bytes read");
    assert_eq!(c.env.instr.open_count(), 0, "no file opened");
    assert_eq!(
        (s.diff.new, s.diff.changed, s.diff.missing),
        (0, 0, 0),
        "{:?}",
        s.diff
    );
    assert_eq!(s.diff.unchanged, 4);
    assert_eq!(s.processed, 0);
}

#[test]
fn scenario_incremental_rescan_new_inode_numbers_still_mean_changed_where_ids_are_stable() {
    if cfg!(windows) {
        // Windows collects no file index on any filesystem (ADR-0112): there is no number to change.
        return;
    }
    let c = card(IdStability::Stable, false);
    let before = c.env.files(c.id);
    assert!(before.values().all(|r| r.stat.file_index.is_some()));
    c.env.instr.remap_file_index(remounted);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!(
        s.diff.changed, 4,
        "every file with a new inode number: {:?}",
        s.diff
    );
    assert_eq!(s.diff.unchanged, 0);
    let after = c.env.files(c.id);
    for (p, r) in &after {
        assert_eq!(
            r.stat.file_index,
            before[p].stat.file_index.map(remounted),
            "{p}: the new number is stored"
        );
        assert_eq!(r.stage, Stage::Thumbed, "{p}: analysed again");
        assert_eq!(r.identity, before[p].identity, "{p}: same content");
    }
}

#[test]
fn scenario_incremental_rescan_first_rescan_after_upgrading_reads_nothing() {
    // An older version stored the invented inode numbers (it treated every filesystem as keeping them).
    let mut c = card(IdStability::Stable, false);
    let before = c.env.files(c.id);
    if cfg!(unix) {
        assert!(before.values().all(|r| r.stat.file_index.is_some()));
    }
    // This version knows better, and the card was remounted meanwhile.
    c.env.reopen(ScanOptions::default());
    c.env.instr.force_ids(IdStability::Unstable);
    c.env.instr.remap_file_index(remounted);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!(c.env.instr.total_bytes(), 0, "zero bytes read");
    assert_eq!(c.env.instr.open_count(), 0);
    assert_eq!((s.diff.changed, s.diff.unchanged), (0, 4), "{:?}", s.diff);
    let after = c.env.files(c.id);
    for (p, r) in &after {
        assert_eq!(r.stat.file_index, None, "{p}: the stale inode is cleared");
        assert_eq!(r.stat.size, before[p].stat.size);
        assert_eq!(r.stat.mtime_ns, before[p].stat.mtime_ns);
        assert_eq!(r.stage, before[p].stage, "{p}: results kept");
        assert_eq!(r.identity, before[p].identity);
        assert_eq!(r.id, before[p].id);
    }
    // And the next rescan writes nothing more.
    c.env.instr.reset();
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!((c.env.instr.open_count(), s.diff.changed), (0, 0));
}

#[test]
fn scenario_incremental_rescan_renamed_file_on_fat_is_detected() {
    let c = card(IdStability::Unstable, false);
    let from = "DCIM/100CANON/IMG_0101.JPG";
    let to = "DCIM/101CANON/IMG_0901.JPG";
    let rec = c.env.files(c.id)[from].clone();
    ps_fixtures::scratch::rename(c.root.join(from), c.root.join(to)).expect("rename");
    c.env.instr.remap_file_index(remounted);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!(s.diff.moved, 1, "{:?}", s.diff);
    assert_eq!(s.diff.missing, 0, "the old path is not reported missing");
    assert_eq!(s.diff.changed, 0, "{:?}", s.diff);
    let files = c.env.files(c.id);
    assert!(!files.contains_key(from), "no row left at the old path");
    let moved = &files[to];
    assert_eq!(moved.id, rec.id, "the original file's id");
    assert_eq!(moved.first_seen_ms, rec.first_seen_ms);
    assert_eq!(moved.identity, rec.identity);
    assert_eq!(moved.stage, Stage::Thumbed);
    let root = c.env.root(c.id);
    assert_eq!(
        paths_opened(&c.env),
        BTreeSet::from([Path::new(to).iter().fold(root, |p, c| p.join(c))]),
        "only the renamed file is read"
    );
}

#[test]
fn scenario_incremental_rescan_replaced_file_on_fat_is_detected() {
    let c = card(IdStability::Unstable, false);
    let before = c.env.files(c.id);
    // Overwritten with other content: a different size, and the same size with a new (2 s later) time.
    let other = "DCIM/100CANON/IMG_0100.JPG";
    let same_size = "DCIM/101CANON/IMG_0103.JPG";
    ps_fixtures::scratch::write(c.root.join(other), jpeg(9_001)).expect("write");
    let mut bytes = std::fs::read(c.root.join(same_size)).expect("read");
    let n = bytes.len();
    bytes[n - 3] ^= 0xff;
    ps_fixtures::scratch::write(c.root.join(same_size), &bytes).expect("write");
    let t = std::time::UNIX_EPOCH
        + Duration::from_nanos(before[same_size].stat.mtime_ns as u64)
        + Duration::from_secs(2);
    ps_fixtures::scratch::set_mtime(c.root.join(same_size), t).expect("mtime");
    assert_ne!(
        std::fs::metadata(c.root.join(other)).expect("stat").len(),
        before[other].stat.size
    );
    c.env.instr.remap_file_index(remounted);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!((s.diff.changed, s.diff.unchanged), (2, 2), "{:?}", s.diff);
    let after = c.env.files(c.id);
    for p in [other, same_size] {
        assert_ne!(
            after[p].identity, before[p].identity,
            "{p}: identity updated"
        );
        assert_eq!(after[p].stage, Stage::Thumbed, "{p}");
        assert_eq!(after[p].id, before[p].id, "{p}: same row");
    }
}

#[test]
fn scenario_incremental_rescan_fat_time_zone_shift_is_not_a_change() {
    let c = card(IdStability::Unstable, true);
    let before = c.env.files(c.id);
    c.env.instr.remap_file_index(remounted);
    c.env.instr.shift_mtime(HOUR_NS);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!(c.env.instr.total_bytes(), 0, "zero bytes read");
    assert_eq!(c.env.instr.open_count(), 0);
    assert_eq!((s.diff.changed, s.diff.unchanged), (0, 4), "{:?}", s.diff);
    let after = c.env.files(c.id);
    for (p, r) in &after {
        assert_eq!(
            r.stat.mtime_ns,
            before[p].stat.mtime_ns + HOUR_NS,
            "{p}: the new time is stored"
        );
        assert_eq!(r.stage, before[p].stage, "{p}: results kept");
        assert_eq!(r.identity, before[p].identity);
    }
    // Shifted back (the zone changed again): still nothing read.
    c.env.instr.reset();
    c.env.instr.shift_mtime(-HOUR_NS);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!((c.env.instr.open_count(), s.diff.changed), (0, 0));
    let back = c.env.files(c.id);
    assert!(
        back.iter()
            .all(|(p, r)| r.stat.mtime_ns == before[p].stat.mtime_ns - HOUR_NS)
    );
    // A card written at UTC-12 and read at UTC+14: 26 hours, still nothing read.
    c.env.instr.reset();
    c.env.instr.shift_mtime(25 * HOUR_NS);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!((c.env.instr.open_count(), s.diff.changed), (0, 0));
}

#[test]
fn scenario_incremental_rescan_other_shifts_are_changes() {
    // 30 minutes on FAT.
    let c = card(IdStability::Unstable, true);
    c.env.instr.shift_mtime(HOUR_NS / 2);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!((s.diff.changed, s.diff.unchanged), (4, 0), "{:?}", s.diff);
    // One hour on exFAT, which stores a UTC offset.
    let c = card(IdStability::Unstable, false);
    c.env.instr.shift_mtime(HOUR_NS);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!((s.diff.changed, s.diff.unchanged), (4, 0), "{:?}", s.diff);
    // 27 hours on FAT: beyond any zone change (UTC-12 to UTC+14 is 26 h).
    let c = card(IdStability::Unstable, true);
    c.env.instr.shift_mtime(27 * HOUR_NS);
    let s = c.env.scan(&[c.id]);
    completed(&s);
    assert_eq!(s.diff.changed, 4, "{:?}", s.diff);
}

#[test]
fn scenario_sources_exfat_source_shows_unstable_file_ids() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("card");
    std::fs::create_dir_all(&root).expect("mkdir");
    ps_fixtures::scratch::write(root.join("IMG_0001.JPG"), jpeg(1)).expect("write");
    let env = env();
    env.instr.force_ids(IdStability::Unstable);
    let added = env.s().add_source(&root).expect("add").source;
    assert_eq!(added.stable_file_ids, Some(false));
    let listed = env.s().refresh_sources().expect("sources");
    assert_eq!(
        listed[0].stable_file_ids,
        Some(false),
        "sources list shows it"
    );
    let hint = env
        .readers()
        .sources()
        .expect("records")
        .into_iter()
        .find(|s| s.id == added.id)
        .and_then(|s| s.device_hint)
        .expect("hint");
    assert!(hint.ends_with(";ids=unstable"), "{hint}");

    // The scan's volume check records a change (a source added before ADR-0112, or a re-formatted drive).
    env.instr.force_ids(IdStability::Stable);
    completed(&env.scan(&[added.id]));
    let listed = env.s().sources().expect("sources");
    assert_eq!(listed[0].stable_file_ids, Some(true));

    // Unforced, this machine's temp folder keeps file ids (Windows records none).
    let other = tempfile::tempdir().expect("tmp");
    let env = self::env();
    let s = env.s().add_source(other.path()).expect("add").source;
    let want = if cfg!(unix) { Some(true) } else { None };
    assert_eq!(s.stable_file_ids, want);
}

#[test]
fn library_search_probes_each_device_once() {
    use ps_scan::walk::{LIBRARY_MANIFEST_FILE, LIBRARY_META_DIR, find_export_library};
    let dir = tempfile::tempdir().unwrap();
    for d in ["a", "a/b", "c", "c/d/e"] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
        ps_fixtures::scratch::write(dir.path().join(d).join("x.jpg"), b"x").unwrap();
    }
    let (fs, instr) = ps_scan::SourceFs::instrumented();
    assert_eq!(find_export_library(&fs, dir.path()), None);
    assert!(instr.read_dirs() >= 5, "{}", instr.read_dirs());
    assert_eq!(instr.probe_count(), 1, "one statfs for the one device");
    // A library deeper down is still found.
    let meta = dir.path().join("c/d").join(LIBRARY_META_DIR);
    std::fs::create_dir_all(&meta).unwrap();
    ps_fixtures::scratch::write(meta.join(LIBRARY_MANIFEST_FILE), b"").unwrap();
    assert_eq!(
        find_export_library(&fs, dir.path()),
        Some(dir.path().join("c/d"))
    );
}
