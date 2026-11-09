//! Spec `app-updates`, "Updates touch only the installation": an install and a rollback change nothing in the
//! sources, the library, the quarantine or the data directory (ADR-0004, ADR-0126).
//!
//! A scanned small corpus with an exported library and a quarantine; a fake AppImage installation updated from
//! 0.4.0 to 0.4.1 and rolled back by the real installer. The "programs" are shell scripts, so this runs on Unix.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ps_export::ExecConfig;
use ps_export::quarantine::RunProgress;
use ps_model::SourceId;
use ps_service::cleanup::RunOptions;
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};
use ps_update::UpdateDir;
use ps_update::install::{self, Action, Plan, Step};
use ps_update::kind::{Installation, Kind};
use ps_update::state::{Ready, UpdateState};

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let (p, t) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            copy_tree(&p, &t);
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}

/// Every file under `root`: relative path -> bytes' BLAKE3.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, [u8; 32]> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, [u8; 32]>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_path_buf(),
                    *blake3::hash(&fs::read(&p).unwrap()).as_bytes(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn program(path: &Path, version: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\necho 'photosquirrel {version}'\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn scenario_app_updates_user_data_untouched_by_install_and_rollback() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(base).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("ps-update-scope-")
        .tempdir_in(base)
        .unwrap();
    let shared = ps_fixtures::shared_corpus(
        base,
        "update-scope-small-42",
        42,
        ps_fixtures::Preset::Small,
    );
    let corpus = tmp.path().join("corpus");
    copy_tree(&shared, &corpus);
    let corpus = corpus
        .canonicalize()
        .map(ps_model::path::simplify_canonical)
        .unwrap();
    let data = tmp.path().join("data");
    let library = tmp.path().join("Library");

    // A scanned corpus, an exported library and a quarantine (one drive cleaned up).
    {
        let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
        for d in ["drive-a", "drive-b", "drive-c"] {
            svc.add_source(&corpus.join(d)).unwrap();
        }
        svc.start(&[]).unwrap();
        svc.wait().unwrap();
        let (_, report) = svc
            .exports()
            .run(
                &library,
                profile_settings("lightroom").unwrap(),
                &ExecConfig::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let plan = svc.cleanups().plan(SourceId(2), None, None).unwrap();
        let run = plan.summary.expect("a plan").run.id;
        svc.cleanups()
            .run(
                run,
                RunOptions::default(),
                &AtomicBool::new(false),
                &RunProgress::default(),
            )
            .unwrap();
    }
    let watched = [corpus.clone(), library.clone(), data.clone()];
    let before: Vec<_> = watched.iter().map(|p| snapshot(p)).collect();
    assert!(
        before[0].keys().any(|k| k
            .to_string_lossy()
            .contains(ps_export::quarantine::QUARANTINE_DIR)),
        "a quarantine is present"
    );

    // A fake AppImage installation, updated to 0.4.1 and rolled back by the real installer.
    let updates = UpdateDir::at(tmp.path().join("updates"));
    let location = tmp.path().join("Apps/PhotoSquirrel.AppImage");
    program(&location, "0.4.0");
    let payload = updates
        .ready_dir("0.4.1")
        .join("PhotoSquirrel-0.4.1.AppImage");
    program(&payload, "0.4.1");
    let mut state = UpdateState {
        ready: Some(Ready {
            version: "0.4.1".into(),
            kind: "appimage".into(),
            file: payload,
            sha256: String::new(),
            size: 0,
        }),
        ..Default::default()
    };
    state.remember_catalog(&data);
    state.save(&updates).unwrap();
    let installation = Installation {
        kind: Kind::AppImage,
        location: location.clone(),
        cannot_install: None,
    };
    let plan = install::plan_install(&updates, &installation, "0.4.0", 27, false, None).unwrap();
    assert_eq!(
        install::finish(&updates, &plan, Duration::from_secs(1), None).unwrap(),
        Step::Done
    );
    let rollback = install::plan_rollback(&updates, &installation, "0.4.1", false, None).unwrap();
    assert_eq!(Plan::load(&rollback).unwrap().action, Action::Rollback);
    assert_eq!(
        install::finish(&updates, &rollback, Duration::from_secs(1), None).unwrap(),
        Step::Done
    );
    let out = std::process::Command::new(&location).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "photosquirrel 0.4.0"
    );

    let after: Vec<_> = watched.iter().map(|p| snapshot(p)).collect();
    for (i, root) in watched.iter().enumerate() {
        assert_eq!(after[i], before[i], "{} changed", root.display());
    }
}
