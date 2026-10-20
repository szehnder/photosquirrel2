//! `scanning` scenarios on the generated corpus: persisted stages, read-only guarantees, probe-only junk and
//! the single full read (scanning tasks 3.1-3.3).

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use common::*;
use ps_model::{PhotoClass, SourceId, Stage};
use ps_scan::RunSummary;

const PROBE_LIMIT: u64 = 262_144;

struct FullScan {
    env: Env,
    sources: Vec<(String, SourceId, PathBuf)>,
    before: BTreeMap<String, Snap>,
    summary: RunSummary,
}

/// The whole corpus (its three drives as three sources) scanned once with an instrumented file layer.
fn full_scan() -> &'static FullScan {
    static S: OnceLock<FullScan> = OnceLock::new();
    S.get_or_init(|| {
        let c = corpus();
        let before = snapshot_tree(&c.root);
        let env = env();
        let sources: Vec<(String, SourceId, PathBuf)> = c
            .truth
            .drives
            .iter()
            .map(|d| {
                let id = env.add(&c.root.join(d));
                (d.clone(), id, env.root(id))
            })
            .collect();
        env.instr.reset(); // count the scan only, not the fingerprinting
        let summary = env.scan(&[]);
        FullScan {
            env,
            sources,
            before,
            summary,
        }
    })
}

#[test]
fn scenario_scanning_corpus_files_reach_final_stages() {
    let s = full_scan();
    completed(&s.summary);
    let c = corpus();
    let mut checked = 0;
    for (drive, id, _) in &s.sources {
        let files = s.env.files(*id);
        let truth: Vec<_> = truth_under(c, drive).collect();
        assert_eq!(
            files.len(),
            truth.len(),
            "every file of {drive} is catalogued"
        );
        for (rel, t) in truth {
            let f = files
                .get(&rel)
                .unwrap_or_else(|| panic!("{rel} catalogued"));
            if t.expected_class.is_candidate() {
                assert_eq!(f.stage, Stage::Thumbed, "{rel} ({:?})", t.category);
                assert!(f.identity.is_some(), "{rel} has an identity");
                assert_eq!(
                    f.identity.map(|i| i.to_hex()),
                    Some(t.content.clone()),
                    "{rel}: identity is the BLAKE3 of the file"
                );
            } else {
                assert_eq!(f.stage, Stage::Skipped, "{rel} ({:?})", t.category);
                assert!(!f.reasons.is_empty(), "{rel} is skipped with a reason");
            }
            assert_eq!(f.class, Some(t.expected_class), "{rel}");
            checked += 1;
        }
        assert!(
            files
                .values()
                .all(|f| !matches!(f.stage, Stage::Discovered | Stage::Probed)),
            "no file of {drive} left at discovered or probed"
        );
    }
    assert_eq!(checked, c.truth.files.len());
    // Every thumbed image has its thumbnail stored.
    let thumbs = s.env.s().thumbs().count().expect("count");
    assert!(thumbs > 0);
    assert_eq!(s.summary.stages.discovered + s.summary.stages.probed, 0);
}

#[test]
fn scenario_scanning_scanner_never_opens_for_writing() {
    let s = full_scan();
    let opens = s.env.instr.opens();
    assert!(
        opens.len() as u64 >= s.summary.processed,
        "every processed file was opened through the instrumented layer"
    );
    for o in &opens {
        assert!(
            o.access.read && !o.access.writes(),
            "{} opened with {:?}",
            o.path.display(),
            o.access
        );
        if cfg!(target_os = "linux") && o.ok {
            assert!(o.access.verified, "access read back from the kernel");
        }
    }
    // Nothing in the tree was created, modified, renamed or deleted.
    assert_eq!(snapshot_tree(&corpus().root), s.before);
}

#[test]
fn scenario_scanning_junk_costs_only_a_probe() {
    let s = full_scan();
    let bytes = s.env.instr.bytes_per_file();
    let mut skipped = 0;
    for (_, id, root) in &s.sources {
        for (rel, f) in s.env.files(*id) {
            if f.stage == Stage::Skipped {
                let n = bytes.get(&root.join(&rel)).copied().unwrap_or(0);
                assert!(n <= PROBE_LIMIT, "{rel}: {n} bytes read for a skipped file");
                skipped += 1;
            }
        }
    }
    assert!(skipped > 0, "the corpus has junk");
}

#[test]
fn scenario_scanning_one_full_read_per_candidate() {
    let s = full_scan();
    let bytes = s.env.instr.bytes_per_file();
    let mut analyzed = 0;
    for (_, id, root) in &s.sources {
        for (rel, f) in s.env.files(*id) {
            if matches!(f.stage, Stage::Analyzed | Stage::Thumbed) {
                let n = bytes.get(&root.join(&rel)).copied().unwrap_or(0);
                assert!(
                    n <= f.stat.size + PROBE_LIMIT,
                    "{rel}: {n} bytes read for a {} byte file",
                    f.stat.size
                );
                assert!(n >= f.stat.size, "{rel} was read in full ({n} bytes)");
                analyzed += 1;
            }
        }
    }
    assert!(analyzed > 0);
}

#[test]
fn scenario_scanning_read_only_tree_scans_fully() {
    let c = corpus();
    let tmp = tempfile::tempdir().expect("tmp");
    let writable = tmp.path().join("writable");
    let readonly = tmp.path().join("readonly");
    copy_tree(&c.root.join("drive-c"), &writable);
    copy_tree(&c.root.join("drive-c"), &readonly);
    set_tree_readonly(&readonly, true);
    let before = snapshot_tree(&readonly);

    let a = env();
    let wa = a.add(&writable);
    completed(&a.scan(&[wa]));
    let b = env();
    let rb = b.add(&readonly);
    let summary = b.scan(&[rb]);
    let after = snapshot_tree(&readonly);
    set_tree_readonly(&readonly, false);

    completed(&summary);
    assert_eq!(summary.errors, 0, "{summary:#?}");
    let fa = a.files(wa);
    let fb = b.files(rb);
    assert_eq!(fa.len(), fb.len());
    for (rel, f) in &fa {
        assert_eq!(
            normalized(f),
            normalized(&fb[rel]),
            "{rel} scans the same read-only"
        );
    }
    assert_eq!(after, before, "the read-only tree is unchanged");
    assert!(fb.values().any(|f| f.class == Some(PhotoClass::Photo)));
}

/// Make every file 0444 and folder 0555 (or back to 0644/0755).
fn set_tree_readonly(root: &std::path::Path, ro: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut dirs = vec![root.to_path_buf()];
        let mut all_dirs = Vec::new();
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else {
                    let mode = if ro { 0o444 } else { 0o644 };
                    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                        .expect("chmod file");
                }
            }
            all_dirs.push(d);
        }
        // Folders last (children first when making them writable again does not matter for chmod).
        for d in all_dirs {
            let mode = if ro { 0o555 } else { 0o755 };
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(mode)).expect("chmod dir");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (root, ro);
    }
}
