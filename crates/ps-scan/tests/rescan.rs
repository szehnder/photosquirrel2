//! `incremental-rescan` scenarios (scanning tasks 4.1-4.5): the stat-fingerprint diff, re-analysis of
//! changed files, missing and returning files, moves, and hash upgrades from the thumbnail store.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use ps_model::{PhotoClass, ScanPhase, SourceId, Stage};
use ps_scan::ScanOptions;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: Env,
    id: SourceId,
}

/// drive-c of the corpus copied to a temp folder, added and scanned once.
fn scanned(opts: ScanOptions) -> Fixture {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("drive-c");
    copy_tree(&corpus().root.join("drive-c"), &root);
    let env = env_with(opts);
    let id = env.add(&root);
    completed(&env.scan(&[id]));
    env.instr.reset();
    Fixture {
        _tmp: tmp,
        root,
        env,
        id,
    }
}

/// Thumbed photos of the fixture, by path (sorted).
fn photos(f: &Fixture) -> Vec<String> {
    f.env
        .files(f.id)
        .into_iter()
        .filter(|(_, r)| r.stage == Stage::Thumbed && r.class == Some(PhotoClass::Photo))
        .map(|(p, _)| p)
        .collect()
}

fn new_jpeg(seed: u64) -> Vec<u8> {
    ps_fixtures::media::jpeg_with_exif(
        800,
        600,
        seed,
        88,
        &ps_fixtures::media::ExifSpec::canon_5d(),
    )
}

fn write(path: &Path, bytes: &[u8]) {
    ps_fixtures::scratch::write(path, bytes).expect("write");
}

#[test]
fn scenario_incremental_rescan_diff_counts_before_analysis() {
    let f = scanned(ScanOptions::default());
    let total = f.env.files(f.id).len() as u64;
    let p = photos(&f);
    std::fs::create_dir_all(f.root.join("New Photos")).expect("mkdir");
    for i in 0..3 {
        write(
            &f.root.join(format!("New Photos/new-{i}.jpg")),
            &new_jpeg(1000 + i),
        );
    }
    for (i, rel) in p.iter().take(2).enumerate() {
        write(&f.root.join(rel), &new_jpeg(2000 + i as u64));
    }
    ps_fixtures::scratch::remove(f.root.join(&p[2])).expect("delete");

    // Slow readers so the diff is observable before any file is opened.
    let mut env = f.env;
    env.reopen(ScanOptions {
        read_delay: Some(Duration::from_millis(300)),
        ..ScanOptions::default()
    });
    env.s().start_scan(&[f.id]).expect("start");
    let snap = wait_snapshot(env.s(), Duration::from_secs(60), "the diff", |s| {
        s.phase != ScanPhase::Starting && s.phase != ScanPhase::Walking
    });
    assert_eq!(env.instr.open_count(), 0, "no file opened before the diff");
    assert_eq!(snap.bytes_read, 0);
    let d = snap.diff;
    assert_eq!((d.new, d.changed, d.missing), (3, 2, 1), "{d:?}");
    assert_eq!(d.unchanged, total - 3);
    let summary = env.s().wait().expect("summary");
    completed(&summary);
    assert_eq!(summary.diff, d, "the run reports the same diff");
    assert_eq!(
        summary.processed, 5,
        "only new and changed files are processed"
    );
}

#[test]
fn scenario_incremental_rescan_unchanged_files_not_read() {
    let f = scanned(ScanOptions::default());
    let summary = f.env.scan(&[f.id]);
    completed(&summary);
    assert_eq!(f.env.instr.total_bytes(), 0, "zero bytes of content read");
    assert_eq!(f.env.instr.open_count(), 0, "no file opened");
    assert_eq!(
        summary.diff.new + summary.diff.changed + summary.diff.missing,
        0
    );
    assert_eq!(summary.processed, 0);
}

#[test]
fn scenario_incremental_rescan_edited_file_re_analyzed() {
    let f = scanned(ScanOptions::default());
    let before = f.env.files(f.id);
    let rel = photos(&f).into_iter().next().expect("a photo");
    let old = before[&rel].clone();
    let bytes = new_jpeg(77);
    write(&f.root.join(&rel), &bytes);
    let summary = f.env.scan(&[f.id]);
    completed(&summary);
    assert_eq!(summary.diff.changed, 1);
    let after = f.env.files(f.id);
    let new = &after[&rel];
    assert_eq!(new.id, old.id, "same catalog row");
    assert_eq!(new.stage, Stage::Thumbed);
    let id = new.identity.expect("identity");
    assert_eq!(id.to_hex(), blake3::hash(&bytes).to_hex().to_string());
    assert_ne!(new.identity, old.identity, "identity updated");
    assert_ne!(new.hashes, old.hashes, "hashes updated");
    assert_eq!(
        (new.width, new.height),
        (Some(800), Some(600)),
        "metadata updated"
    );
    let t = f
        .env
        .s()
        .thumbs()
        .get(&id)
        .expect("get")
        .expect("new thumbnail");
    assert!(!t.jpeg.is_empty());
    for (p, r) in &before {
        if *p != rel {
            assert_eq!(normalized(&after[p]), normalized(r), "{p} untouched");
            assert_eq!(after[p].id, r.id);
        }
    }
    assert_eq!(
        f.env.instr.bytes_per_file().len(),
        1,
        "only the edited file was read"
    );
}

#[test]
fn scenario_incremental_rescan_missing_file_kept_in_history() {
    let f = scanned(ScanOptions::default());
    let rel = photos(&f).into_iter().next().expect("a photo");
    let rec = f.env.files(f.id)[&rel].clone();
    ps_fixtures::scratch::remove(f.root.join(&rel)).expect("delete");
    let summary = f.env.scan(&[f.id]);
    completed(&summary);
    assert_eq!(summary.diff.missing, 1);
    let now = &f.env.files(f.id)[&rel];
    assert!(now.missing_since_ms.is_some(), "marked missing with a time");
    assert_eq!(now.id, rec.id);
    assert_eq!(now.identity, rec.identity);
    assert!(
        f.env
            .readers()
            .metadata(rec.id)
            .expect("metadata")
            .is_some(),
        "metadata kept"
    );
    assert!(
        f.env
            .s()
            .thumbs()
            .get(&rec.identity.expect("id"))
            .expect("get")
            .is_some(),
        "thumbnail kept"
    );
    // Still missing (same time) after another rescan.
    let since = now.missing_since_ms;
    completed(&f.env.scan(&[f.id]));
    assert_eq!(f.env.files(f.id)[&rel].missing_since_ms, since);
}

#[test]
fn scenario_incremental_rescan_returning_file_restored() {
    let f = scanned(ScanOptions::default());
    let rel = photos(&f).into_iter().next().expect("a photo");
    let rec = f.env.files(f.id)[&rel].clone();
    let path = f.root.join(&rel);
    let saved = f.root.parent().expect("parent").join("saved.jpg");
    ps_fixtures::scratch::rename(&path, &saved).expect("take away");
    completed(&f.env.scan(&[f.id]));
    assert!(f.env.files(f.id)[&rel].missing_since_ms.is_some());
    // Restored with its original size and modification time.
    ps_fixtures::scratch::rename(&saved, &path).expect("restore");
    f.env.instr.reset();
    let summary = f.env.scan(&[f.id]);
    completed(&summary);
    assert_eq!(summary.diff.returning, 1);
    assert_eq!(summary.diff.missing, 0);
    let back = &f.env.files(f.id)[&rel];
    assert!(back.missing_since_ms.is_none(), "no longer missing");
    assert_eq!(normalized(back), normalized(&rec), "unchanged");
    assert_eq!(back.id, rec.id);
    assert_eq!(f.env.instr.open_count(), 0, "not read");
}

#[test]
fn scenario_incremental_rescan_moved_file_keeps_history() {
    let f = scanned(ScanOptions::default());
    let rel = photos(&f).into_iter().next().expect("a photo");
    let rec = f.env.files(f.id)[&rel].clone();
    std::fs::create_dir_all(f.root.join("Moved Here")).expect("mkdir");
    let to = "Moved Here/renamed.jpg";
    ps_fixtures::scratch::rename(f.root.join(&rel), f.root.join(to)).expect("move");
    let summary = f.env.scan(&[f.id]);
    completed(&summary);
    assert_eq!(summary.diff.moved, 1, "{:?}", summary.diff);
    assert_eq!(
        summary.diff.missing, 0,
        "the old path is not reported missing"
    );
    let files = f.env.files(f.id);
    assert!(!files.contains_key(&rel), "no row left at the old path");
    let moved = &files[to];
    assert_eq!(moved.id, rec.id, "original id");
    assert_eq!(
        moved.first_seen_ms, rec.first_seen_ms,
        "original first-seen time"
    );
    assert!(moved.first_seen_ms.is_some());
    assert!(moved.missing_since_ms.is_none());
    assert_eq!(moved.stage, Stage::Thumbed);
    assert_eq!(moved.identity, rec.identity);
    assert_eq!(
        files
            .values()
            .filter(|r| r.identity == rec.identity && r.missing_since_ms.is_none())
            .count(),
        files
            .values()
            .filter(|r| r.identity == rec.identity)
            .count(),
        "one live file per path"
    );
}

#[test]
fn scenario_incremental_rescan_hash_version_bump_uses_thumbnails() {
    let f = scanned(ScanOptions::default());
    let before: Vec<_> = f
        .env
        .files(f.id)
        .into_values()
        .filter(|r| r.stage == Stage::Thumbed && r.hashes.is_some())
        .collect();
    assert!(!before.is_empty());
    // The source goes offline; the upgrade must not need it.
    ps_fixtures::scratch::rename(&f.root, f.root.with_file_name("offline")).expect("unplug");
    f.env.s().refresh_sources().expect("refresh");
    f.env.instr.reset();
    let next = ps_hash::PERCEPTUAL_HASH_VERSION + 1;
    let report = ps_scan::upgrade_hashes(f.env.s().catalog(), f.env.s().thumbs(), next, |v| {
        let mut h = ps_hash::perceptual_hash(v);
        h.version = next;
        h
    })
    .expect("upgrade");
    assert_eq!(report.upgraded as usize, before.len(), "{report:?}");
    let after = f.env.files(f.id);
    for r in &before {
        let h = after[&r.key.path.display().into_owned()]
            .hashes
            .expect("hashes");
        assert_eq!(h.version, next, "{} upgraded", r.key.path);
    }
    assert_eq!(f.env.instr.open_count(), 0, "no source file opened");
    // Hashes from the 384 px thumbnail stay close to the 512 px analysis hashes.
    let close = before
        .iter()
        .filter(|r| {
            let a = r.hashes.expect("old");
            let b = after[&r.key.path.display().into_owned()]
                .hashes
                .expect("new");
            a.phash_distance(&b) <= 8
        })
        .count();
    assert!(
        close * 10 >= before.len() * 9,
        "{close} of {} close",
        before.len()
    );
}
