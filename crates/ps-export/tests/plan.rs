//! Planner scenarios (export spec: plan, layout, never overwrite, pairs, overlap, offline, free space).

mod common;

use std::collections::HashSet;
use std::fs;
use std::io;
use std::sync::atomic::AtomicBool;

use common::*;
use ps_export::{
    BundleKind, ExecConfig, ExecError, Existing, FsLibrary, LibraryProbe, MemoryLedger, NoHooks,
    PlanBlocker, PlanError, PlannedAction, ProfileSettings, execute, plan,
};
use ps_model::{ContentId, ExportProfile, RelPath};

fn lightroom() -> ProfileSettings {
    ProfileSettings::defaults(ExportProfile::Lightroom)
}

fn google() -> ProfileSettings {
    ProfileSettings::defaults(ExportProfile::GooglePhotos)
}

fn targets(p: &ps_export::Plan) -> Vec<String> {
    p.files().map(|f| f.target.display().into_owned()).collect()
}

#[test]
fn scenario_export_dry_run_writes_nothing() {
    let c = small_corpus();
    let m = corpus_model(c, &[], &[]);
    let lib_dir = scratch("dry");
    let lib = lib_dir.path().join("library");
    fs::create_dir(&lib).expect("empty library");
    let p = plan(&m.input, &[], &lib, &FsLibrary::new(&lib), lightroom()).expect("plan");
    let expected: usize = m.input.units.iter().map(|u| u.files.len()).sum();
    assert!(expected > 50, "corpus has keepers");
    assert_eq!(p.totals.files as usize, expected);
    assert_eq!(p.files().count(), expected);
    let keepers: HashSet<_> = m
        .input
        .units
        .iter()
        .flat_map(|u| u.files.iter().map(|f| f.file))
        .collect();
    let planned: HashSet<_> = p.files().map(|f| f.file).collect();
    assert_eq!(keepers, planned, "every expected keeper is listed");
    assert!(p.files().all(|f| !f.target.as_bytes().is_empty()));
    assert_eq!(
        p.totals.bytes,
        m.input
            .units
            .iter()
            .flat_map(|u| &u.files)
            .map(|f| f.size)
            .sum::<u64>()
    );
    assert!(
        fs::read_dir(&lib).expect("read").next().is_none(),
        "library still empty"
    );
}

#[test]
fn scenario_export_dated_file_placed_by_capture_date() {
    let mut s = Sources::new("dated", 1);
    let f = s.add(
        0,
        "DCIM/IMG_0042.JPG",
        &bytes(1, 1000),
        1_500_000_000,
        day(2011, 8, 14),
    );
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    assert_eq!(targets(&p), ["2011/2011-08-14/IMG_0042.JPG"]);
    let mut ledger = MemoryLedger::new();
    let r = execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert_eq!(r.copied, 1);
    assert!(lib.join("2011/2011-08-14/IMG_0042.JPG").is_file());
}

#[test]
fn scenario_export_undated_file_placed_in_undated() {
    let mut s = Sources::new("undated", 1);
    // 2009-02-03 12:00:00 UTC, no capture date at all.
    let secs = ps_model::days_from_civil(2009, 2, 3) * 86_400 + 43_200;
    let f = s.add(0, "scan.png", &bytes(2, 500), secs, None);
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    assert_eq!(targets(&p), ["Undated/2009/2009-02-03/scan.png"]);
    let mut ledger = MemoryLedger::new();
    execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert!(lib.join("Undated/2009/2009-02-03/scan.png").is_file());
}

#[test]
fn scenario_export_name_collision_gets_suffix() {
    let mut s = Sources::new("collide", 2);
    let a = s.add(0, "a/IMG_0001.JPG", &bytes(3, 800), 0, day(2015, 6, 1));
    let b = s.add(1, "b/IMG_0001.JPG", &bytes(4, 900), 0, day(2015, 6, 1));
    let input = s.input(vec![unit(vec![a]), unit(vec![b])]);
    let lib = s.library();
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    let mut t = targets(&p);
    t.sort();
    assert_eq!(
        t,
        [
            "2015/2015-06-01/IMG_0001 (2).JPG",
            "2015/2015-06-01/IMG_0001.JPG"
        ]
    );
    assert_eq!(p.totals.collisions, 1);
    let mut ledger = MemoryLedger::new();
    let r = execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert_eq!(r.copied, 2);
    assert_eq!(
        fs::read(lib.join("2015/2015-06-01/IMG_0001.JPG")).expect("a"),
        bytes(3, 800)
    );
    assert_eq!(
        fs::read(lib.join("2015/2015-06-01/IMG_0001 (2).JPG")).expect("b"),
        bytes(4, 900)
    );
}

#[test]
fn scenario_export_identical_existing_file_adopted() {
    let mut s = Sources::new("adopt", 1);
    let data = bytes(5, 1200);
    let f = s.add(0, "IMG_0007.JPG", &data, 0, day(2012, 1, 2));
    let input = s.input(vec![unit(vec![f.clone()])]);
    let lib = s.library();
    let target = lib.join("2012/2012-01-02/IMG_0007.JPG");
    fs::create_dir_all(target.parent().expect("p")).expect("mkdir");
    fs::write(&target, &data).expect("pre-existing identical copy");
    set_mtime(&target, 1_000);
    let before = mtime_ns(&target);
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    assert_eq!(p.totals.files, 0);
    assert_eq!(p.totals.adopted, 1);
    let pf = p.files().next().expect("planned");
    assert_eq!(pf.action, PlannedAction::Adopt);
    let mut ledger = MemoryLedger::new();
    let r = execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert_eq!((r.copied, r.adopted), (0, 1));
    assert_eq!(mtime_ns(&target), before, "adopted file untouched");
    let e = ledger
        .get(&rel("2012/2012-01-02/IMG_0007.JPG"))
        .expect("ledger row");
    assert_eq!(e.state, ps_model::LedgerState::Done);
    assert_eq!(e.content, f.content);
    assert_eq!(
        files_under(&lib)
            .iter()
            .filter(|p| !p.starts_with(".photosquirrel"))
            .count(),
        1
    );
}

/// A probe with a fixed free space.
struct TinyVolume(FsLibrary, u64);

impl LibraryProbe for TinyVolume {
    fn existing(&self, rel: &RelPath) -> io::Result<Existing> {
        self.0.existing(rel)
    }
    fn identical(&self, rel: &RelPath, c: &ContentId, size: u64) -> io::Result<bool> {
        self.0.identical(rel, c, size)
    }
    fn free_space(&self) -> io::Result<Option<u64>> {
        Ok(Some(self.1))
    }
}

#[test]
fn scenario_export_insufficient_space_blocks_export() {
    let mut s = Sources::new("space", 1);
    let f = s.add(0, "big.JPG", &bytes(6, 200_000), 0, day(2020, 2, 2));
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let p = plan(
        &input,
        &[],
        &lib,
        &TinyVolume(FsLibrary::new(&lib), 100_000),
        google(),
    )
    .expect("plan");
    let Some(PlanBlocker::InsufficientSpace {
        shortfall,
        free,
        required,
    }) = p.blocker()
    else {
        panic!("blocked expected");
    };
    assert_eq!(free, 100_000);
    assert_eq!(shortfall, required - free);
    assert!(required >= 200_000);
    let mut ledger = MemoryLedger::new();
    let err = execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect_err("cannot start");
    assert!(matches!(err, ExecError::Blocked(_)));
    assert!(err.to_string().contains("short"));
    assert!(!lib.exists(), "nothing written");
    assert!(ledger.is_empty());
    // The real volume has room.
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    assert!(p.blocker().is_none());
}

#[test]
fn scenario_export_library_inside_source_refused() {
    let mut s = Sources::new("overlap", 2);
    let f = s.add(0, "a.JPG", &bytes(7, 100), 0, day(2020, 1, 1));
    let input = s.input(vec![unit(vec![f])]);
    let inside = s.sources[1].root.join("Exports/Library");
    let err = plan(&input, &[], &inside, &FsLibrary::new(&inside), google()).expect_err("refused");
    match &err {
        PlanError::LibraryInsideSource { source_name, .. } => assert_eq!(source_name, "drive-1"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(err.to_string().contains("drive-1"));
    // A source inside the library is refused too.
    let outer = s.dir.path().to_path_buf();
    assert!(matches!(
        plan(&input, &[], &outer, &FsLibrary::new(&outer), google()),
        Err(PlanError::SourceInsideLibrary { .. })
    ));
    assert!(!inside.exists());
}

#[test]
fn scenario_export_raw_jpeg_pair_keeps_stem() {
    let mut s = Sources::new("rawpair", 1);
    let raw = s.add(0, "DCIM/DSC_0100.DNG", &bytes(8, 3000), 0, day(2016, 5, 5));
    let jpg = s.add(0, "DCIM/DSC_0100.JPG", &bytes(9, 1500), 0, day(2016, 5, 5));
    let input = s.input(vec![unit(vec![raw, jpg])]);
    let lib = s.library();
    let other = lib.join("2016/2016-05-05/DSC_0100.JPG");
    fs::create_dir_all(other.parent().expect("p")).expect("mkdir");
    fs::write(&other, b"another photo").expect("other");
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), lightroom()).expect("plan");
    assert_eq!(p.bundles[0].kind, BundleKind::KeeperSet);
    let mut t = targets(&p);
    t.sort();
    assert_eq!(
        t,
        [
            "2016/2016-05-05/DSC_0100 (2).DNG",
            "2016/2016-05-05/DSC_0100 (2).JPG"
        ]
    );
    assert_eq!(
        p.bundles[0]
            .sidecar
            .as_ref()
            .map(|r| r.display().into_owned())
            .as_deref(),
        Some("2016/2016-05-05/DSC_0100 (2).xmp")
    );
    let mut ledger = MemoryLedger::new();
    execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert_eq!(fs::read(&other).expect("other"), b"another photo");
    assert!(lib.join("2016/2016-05-05/DSC_0100 (2).DNG").is_file());
    assert!(lib.join("2016/2016-05-05/DSC_0100 (2).JPG").is_file());
}

#[test]
fn scenario_export_live_photo_pair_kept_together() {
    let mut s = Sources::new("live", 1);
    let still = s.add(
        0,
        "iPhone/IMG_2000.HEIC",
        &bytes(10, 2000),
        0,
        day(2019, 7, 1),
    );
    let mut video = s.add(
        0,
        "iPhone/IMG_2000.MOV",
        &bytes(11, 4000),
        0,
        day(2019, 7, 1),
    );
    // The video's container date says the next day (UTC), but it belongs with the still.
    video.capture = Some(ps_model::CaptureTime::from_parts(2019, 7, 1, 12, 0, 1));
    let input = s.input(vec![unit(vec![video]), unit(vec![still])]);
    let lib = s.library();
    let other = lib.join("2019/2019-07-01/IMG_2000.MOV");
    fs::create_dir_all(other.parent().expect("p")).expect("mkdir");
    fs::write(&other, b"unrelated video").expect("other");
    let apple = ProfileSettings::defaults(ExportProfile::ApplePhotos);
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), apple).expect("plan");
    assert_eq!(p.bundles.len(), 1);
    assert_eq!(p.bundles[0].kind, BundleKind::LivePhoto);
    let mut t = targets(&p);
    t.sort();
    assert_eq!(
        t,
        [
            "2019/2019-07-01/IMG_2000 (2).HEIC",
            "2019/2019-07-01/IMG_2000 (2).MOV"
        ]
    );
    let mut ledger = MemoryLedger::new();
    execute(
        &p,
        &input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert!(lib.join("2019/2019-07-01/IMG_2000 (2).HEIC").is_file());
    assert!(lib.join("2019/2019-07-01/IMG_2000 (2).MOV").is_file());
    // Re-planned with the ledger (any profile): both already exported.
    let rows = ps_export::Ledger::entries(&ledger).expect("rows");
    let p = plan(&input, &rows, &lib, &FsLibrary::new(&lib), google()).expect("plan");
    assert_eq!(p.bundles.len(), 0);
    assert_eq!(p.totals.already_exported, 2);
}

#[test]
fn scenario_export_offline_source_files_deferred() {
    let c = small_corpus();
    let drive = c.truth.drives[1].clone();
    let m = corpus_model(c, &[], &[drive.as_str()]);
    let lib_dir = scratch("offline");
    let lib = lib_dir.path().join("library");
    let p = plan(&m.input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    let off = m
        .input
        .sources
        .iter()
        .find(|s| s.name == drive)
        .expect("source")
        .id;
    let expected_off: usize = m
        .input
        .units
        .iter()
        .filter(|u| u.files.iter().any(|f| f.source == off))
        .map(|u| u.files.len())
        .sum();
    assert!(expected_off > 0);
    assert_eq!(p.unavailable.len(), expected_off);
    assert!(p.unavailable.iter().all(|u| u.source_name == drive));
    assert!(p.files().all(|f| f.source != off));
    let mut ledger = MemoryLedger::new();
    let r = execute(
        &p,
        &m.input,
        &mut ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    // They stay pending.
    let entries: Vec<_> = ps_export::Ledger::entries(&ledger).expect("entries");
    let checks = ps_export::check_library(&lib, &entries, false);
    let d = ps_export::derive_dispositions(&ps_export::DispositionFacts {
        files: &m.files,
        groups: &m.groups,
        ledger: &entries,
        checks: &checks,
    });
    for u in m
        .input
        .units
        .iter()
        .filter(|u| u.files.iter().any(|f| f.source == off))
    {
        for f in &u.files {
            let x = d.iter().find(|x| x.file == f.file).expect("disposition");
            assert_eq!(x.disposition, ps_model::Disposition::Pending);
        }
    }
}

mod naming {
    use super::*;
    use proptest::prelude::*;
    use ps_export::layout::{candidate, split_name};

    proptest! {
        #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

        /// Random names (case variants, shared stems, pairs) on a few days into a library with some
        /// pre-existing files: targets are unique case-insensitively, never an existing file, in the right
        /// day folder, named `stem[ (n)]ext` with one suffix per bundle.
        #[test]
        fn layout_and_collision_naming(
            specs in proptest::collection::vec((0u8..3, 0u8..4, 0u8..3, any::<bool>()), 1..24),
            existing in proptest::collection::vec((0u8..3, 0u8..4, 0u8..3), 0..6),
        ) {
            let stems = ["IMG_0001", "img_0001", "DSC_7", "Photo"];
            let exts = [".JPG", ".jpg", ".PNG"];
            let mut s = Sources::new("prop", 1);
            let lib = s.library();
            for (i, (d, st, e)) in existing.iter().enumerate() {
                let p = lib.join(format!("2020/2020-01-0{}", d + 1)).join(format!("{}{}", stems[*st as usize], exts[*e as usize]));
                fs::create_dir_all(p.parent().expect("p")).expect("mkdir");
                if !p.exists() { fs::write(&p, bytes(1000 + i as u64, 64)).expect("existing"); }
            }
            let mut units = Vec::new();
            for (i, (d, st, e, pair)) in specs.iter().enumerate() {
                let name = format!("{}{}", stems[*st as usize], exts[*e as usize]);
                let f = s.add(0, &format!("u{i}/{name}"), &bytes(i as u64, 100 + i), 0, day(2020, 1, d + 1));
                let mut files = vec![f];
                if *pair {
                    let raw = format!("u{i}/{}.DNG", stems[*st as usize]);
                    files.insert(0, s.add(0, &raw, &bytes(500 + i as u64, 90), 0, day(2020, 1, d + 1)));
                }
                units.push(unit(files));
            }
            let input = s.input(units);
            let before = snapshot(s.dir.path(), &[]);
            let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), ProfileSettings::defaults(ExportProfile::Lightroom)).expect("plan");
            prop_assert_eq!(snapshot(s.dir.path(), &[]), before, "planning wrote nothing");
            let mut seen = HashSet::new();
            for b in &p.bundles {
                for f in &b.files {
                    let t = f.target.as_bytes().to_ascii_lowercase();
                    prop_assert!(seen.insert(t), "unique target {}", f.target);
                    prop_assert!(!f.target.under(&lib).exists(), "never an existing file");
                    let kf = input.units[f.unit].files.iter().find(|k| k.file == f.file).expect("file");
                    let (stem, ext) = split_name(kf.name());
                    let want = candidate(stem, ext, b.suffix);
                    prop_assert_eq!(f.target.file_name(), want.as_slice());
                    let folder = ps_export::layout::folder_of(kf);
                    prop_assert_eq!(f.target.parent().expect("folder").display().into_owned(), folder);
                }
                if let Some(x) = &b.sidecar {
                    prop_assert!(seen.insert(x.as_bytes().to_ascii_lowercase()));
                }
            }
            let planned: usize = p.files().count();
            prop_assert_eq!(planned, input.units.iter().map(|u| u.files.len()).sum::<usize>());
        }
    }
}

/// A keeper named like a copy ("Copy of IMG_5437.JPG") whose group also has "IMG_5437.JPG" is filed under
/// the cleaner name (`layout::cleaner_name`, applied by the service); its bytes still come from its own path.
#[test]
fn scenario_export_copy_named_keeper_gets_the_clean_name() {
    let mut s = Sources::new("cleanname", 1);
    let mut keeper = s.add(
        0,
        "Downloads/Copy of IMG_5437.JPG",
        &bytes(5437, 30_000),
        0,
        day(2019, 9, 1),
    );
    let small = s.add(
        0,
        "Phone/IMG_5437.JPG",
        &bytes(5438, 9_000),
        0,
        day(2019, 9, 1),
    );
    keeper.library_name = ps_export::layout::cleaner_name(
        keeper.rel_path.file_name(),
        &[keeper.rel_path.file_name(), small.rel_path.file_name()],
    );
    let mut u = unit(vec![keeper.clone()]);
    u.copies.push(ps_export::CopyRef {
        file: small.file,
        source: small.source,
        rel_path: small.rel_path.clone(),
    });
    let input = s.input(vec![u]);
    let lib = s.library();
    let p = plan(&input, &[], &lib, &FsLibrary::new(&lib), google()).expect("plan");
    assert_eq!(targets(&p), vec!["2019/2019-09-01/IMG_5437.JPG"]);
    let f = p.files().next().expect("file");
    assert!(f.source_path.ends_with("Downloads/Copy of IMG_5437.JPG"));
}
