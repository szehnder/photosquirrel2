//! Completion-report scenarios (dispositions derived from catalog facts and the ledger, ADR-0023).

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use common::*;
use ps_export::{
    Badge, CopyCheck, DispositionFacts, ExecConfig, FileDisposition, FsLibrary, Ledger,
    MemoryLedger, NoHooks, ProfileSettings, check_library, derive_dispositions, execute, plan,
    report,
};
use ps_model::{Disposition, ExportProfile, FileId, LedgerEntry, PhotoClass, RelPath};

struct Exported {
    _dir: tempfile::TempDir,
    lib: PathBuf,
    m: CorpusModel,
    rows: Vec<LedgerEntry>,
}

/// Export the small corpus, leaving out `skip` units.
fn exported(tag: &str, skip: &[usize]) -> Exported {
    let c = small_corpus();
    let m = corpus_model(c, &[], &[]);
    let mut input = m.input.clone();
    input.units = input
        .units
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !skip.contains(i))
        .map(|(_, u)| u)
        .collect();
    let dir = scratch(tag);
    let lib = dir.path().join("library");
    let mut ledger = MemoryLedger::new();
    let settings = ProfileSettings::defaults(ExportProfile::GooglePhotos);
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), settings).expect("plan");
    let r = execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert!(r.failed.is_empty());
    let rows = ledger.entries().expect("rows");
    Exported {
        _dir: dir,
        lib,
        m,
        rows,
    }
}

fn derive(e: &Exported, verify: bool) -> (Vec<FileDisposition>, HashMap<RelPath, CopyCheck>) {
    let checks = check_library(&e.lib, &e.rows, verify);
    let d = derive_dispositions(&DispositionFacts {
        files: &e.m.files,
        groups: &e.m.groups,
        ledger: &e.rows,
        checks: &checks,
    });
    (d, checks)
}

fn of(d: &[FileDisposition], f: FileId) -> Disposition {
    d.iter().find(|x| x.file == f).expect("file").disposition
}

fn truth_of(e: &Exported, f: FileId) -> &'static ps_fixtures::TruthFile {
    let path =
        e.m.ids
            .iter()
            .find(|(_, id)| **id == f)
            .expect("id")
            .0
            .clone();
    small_corpus()
        .truth
        .files
        .iter()
        .find(|t| t.path == path)
        .expect("truth")
}

#[test]
fn scenario_completion_report_dispositions_derived_after_export() {
    let e = exported("disp", &[]);
    let (d, _) = derive(&e, false);
    let keepers: std::collections::HashSet<FileId> =
        e.m.input
            .units
            .iter()
            .flat_map(|u| u.files.iter().map(|f| f.file))
            .collect();
    let mut junk = 0;
    for x in &d {
        let t = truth_of(&e, x.file);
        if keepers.contains(&x.file) {
            assert_eq!(x.disposition, Disposition::Exported, "{}", t.path);
            assert!(x.library_path.is_some());
        } else if t.category.is_junk() {
            assert_eq!(x.disposition, Disposition::Excluded, "{}", t.path);
            junk += 1;
        } else {
            assert_eq!(x.disposition, Disposition::Covered, "{}", t.path);
            assert!(x.library_path.is_some(), "covered names the keeper's copy");
        }
    }
    assert!(junk > 0);
    assert!(d.iter().all(|x| x.disposition != Disposition::Pending));
}

/// A unit index whose group has several members.
fn grouped_unit(m: &CorpusModel) -> usize {
    m.input
        .units
        .iter()
        .position(|u| u.copies.len() >= 3)
        .expect("a group")
}

#[test]
fn scenario_completion_report_covered_requires_exported_keeper() {
    let c = small_corpus();
    let skip = grouped_unit(&corpus_model(c, &[], &[]));
    let e = exported("covered", &[skip]);
    let (d, _) = derive(&e, false);
    let u = &e.m.input.units[skip];
    for copy in &u.copies {
        assert_eq!(of(&d, copy.file), Disposition::Pending);
    }
}

#[test]
fn scenario_completion_report_deleted_library_copy_becomes_pending() {
    let e = exported("deleted", &[]);
    let ui = grouped_unit(&e.m);
    let u = &e.m.input.units[ui];
    let keeper = &u.files[0];
    let row = e.rows.iter().find(|r| r.file == keeper.file).expect("row");
    fs::remove_file(row.library_rel_path.under(&e.lib)).expect("user deletes the copy");
    let (d, checks) = derive(&e, false);
    assert_eq!(checks[&row.library_rel_path], CopyCheck::Missing);
    for copy in &u.copies {
        assert_eq!(of(&d, copy.file), Disposition::Pending);
    }
}

#[test]
fn scenario_completion_report_verify_detects_altered_copy() {
    let e = exported("verify", &[]);
    let ui = grouped_unit(&e.m);
    let keeper = e.m.input.units[ui].files[0].file;
    let row = e.rows.iter().find(|r| r.file == keeper).expect("row");
    let path = row.library_rel_path.under(&e.lib);
    let mut bytes = fs::read(&path).expect("copy");
    bytes[100] ^= 0x55;
    fs::write(&path, bytes).expect("alter, same size");
    // A changed modification time alone is enough for the quick check to re-hash it (ADR-0044).
    let (d, _) = derive(&e, false);
    assert_eq!(of(&d, keeper), Disposition::Pending);
    // With its recorded time restored by hand, only verify can see it.
    let recorded = row.library_mtime_ns.expect("time recorded");
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|f| f.set_modified(ps_export::copy::system_time(recorded)))
        .expect("restore mtime");
    let (d, _) = derive(&e, false);
    assert_eq!(
        of(&d, keeper),
        Disposition::Exported,
        "size and time unchanged: trusted without reading"
    );
    let (d, checks) = derive(&e, true);
    assert_eq!(of(&d, keeper), Disposition::Pending);
    let r = report(&e.m.files, &d, &checks);
    assert_eq!(
        r.mismatches,
        vec![(row.library_rel_path.clone(), CopyCheck::HashMismatch)]
    );
}

#[test]
fn scenario_completion_report_report_per_folder() {
    let e = exported("folders", &[]);
    let (d, checks) = derive(&e, false);
    let r = report(&e.m.files, &d, &checks);
    assert_eq!(r.sources.len(), small_corpus().truth.drives.len());
    for s in &r.sources {
        let mut sum = ps_model::CompletionCounts::default();
        for f in &s.folders {
            let in_folder: Vec<_> =
                e.m.files
                    .iter()
                    .filter(|x| x.source == s.source && x.rel_path.parent() == f.folder)
                    .collect();
            assert_eq!(f.counts.total_files(), in_folder.len() as u64);
            assert_eq!(
                f.counts.bytes.iter().sum::<u64>(),
                in_folder.iter().map(|x| x.size).sum::<u64>()
            );
            for i in 0..6 {
                sum.files[i] += f.counts.files[i];
                sum.bytes[i] += f.counts.bytes[i];
            }
        }
        assert_eq!(sum, s.counts);
    }
}

#[test]
fn scenario_completion_report_excluded_reasons_summarized() {
    let e = exported("reasons", &[]);
    let (d, checks) = derive(&e, false);
    let r = report(&e.m.files, &d, &checks);
    let mut total = 0;
    for s in &r.sources {
        let (n, b) = s
            .excluded
            .values()
            .fold((0, 0), |a, v| (a.0 + v.0, a.1 + v.1));
        assert_eq!(n, s.counts.files_of(Disposition::Excluded));
        assert_eq!(b, s.counts.bytes[Disposition::Excluded as usize]);
        for f in &s.folders {
            let n: u64 = f.excluded.values().map(|v| v.0).sum();
            assert_eq!(n, f.counts.files_of(Disposition::Excluded));
        }
        total += n;
    }
    let expected =
        e.m.files
            .iter()
            .filter(|f| matches!(f.class, Some(PhotoClass::Junk | PhotoClass::Graphic)))
            .filter(|f| !e.m.groups.iter().any(|g| g.members.contains(&f.file)))
            .count() as u64;
    assert_eq!(total, expected);
}

#[test]
fn scenario_completion_report_fully_accounted_badge() {
    let e = exported("badge", &[]);
    let (d, checks) = derive(&e, false);
    let r = report(&e.m.files, &d, &checks);
    for s in &r.sources {
        assert_eq!(s.badge, Badge::FullyAccounted);
        assert_eq!(s.badge.label(), "100% accounted for");
        assert!(s.folders.iter().all(|f| f.badge == Badge::FullyAccounted));
    }
}

#[test]
fn scenario_completion_report_pending_blocks_badge() {
    let c = small_corpus();
    let m = corpus_model(c, &[], &[]);
    // A keeper without duplicates.
    let skip = m
        .input
        .units
        .iter()
        .position(|u| u.copies.len() == 1 && u.files.len() == 1)
        .expect("singleton keeper");
    let drive = m.input.units[skip].files[0].source;
    let e = exported("pending", &[skip]);
    let (d, checks) = derive(&e, false);
    let r = report(&e.m.files, &d, &checks);
    for s in &r.sources {
        if s.source == drive {
            assert_eq!(
                s.badge,
                Badge::Outstanding {
                    pending: 1,
                    unreadable: 0,
                    in_quarantine: 0,
                    superseded: 0,
                }
            );
            assert_eq!(s.badge.label(), "1 pending");
        } else {
            assert_eq!(s.badge, Badge::FullyAccounted);
        }
    }
}

/// A group whose keeper the user excluded is not exported (the service leaves it out of the export input), and
/// its other copies are excluded with it instead of staying pending forever.
#[test]
fn scenario_completion_report_excluded_keeper_excludes_its_group() {
    let c = small_corpus();
    let skip = grouped_unit(&corpus_model(c, &[], &[]));
    let mut e = exported("exclkeeper", &[skip]);
    let u = e.m.input.units[skip].clone();
    let keeper = u.files[0].file;
    for f in &mut e.m.files {
        if f.file == keeper {
            f.user_excluded = true;
        }
    }
    let (d, _) = derive(&e, false);
    for copy in &u.copies {
        let x = d.iter().find(|x| x.file == copy.file).expect("file");
        assert_eq!(x.disposition, Disposition::Excluded, "{:?}", copy.rel_path);
        assert_eq!(x.reason, Some(ps_model::ExclusionReason::UserExcluded));
    }
    assert!(
        d.iter().all(|x| x.disposition != Disposition::Pending),
        "nothing left pending"
    );
}
