//! `sources` scenarios (scanning tasks 2.1-2.6): add, fingerprint, relative paths, offline + re-link,
//! overlap folding, removal. Sources are only ever read.

mod common;

use std::path::Path;

use common::*;
use ps_model::{SourceStatus, Stage};
use ps_scan::{Confirmation, ScanError};

/// A small writable copy of part of a drive: `<tmp>/<name>/{Backup 2015, Backup 2016}` from drive-b.
fn small_drive(tmp: &Path, name: &str) -> std::path::PathBuf {
    let c = corpus();
    let root = tmp.join(name);
    for sub in ["Backup 2015", "Backup 2016"] {
        copy_tree(&c.root.join("drive-b").join(sub), &root.join(sub));
    }
    root
}

#[test]
fn scenario_sources_add_a_source() {
    let c = corpus();
    let e = env();
    let root = c.root.join("drive-a");
    let added = e.s().add_source(&root).expect("add");
    assert!(added.folded.is_empty());
    let list = e.s().sources().expect("sources");
    assert_eq!(list.len(), 1);
    let s = &list[0];
    assert_eq!(s.root, std::fs::canonicalize(&root).expect("canonical"));
    assert_eq!(s.status, SourceStatus::Online);
    let total: u64 = truth_under(c, "drive-a").map(|(_, f)| f.size).sum();
    assert_eq!(s.total_bytes, Some(total));
    assert!(
        s.fingerprint_samples > 0 && s.fingerprint_samples <= 64,
        "{} samples",
        s.fingerprint_samples
    );
}

#[test]
fn scenario_sources_missing_folder_rejected() {
    let e = env();
    let tmp = tempfile::tempdir().expect("tmp");
    let missing = tmp.path().join("no such folder");
    let err = e.s().add_source(&missing).expect_err("must fail");
    assert!(matches!(err, ScanError::SourceMissing(_)), "{err:?}");
    assert!(
        err.to_string().contains(&missing.display().to_string()),
        "the error names the path: {err}"
    );
    assert!(e.s().sources().expect("sources").is_empty());
}

#[test]
fn scenario_sources_adding_writes_nothing_to_source() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = small_drive(tmp.path(), "drive");
    let before = snapshot_tree(&root);
    let e = env();
    let id = e.add(&root);
    assert_eq!(snapshot_tree(&root), before, "adding and fingerprinting");
    // Re-linking and removing read only as well.
    let moved = tmp.path().join("drive-moved");
    std::fs::rename(&root, &moved).expect("move drive");
    e.s().refresh_sources().expect("refresh");
    let before_moved = snapshot_tree(&moved);
    e.s().relink_source(id, &moved).expect("relink");
    e.s()
        .remove_source(id, Confirmation::Confirmed)
        .expect("remove");
    assert_eq!(
        snapshot_tree(&moved),
        before_moved,
        "re-linking and removing"
    );
    assert!(e.instr.opens().iter().all(|o| !o.access.writes()));
}

#[test]
fn scenario_sources_paths_are_relative() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("mnt/usb1/Photos");
    std::fs::create_dir_all(root.join("2010")).expect("mkdir");
    let jpeg = ps_fixtures::media::jpeg_with_exif(
        640,
        480,
        1,
        90,
        &ps_fixtures::media::ExifSpec::canon_5d(),
    );
    std::fs::write(root.join("2010/a.jpg"), &jpeg).expect("write");
    let e = env();
    let id = e.add(&root);
    completed(&e.scan(&[id]));
    let files = e.files(id);
    assert_eq!(files.keys().collect::<Vec<_>>(), vec!["2010/a.jpg"]);
    let f = &files["2010/a.jpg"];
    assert_eq!(f.key.source, id);
    assert_eq!(f.key.path.as_bytes(), b"2010/a.jpg");
    assert_eq!(f.stage, Stage::Thumbed);
}

#[test]
fn scenario_sources_remounted_drive_relinked() {
    let tmp = tempfile::tempdir().expect("tmp");
    std::fs::create_dir_all(tmp.path().join("usb1")).expect("mkdir");
    let root = small_drive(&tmp.path().join("usb1"), "Photos");
    let e = env();
    let id = e.add(&root);
    completed(&e.scan(&[id]));
    let before = e.files(id);
    // The drive comes back at another mount point.
    std::fs::rename(tmp.path().join("usb1"), tmp.path().join("usb2")).expect("remount");
    let list = e.s().refresh_sources().expect("refresh");
    assert_eq!(list[0].status, SourceStatus::Offline);
    let new_root = tmp.path().join("usb2/Photos");
    let s = e.s().relink_source(id, &new_root).expect("relink");
    assert_eq!(s.id, id);
    assert_eq!(s.status, SourceStatus::Online);
    assert_eq!(s.root, std::fs::canonicalize(&new_root).expect("canonical"));
    e.instr.reset();
    let summary = e.scan(&[id]);
    completed(&summary);
    assert_eq!(summary.diff.new, 0, "{:?}", summary.diff);
    assert_eq!(summary.diff.changed, 0, "{:?}", summary.diff);
    assert_eq!(summary.diff.unchanged as usize, before.len());
    assert_eq!(e.instr.total_bytes(), 0, "nothing re-read");
    let after = e.files(id);
    for (rel, f) in &before {
        assert_eq!(after[rel].id, f.id, "{rel} keeps its history");
        assert_eq!(normalized(&after[rel]), normalized(f));
    }
}

#[test]
fn scenario_sources_wrong_drive_rejected() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = small_drive(tmp.path(), "drive");
    let e = env();
    let id = e.add(&root);
    completed(&e.scan(&[id]));
    std::fs::rename(&root, tmp.path().join("unplugged")).expect("unplug");
    e.s().refresh_sources().expect("refresh");
    // Another drive entirely.
    let other = corpus().root.join("drive-c");
    let err = e.s().relink_source(id, &other).expect_err("wrong drive");
    match &err {
        ScanError::FingerprintMismatch {
            mismatched,
            samples,
            ..
        } => {
            assert!(*mismatched > 0 && *mismatched <= *samples, "{err}");
        }
        other => panic!("expected a fingerprint mismatch, got {other:?}"),
    }
    assert!(err.to_string().contains("do not match"), "{err}");
    let s = &e.s().sources().expect("sources")[0];
    assert_eq!(s.status, SourceStatus::Offline);
    assert_eq!(
        s.root,
        std::fs::canonicalize(tmp.path()).expect("c").join("drive")
    );
}

#[test]
fn scenario_sources_unplugged_source_is_offline() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = small_drive(tmp.path(), "drive");
    let e = env();
    let id = e.add(&root);
    completed(&e.scan(&[id]));
    let before = e.files(id);
    std::fs::rename(&root, tmp.path().join("elsewhere")).expect("unplug");
    let list = e.s().refresh_sources().expect("refresh");
    assert_eq!(list[0].status, SourceStatus::Offline);
    let after = e.files(id);
    assert_eq!(after.len(), before.len(), "files are still listed");
    let mut thumbs = 0;
    for f in after.values() {
        assert!(f.missing_since_ms.is_none(), "offline is not missing");
        if f.stage == Stage::Thumbed {
            let t = e
                .s()
                .thumbs()
                .get(&f.identity.expect("identity"))
                .expect("get");
            assert!(t.is_some(), "thumbnail still available");
            thumbs += 1;
        }
    }
    assert!(thumbs > 0);
    // A scan of the offline source does not mark its files missing.
    let summary = e.scan(&[id]);
    assert_eq!(summary.incomplete_sources, vec![id]);
    assert!(e.files(id).values().all(|f| f.missing_since_ms.is_none()));
}

#[test]
fn scenario_sources_inner_source_folded_into_outer() {
    let tmp = tempfile::tempdir().expect("tmp");
    let outer = small_drive(tmp.path(), "drive-b");
    let inner = outer.join("Backup 2015");
    let e = env();
    let inner_id = e.add(&inner);
    completed(&e.scan(&[inner_id]));
    let inner_files = e.files(inner_id);
    assert!(!inner_files.is_empty());

    let added = e.s().add_source(&outer).expect("add outer");
    assert_eq!(added.folded, vec![inner_id]);
    let list = e.s().sources().expect("sources");
    assert_eq!(list.len(), 1, "one source remains");
    let outer_id = list[0].id;
    let folded = e.files(outer_id);
    assert_eq!(folded.len(), inner_files.len());
    for (rel, f) in &inner_files {
        let g = &folded[&format!("Backup 2015/{rel}")];
        assert_eq!(g.id, f.id, "{rel} keeps its id");
        assert_eq!(
            normalized(g),
            normalized(f),
            "{rel} keeps stage and results"
        );
    }
    // Scanning the outer source analyzes the rest and leaves the folded files alone.
    e.instr.reset();
    let summary = e.scan(&[outer_id]);
    completed(&summary);
    assert_eq!(summary.diff.unchanged as usize, inner_files.len());
    let after = e.files(outer_id);
    let bytes = e.instr.bytes_per_file();
    for (rel, f) in &inner_files {
        let key = format!("Backup 2015/{rel}");
        assert_eq!(after[&key].id, f.id);
        assert!(!bytes.contains_key(&outer.join(&key)), "{key} not re-read");
    }
    assert_eq!(
        after.len(),
        after
            .keys()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    assert!(after.values().all(|f| f.stage.is_terminal()));
}

#[test]
fn scenario_sources_inner_of_existing_reported() {
    let c = corpus();
    let e = env();
    let outer = e.add(&c.root.join("drive-b"));
    let err = e
        .s()
        .add_source(&c.root.join("drive-b/Backup 2015"))
        .expect_err("covered");
    match &err {
        ScanError::AlreadyCovered { by, .. } => assert_eq!(*by, outer),
        other => panic!("expected AlreadyCovered, got {other:?}"),
    }
    assert!(err.to_string().contains("already covered by"), "{err}");
    assert!(err.to_string().contains("drive-b"), "{err}");
    assert_eq!(e.s().sources().expect("sources").len(), 1);
}

#[test]
fn scenario_sources_remove_keeps_files_on_disk() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = small_drive(tmp.path(), "drive");
    let e = env();
    let id = e.add(&root);
    completed(&e.scan(&[id]));
    let before = snapshot_tree(&root);
    assert!(matches!(
        e.s().remove_source(id, Confirmation::NotConfirmed),
        Err(ScanError::NotConfirmed)
    ));
    assert_eq!(e.s().sources().expect("sources").len(), 1);
    e.s()
        .remove_source(id, Confirmation::Confirmed)
        .expect("remove");
    assert!(e.s().sources().expect("sources").is_empty());
    assert!(e.files(id).is_empty(), "catalog records are gone");
    assert_eq!(
        snapshot_tree(&root),
        before,
        "the tree on disk is unchanged"
    );
}
