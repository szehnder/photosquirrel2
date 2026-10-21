//! Failure records and the retry limit (scanning task 3.6), and progress snapshots (task 3.7).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::*;
use ps_model::{ProgressSnapshot, SourceId, Stage, StageCounts};

fn chmod(p: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }
    #[cfg(not(unix))]
    let _ = (p, mode);
}

/// drive-c copied with one photo made unreadable.
fn with_unreadable() -> (tempfile::TempDir, PathBuf, String) {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("drive-c");
    copy_tree(&corpus().root.join("drive-c"), &root);
    let (rel, _) = truth_under(corpus(), "drive-c")
        .find(|(_, f)| f.expected_class.is_candidate() && f.format == "jpeg")
        .expect("a photo");
    chmod(&root.join(&rel), 0o000);
    (tmp, root, rel)
}

#[cfg(unix)]
#[test]
fn scenario_scanning_unreadable_file_recorded() {
    let _cores = shared();
    let (_tmp, root, rel) = with_unreadable();
    let e = env();
    let id = e.add(&root);
    let summary = e.scan(&[id]);
    chmod(&root.join(&rel), 0o644);
    completed(&summary);
    let files = e.files(id);
    let f = &files[&rel];
    assert_eq!(f.stage, Stage::Failed);
    assert_eq!(f.attempts, 1);
    let errors = e.readers().file_errors(f.id).expect("errors");
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].message.contains("permission denied"),
        "{}",
        errors[0].message
    );
    assert_eq!(summary.errors, 1);
    for (p, r) in &files {
        if *p != rel {
            assert!(
                matches!(r.stage, Stage::Thumbed | Stage::Skipped),
                "{p} completes ({:?})",
                r.stage
            );
        }
    }
    let snap = e.s().snapshot().expect("snapshot");
    assert_eq!(snap.recent_errors.len(), 1);
    assert_eq!(snap.recent_errors[0].path.display(), rel);
}

#[cfg(unix)]
#[test]
fn scenario_scanning_failed_retried_at_most_twice() {
    let _cores = shared();
    let (_tmp, root, rel) = with_unreadable();
    let e = env();
    let id = e.add(&root);
    let path = e.root(id).join(&rel);
    let mut attempted = Vec::new();
    for _run in 0..4 {
        e.instr.reset();
        completed(&e.scan(&[id]));
        attempted.push(e.instr.opens().iter().any(|o| o.path == path));
    }
    chmod(&path, 0o644);
    assert_eq!(
        attempted,
        vec![true, true, true, false],
        "attempted in the first three runs only"
    );
    let f = &e.files(id)[&rel];
    assert_eq!(f.stage, Stage::Failed, "stays failed");
    assert_eq!(f.attempts, 3);
    let errors = e.readers().file_errors(f.id).expect("errors");
    let runs: Vec<_> = errors.iter().map(|x| x.run).collect();
    let all: Vec<_> = e
        .s()
        .runs()
        .expect("runs")
        .iter()
        .map(|r| Some(r.id))
        .collect();
    assert_eq!(runs, all[..3].to_vec());
    assert_eq!(
        e.s()
            .summary(e.s().runs().expect("runs")[3].id)
            .expect("summary")
            .expect("stored")
            .retries_exhausted,
        1
    );
}

/// Per-stage counts of the present files of `sources`, from the catalog.
fn catalog_stages(e: &Env, sources: &[SourceId]) -> BTreeMap<SourceId, StageCounts> {
    sources
        .iter()
        .map(|id| {
            let mut c = StageCounts::default();
            for f in e
                .files(*id)
                .values()
                .filter(|f| f.missing_since_ms.is_none())
            {
                *c.get_mut(f.stage) += 1;
            }
            (*id, c)
        })
        .collect()
}

#[test]
fn scenario_scanning_snapshot_counts_match_catalog() {
    let _cores = shared();
    let c = corpus();
    let e = env();
    let ids: Vec<_> = c
        .truth
        .drives
        .iter()
        .map(|d| e.add(&c.root.join(d)))
        .collect();
    completed(&e.scan(&[]));
    let snap = e.s().snapshot().expect("snapshot");
    let cat = catalog_stages(&e, &ids);
    let mut total = StageCounts::default();
    for p in &snap.sources {
        assert_eq!(p.stages, cat[&p.source], "source {}", p.source);
        total.add(&cat[&p.source]);
    }
    assert_eq!(snap.stages, total);
    assert_eq!(snap.stages.total(), c.truth.files.len() as u64);
    assert_eq!(snap.processed, snap.to_process);
    assert!(snap.bytes_read > 0);
    // A rescan's snapshot counts unchanged files at their stored stage.
    completed(&e.scan(&[]));
    let again = e.s().snapshot().expect("snapshot");
    assert_eq!(again.stages, total);
    assert_eq!(again.processed, 0);
}

#[cfg(unix)]
#[test]
fn scenario_scanning_recent_lists_bounded() {
    let _cores = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("many");
    std::fs::create_dir_all(root.join("photos")).expect("mkdir");
    std::fs::create_dir_all(root.join("locked")).expect("mkdir");
    for i in 0..500u64 {
        let jpeg = ps_fixtures::media::jpeg_with_exif(
            320,
            240,
            i,
            80,
            &ps_fixtures::media::ExifSpec::canon_5d(),
        );
        std::fs::write(root.join(format!("photos/p{i:03}.jpg")), &jpeg).expect("write");
        if i < 100 {
            let p = root.join(format!("locked/l{i:03}.jpg"));
            std::fs::write(&p, &jpeg).expect("write");
            chmod(&p, 0o000);
        }
    }
    let e = env();
    let id = e.add(&root);
    let summary = e.scan(&[id]);
    for i in 0..100u64 {
        chmod(&root.join(format!("locked/l{i:03}.jpg")), 0o644);
    }
    completed(&summary);
    assert_eq!(summary.stages.thumbed, 500);
    assert_eq!(summary.stages.failed, 100);
    let snap: ProgressSnapshot = e.s().snapshot().expect("snapshot");
    assert_eq!(snap.recent_photos.len(), ProgressSnapshot::RECENT_PHOTOS);
    assert_eq!(snap.recent_errors.len(), ProgressSnapshot::RECENT_ERRORS);
    assert_eq!(snap.errors, 100);
    let photos: std::collections::BTreeSet<_> =
        snap.recent_photos.iter().map(|p| p.path.clone()).collect();
    assert_eq!(photos.len(), 64, "64 distinct photos");
    let errors: std::collections::BTreeSet<_> =
        snap.recent_errors.iter().map(|p| p.path.clone()).collect();
    assert_eq!(errors.len(), 32, "32 distinct errors");
    assert!(
        snap.recent_errors
            .iter()
            .all(|x| x.message.contains("permission denied"))
    );
    let files = e.files(id);
    for p in &snap.recent_photos {
        assert_eq!(files[&p.path.display().into_owned()].stage, Stage::Thumbed);
    }
}

/// ADR-0055: a truncated or corrupt JPEG is recorded as an analysis failure, never hashed with grey padding
/// (where a damaged full-size copy could group with its intact derivatives and win keeper on pixel count).
#[test]
fn truncated_and_corrupt_jpegs_are_failed_not_hashed() {
    use ps_fixtures::media as fx;
    let _cores = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("card");
    std::fs::create_dir_all(&root).expect("mkdir");
    let full = fx::jpeg(2400, 1800, 21, 90);
    std::fs::write(root.join("intact.jpg"), &full).expect("write");
    std::fs::write(
        root.join("truncated.jpg"),
        fx::truncate(&full, full.len() * 99 / 100),
    )
    .expect("write");
    let sos = full
        .windows(2)
        .position(|w| w == [0xFF, 0xDA])
        .expect("SOS");
    let mut garbage = full[..sos + 14].to_vec();
    garbage.extend_from_slice(&fx::random_bytes(5 * 1024, 4));
    std::fs::write(root.join("garbage.jpg"), garbage).expect("write");
    let e = env();
    let id = e.add(&root);
    completed(&e.scan(&[id]));
    let files = e.files(id);
    let intact = &files["intact.jpg"];
    assert_eq!(intact.stage, Stage::Thumbed);
    assert!(intact.hashes.is_some(), "the intact copy is hashed");
    for name in ["truncated.jpg", "garbage.jpg"] {
        let f = &files[name];
        assert_eq!(f.stage, Stage::Failed, "{name}");
        assert!(f.hashes.is_none(), "{name} is not hashed");
        let errors = e.readers().file_errors(f.id).expect("errors");
        assert!(
            errors
                .first()
                .is_some_and(|x| x.message.contains("analysis failed")),
            "{name}: {errors:?}"
        );
    }
}
