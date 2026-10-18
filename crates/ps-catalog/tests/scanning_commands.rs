//! Writer commands and queries added by the scanning change: first-seen times, moves, folding, removal,
//! re-linking, hash upgrades and error rows (migration 3).

mod common;

use common::*;
use ps_catalog::{Catalog, Command, FileFailure, FileKey};
use ps_model::{FileId, PerceptualHash, RunStatus, SourceStatus, Stage};

fn file(cat: &Catalog, src: ps_model::SourceId, p: &str) -> Option<ps_catalog::FileRecord> {
    cat.read().file(&FileKey::new(src, rel(p))).expect("query")
}

#[test]
fn first_seen_is_set_on_insert_and_kept_on_update() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    cat.writer()
        .upsert_files(vec![discovered_row(src, 1), full_row(src, 2)])
        .expect("write");
    cat.writer().flush().expect("flush");
    let a = cat
        .read()
        .file(&FileKey::new(src, path_of(1)))
        .expect("q")
        .expect("row");
    let b = cat
        .read()
        .file(&FileKey::new(src, path_of(2)))
        .expect("q")
        .expect("row");
    assert!(a.first_seen_ms.is_some() && b.first_seen_ms.is_some());
    let mut changed = discovered_row(src, 1);
    changed.stat.size += 1;
    cat.writer().upsert_files(vec![changed]).expect("write");
    cat.writer().flush().expect("flush");
    let a2 = cat
        .read()
        .file(&FileKey::new(src, path_of(1)))
        .expect("q")
        .expect("row");
    assert_eq!(a2.first_seen_ms, a.first_seen_ms);
}

#[test]
fn move_file_keeps_the_old_row() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let w = cat.writer();
    w.upsert_files(vec![full_row(src, 1)]).expect("w");
    w.submit(Command::MarkMissing {
        files: vec![FileKey::new(src, path_of(1))],
        at_ms: 5,
    })
    .expect("w");
    let mut new =
        ps_catalog::FileRow::discovered(FileKey::new(src, rel("moved/x.jpg")), stat_of(9));
    new.stat.mtime_ns = 77;
    w.upsert_files(vec![new]).expect("w");
    w.flush().expect("flush");
    let old = cat
        .read()
        .file(&FileKey::new(src, path_of(1)))
        .expect("q")
        .expect("old");
    w.submit(Command::MoveFile {
        from: FileKey::new(src, path_of(1)),
        to: FileKey::new(src, rel("moved/x.jpg")),
    })
    .expect("w");
    w.flush().expect("flush");
    assert!(
        cat.read()
            .file(&FileKey::new(src, path_of(1)))
            .expect("q")
            .is_none()
    );
    let m = file(&cat, src, "moved/x.jpg").expect("moved");
    assert_eq!(m.id, old.id);
    assert_eq!(m.first_seen_ms, old.first_seen_ms);
    assert_eq!(m.identity, old.identity);
    assert_eq!(m.stage, Stage::Analyzed);
    assert!(m.missing_since_ms.is_none());
    assert_eq!(m.stat.mtime_ns, 77, "takes the new path's fingerprint");
}

#[test]
fn fold_relink_remove_and_hashes() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cat = Catalog::open(tmp.path()).expect("open");
    let inner = add_source(&cat, "/mnt/a/inner");
    let outer = add_source(&cat, "/mnt/a");
    let w = cat.writer();
    w.upsert_files(vec![
        full_row(inner, 1),
        full_row(inner, 2),
        full_row(outer, 3),
    ])
    .expect("w");
    w.flush().expect("flush");
    let before = cat
        .read()
        .file(&FileKey::new(inner, path_of(1)))
        .expect("q")
        .expect("row");
    w.submit(Command::FoldSource {
        inner,
        outer,
        prefix: rel("inner"),
    })
    .expect("w");
    w.flush().expect("flush");
    let folded = cat
        .read()
        .file(&FileKey::new(outer, rel("inner").join(&path_of(1))))
        .expect("q")
        .expect("folded row");
    assert_eq!(folded.id, before.id);
    assert_eq!(folded.identity, before.identity);
    let s = cat.read().sources().expect("sources");
    assert_eq!(
        s.iter().find(|x| x.id == inner).expect("inner").folded_into,
        Some(outer)
    );

    // Re-link.
    w.submit(Command::SetSourceStatus {
        source: outer,
        status: SourceStatus::Offline,
    })
    .expect("w");
    w.submit(Command::RelinkSource {
        source: outer,
        root: b"/mnt/b".to_vec(),
        volume_uuid: Some("u".into()),
        device_hint: None,
    })
    .expect("w");
    w.flush().expect("flush");
    let o = cat
        .read()
        .sources()
        .expect("s")
        .into_iter()
        .find(|x| x.id == outer)
        .expect("o");
    assert_eq!(o.root, b"/mnt/b".to_vec());
    assert_eq!(o.status, SourceStatus::Online);
    assert_eq!(o.volume_uuid.as_deref(), Some("u"));

    // Hash upgrades: stale rows are thumbed ones of another version.
    w.thumbed(vec![FileKey::new(outer, path_of(3))]).expect("w");
    w.flush().expect("flush");
    let stale = cat.read().stale_hashes(9).expect("stale");
    let id3 = file(&cat, outer, &path_of(3).display()).expect("3").id;
    assert_eq!(
        stale.iter().map(|s| s.0).collect::<Vec<FileId>>(),
        vec![id3]
    );
    w.submit(Command::SetHashes(vec![(
        id3,
        PerceptualHash {
            phash: 1,
            dhash: 2,
            version: 9,
        },
    )]))
    .expect("w");
    w.flush().expect("flush");
    assert!(cat.read().stale_hashes(9).expect("stale").is_empty());

    // Error rows and removal.
    w.failed(vec![FileFailure {
        key: FileKey::new(outer, path_of(3)),
        stage: Stage::Probed,
        message: "permission denied".into(),
        run: None,
    }])
    .expect("w");
    w.flush().expect("flush");
    let errs = cat.read().file_errors(id3).expect("errors");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].stage, Stage::Probed);
    w.submit(Command::RemoveSource { source: outer })
        .expect("w");
    w.flush().expect("flush");
    assert!(
        cat.read().sources().expect("s").is_empty(),
        "folded sources go too"
    );
    assert!(
        cat.read()
            .source_files(outer, None, 10)
            .expect("f")
            .is_empty()
    );
}

#[test]
fn resumed_run_clears_its_end_time() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let run = cat.writer().start_run(vec![src]).expect("run");
    for status in [RunStatus::Cancelled, RunStatus::Running] {
        cat.writer()
            .submit(Command::SetRunStatus {
                run,
                status,
                stats: None,
            })
            .expect("w");
    }
    cat.writer().flush().expect("flush");
    let r = cat.read().run(run).expect("q").expect("run");
    assert_eq!(r.status, RunStatus::Running);
    assert!(r.ended_at_ms.is_none());
}
