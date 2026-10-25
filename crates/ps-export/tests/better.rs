//! Groups already in the library through an earlier member's copy are not copied again when their keeper
//! changes; better copies are listed and copied only on request (ADR-0092).

mod common;

use std::cell::Cell;
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::AtomicBool;

use common::*;
use ps_export::{
    CopyCheck, CopyRef, DispositionFacts, ExecConfig, ExportInput, ExportUnit, FileFacts,
    FsLibrary, GroupFacts, KeeperFile, Ledger, LibraryProbe, MemoryLedger, NoHooks, Plan,
    PlannedAction, ProfileSettings, check_library, derive_dispositions, execute, plan, report,
};
use ps_model::{ContentId, Disposition, ExportProfile, FileId, GroupId, PhotoClass, RelPath};

fn lightroom() -> ProfileSettings {
    ProfileSettings::defaults(ExportProfile::Lightroom)
}

fn google() -> ProfileSettings {
    ProfileSettings::defaults(ExportProfile::GooglePhotos)
}

fn apple() -> ProfileSettings {
    ProfileSettings::defaults(ExportProfile::ApplePhotos)
}

fn copy_ref(f: &KeeperFile) -> CopyRef {
    CopyRef {
        file: f.file,
        source: f.source,
        rel_path: f.rel_path.clone(),
        content: Some(f.content),
    }
}

/// A group unit: `keeper_set` exported, every file of `members` (keeper set included) a copy.
fn group_unit(g: i64, keeper_set: Vec<KeeperFile>, others: &[&KeeperFile]) -> ExportUnit {
    let mut u = unit(keeper_set);
    u.group = Some(GroupId(g));
    u.keeper_reasons = vec!["original format (HEIC)".into()];
    for o in others {
        u.copies.push(copy_ref(o));
    }
    u
}

fn run(
    input: &ExportInput,
    lib: &std::path::Path,
    ledger: &mut MemoryLedger,
    settings: ProfileSettings,
) -> (Plan, ps_export::ExportReport) {
    let rows = ledger.entries().expect("rows");
    let p = plan(input, &rows, lib, &FsLibrary::new(lib), settings).expect("plan");
    let r = execute(
        &p,
        input,
        ledger,
        &ExecConfig::default(),
        &NoHooks,
        &AtomicBool::new(false),
    )
    .expect("export");
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    (p, r)
}

fn plan_only(
    input: &ExportInput,
    lib: &std::path::Path,
    ledger: &MemoryLedger,
    settings: ProfileSettings,
) -> Plan {
    let rows = ledger.entries().expect("rows");
    plan(input, &rows, lib, &FsLibrary::new(lib), settings).expect("plan")
}

/// A JPEG exported alone, then its HEIC original (decoded at last) becomes the keeper of the JPEG's group.
struct Upgraded {
    s: Sources,
    jpg: KeeperFile,
    heic: KeeperFile,
    ledger: MemoryLedger,
    /// The input after the keeper change.
    after: ExportInput,
}

fn jpeg_then_heic(tag: &str, settings: ProfileSettings) -> Upgraded {
    let mut s = Sources::new(tag, 2);
    // The JPEG is dated 2019-07-04; the HEIC's own date would choose another folder.
    let jpg = s.add(
        0,
        "Export/IMG_1234.JPG",
        &bytes(1, 3000),
        0,
        day(2019, 7, 4),
    );
    let mut heic = s.add(1, "DCIM/IMG_1234.HEIC", &bytes(2, 9000), 0, day(2021, 1, 2));
    heic.width = Some(4032);
    heic.height = Some(3024);
    let before = s.input(vec![group_unit(1, vec![jpg.clone()], &[])]);
    let mut ledger = MemoryLedger::new();
    let (_, r) = run(&before, &s.library(), &mut ledger, settings);
    assert_eq!(r.copied, 1);
    let after = s.input(vec![group_unit(1, vec![heic.clone()], &[&jpg])]);
    Upgraded {
        s,
        jpg,
        heic,
        ledger,
        after,
    }
}

fn library_rel(s: &str) -> RelPath {
    rel(s)
}

#[test]
fn scenario_export_better_keeper_after_export_copies_nothing() {
    let mut u = jpeg_then_heic("better-nothing", lightroom());
    let lib = u.s.library();
    let before = snapshot(&lib, &[]);
    let (p, r) = run(&u.after, &lib, &mut u.ledger, lightroom());
    assert_eq!(p.totals.files, 0, "{p:#?}");
    assert_eq!(r.copied, 0);
    assert_eq!(r.sidecars, 0);
    // Only the manifest may grow: no file is added, removed or changed.
    let manifest = lib.join(".photosquirrel");
    assert_eq!(snapshot(&lib, &[&manifest]), {
        let mut b = before.clone();
        b.retain(|k, _| !lib.join(k).starts_with(&manifest));
        b
    });
    assert_eq!(p.totals.better_copies, 1);
    assert_eq!(p.totals.better_copy_bytes, u.heic.size);
    let b = &p.better_copies[0];
    assert_eq!(b.group, Some(GroupId(1)));
    assert_eq!(b.library.len(), 1);
    assert_eq!(
        b.library[0].library_rel_path,
        library_rel("2019/2019-07-04/IMG_1234.JPG")
    );
    assert_eq!(b.library[0].file, u.jpg.file);
    assert_eq!(b.library[0].check, CopyCheck::Ok);
    assert_eq!(b.keeper[0].file, u.heic.file);
    assert_eq!(b.keeper[0].format, Some(ps_model::Format::Heic));
    assert_eq!(
        (b.keeper[0].width, b.keeper[0].height),
        (Some(4032), Some(3024))
    );
    assert!(b.keeper[0].source_path.ends_with("DCIM/IMG_1234.HEIC"));
    assert_eq!(b.reason.as_deref(), Some("original format (HEIC)"));
    // The earlier copy is listed as already exported, covering the group.
    assert!(
        p.already_exported
            .iter()
            .any(|a| a.covers_group && a.file == u.jpg.file)
    );
}

#[test]
fn scenario_export_missing_earlier_copy_does_not_cover_the_group() {
    let mut u = jpeg_then_heic("better-missing", lightroom());
    let lib = u.s.library();
    fs::remove_file(lib.join("2019/2019-07-04/IMG_1234.JPG")).expect("delete library copy");
    let (p, r) = run(&u.after, &lib, &mut u.ledger, lightroom());
    assert!(p.better_copies.is_empty());
    assert_eq!(p.totals.better_copies, 0);
    assert_eq!(r.copied, 1);
    // Filed by its own date, as any keeper not yet exported.
    assert_eq!(
        p.files().next().expect("planned").target,
        library_rel("2021/2021-01-02/IMG_1234.HEIC")
    );
}

#[test]
fn scenario_export_earlier_copy_changed_by_another_app_still_covers_the_group() {
    let mut u = jpeg_then_heic("better-changed", lightroom());
    let lib = u.s.library();
    let copy = lib.join("2019/2019-07-04/IMG_1234.JPG");
    let mut data = fs::read(&copy).expect("read");
    data.extend_from_slice(b"photo app metadata");
    fs::write(&copy, &data).expect("rewrite");
    let before = fs::read(&copy).expect("read");
    let (p, r) = run(&u.after, &lib, &mut u.ledger, lightroom());
    assert_eq!(r.copied, 0);
    assert_eq!(p.totals.files, 0);
    assert_eq!(p.modified.len(), 1, "{p:#?}");
    assert_eq!(p.modified[0].file, u.jpg.file);
    assert!(matches!(
        p.modified[0].check,
        CopyCheck::SizeMismatch { .. }
    ));
    assert_eq!(p.better_copies.len(), 1);
    assert!(matches!(
        p.better_copies[0].library[0].check,
        CopyCheck::SizeMismatch { .. }
    ));
    assert_eq!(fs::read(&copy).expect("read"), before);
}

/// Counts content reads of the library (`identical` re-hashes); stats are free.
struct CountingProbe {
    inner: FsLibrary,
    reads: Cell<u64>,
}

impl LibraryProbe for CountingProbe {
    fn existing(&self, rel: &RelPath) -> std::io::Result<ps_export::Existing> {
        self.inner.existing(rel)
    }

    fn identical(&self, rel: &RelPath, content: &ContentId, size: u64) -> std::io::Result<bool> {
        self.reads.set(self.reads.get() + 1);
        self.inner.identical(rel, content, size)
    }

    fn free_space(&self) -> std::io::Result<Option<u64>> {
        self.inner.free_space()
    }
}

#[test]
fn scenario_export_covered_group_re_planned_without_reading_the_library() {
    // Three groups, each exported through its JPEG, each now kept by a HEIC.
    let mut s = Sources::new("better-replan", 1);
    let mut before = Vec::new();
    let mut after = Vec::new();
    for i in 0..3u64 {
        let jpg = s.add(
            0,
            &format!("a/IMG_{i}.JPG"),
            &bytes(10 + i, 2000),
            0,
            day(2019, 1, 1 + i as u8),
        );
        let heic = s.add(
            0,
            &format!("b/IMG_{i}.HEIC"),
            &bytes(20 + i, 5000),
            0,
            day(2019, 1, 1 + i as u8),
        );
        before.push(group_unit(i as i64 + 1, vec![jpg.clone()], &[]));
        after.push(group_unit(i as i64 + 1, vec![heic], &[&jpg]));
    }
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    run(&s.input(before), &lib, &mut ledger, google());
    let after = s.input(after);
    let rows = ledger.entries().expect("rows");
    let probe = CountingProbe {
        inner: FsLibrary::new(&lib),
        reads: Cell::new(0),
    };
    let p = plan(&after, &rows, &lib, &probe, google()).expect("plan");
    assert_eq!(probe.reads.get(), 0, "no library file read without verify");
    assert_eq!(p.totals.files, 0);
    assert_eq!(p.totals.better_copies, 3);
    assert_eq!(
        p.already_exported.iter().filter(|a| a.covers_group).count(),
        3
    );
    let p = plan(&after, &rows, &lib, &probe, google().with_verify(true)).expect("plan");
    assert_eq!(probe.reads.get(), 3, "verify re-hashes each earlier copy");
    assert_eq!(p.totals.better_copies, 3);
    assert_eq!(p.totals.files, 0);
}

#[test]
fn scenario_export_keeper_set_completed_beside_its_exported_file() {
    let mut s = Sources::new("better-set", 1);
    let jpg = s.add(0, "DCIM/IMG_7.JPG", &bytes(30, 3000), 0, day(2018, 3, 3));
    let raw = s.add(0, "DCIM/IMG_7.DNG", &bytes(31, 9000), 0, day(2020, 3, 3));
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    run(
        &s.input(vec![group_unit(1, vec![jpg.clone()], &[])]),
        &lib,
        &mut ledger,
        lightroom(),
    );
    // The RAW joins: a RAW+JPEG keeper set whose JPEG is in the library.
    let after = s.input(vec![group_unit(1, vec![raw.clone(), jpg.clone()], &[])]);
    let (p, r) = run(&after, &lib, &mut ledger, lightroom());
    assert!(p.better_copies.is_empty());
    assert_eq!(r.copied, 1);
    let f = p.files().next().expect("raw planned");
    assert_eq!(f.file, raw.file);
    assert_eq!(f.target, library_rel("2018/2018-03-03/IMG_7.DNG"));
}

#[test]
fn scenario_export_upgrade_copies_the_better_copy_beside_the_earlier_copy() {
    let mut u = jpeg_then_heic("better-upgrade", lightroom());
    let lib = u.s.library();
    let jpg_copy = lib.join("2019/2019-07-04/IMG_1234.JPG");
    let xmp = lib.join("2019/2019-07-04/IMG_1234.xmp");
    let jpg_before = fs::read(&jpg_copy).expect("jpg");
    let xmp_before = fs::read(&xmp).expect("the JPEG's sidecar");
    let jpg_mtime = mtime_ns(&jpg_copy);
    let (p, r) = run(
        &u.after,
        &lib,
        &mut u.ledger,
        lightroom().with_upgrade(true),
    );
    assert_eq!(r.copied, 1);
    assert_eq!(
        p.totals.better_copies, 1,
        "listed in the upgrading plan too"
    );
    let f = p.files().next().expect("heic planned");
    assert_eq!(f.action, PlannedAction::Copy);
    assert_eq!(f.target, library_rel("2019/2019-07-04/IMG_1234.HEIC"));
    assert_eq!(
        fs::read(lib.join("2019/2019-07-04/IMG_1234.HEIC")).expect("heic"),
        bytes(2, 9000)
    );
    assert_eq!(fs::read(&jpg_copy).expect("jpg"), jpg_before);
    assert_eq!(mtime_ns(&jpg_copy), jpg_mtime);
    assert_eq!(
        fs::read(&xmp).expect("xmp"),
        xmp_before,
        "the sidecar is shared"
    );
    assert_eq!(r.sidecars, 0);
    let names: Vec<_> = fs::read_dir(lib.join("2019/2019-07-04"))
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.contains("(2)")), "{names:?}");
    assert!(
        !lib.join("2021").exists(),
        "not filed by the HEIC's own date"
    );
    // The manifest names the earlier copy.
    let m = ps_export::manifest::read_manifest(&lib).expect("manifest");
    let line = m
        .iter()
        .find(|l| l.library_path.ends_with("IMG_1234.HEIC"))
        .expect("heic line");
    assert_eq!(line.better_copy_of, vec!["2019/2019-07-04/IMG_1234.JPG"]);
    // The ledger row of the earlier copy is still done.
    assert!(
        u.ledger
            .get(&library_rel("2019/2019-07-04/IMG_1234.JPG"))
            .is_some_and(|e| e.is_exported_file())
    );
    // The next plan lists no better copy and copies nothing.
    let next = plan_only(&u.after, &lib, &u.ledger, lightroom());
    assert!(next.better_copies.is_empty());
    assert_eq!(next.totals.files, 0);
    let next = plan_only(&u.after, &lib, &u.ledger, lightroom().with_upgrade(true));
    assert_eq!(next.totals.files, 0);
}

fn live_file(s: &mut Sources, src: usize, path: &str, seed: u64, len: usize) -> KeeperFile {
    let mut f = s.add(src, path, &bytes(seed, len), 0, day(2022, 6, 1));
    f.content_identifier = Some("CID-LIVE-1".into());
    f
}

#[test]
fn scenario_export_live_photo_upgrade_keeps_the_pair() {
    for taken in [false, true] {
        let tag = if taken { "live-taken" } else { "live-free" };
        let mut s = Sources::new(tag, 1);
        let jpg = live_file(&mut s, 0, "DCIM/IMG_1.JPG", 40, 2000);
        let mov = live_file(&mut s, 0, "DCIM/IMG_1.MOV", 41, 7000);
        let heic = live_file(&mut s, 0, "DCIM/IMG_1.HEIC", 42, 5000);
        let lib = s.library();
        let mut ledger = MemoryLedger::new();
        let video_unit = || group_unit(2, vec![mov.clone()], &[]);
        let (p, _) = run(
            &s.input(vec![group_unit(1, vec![jpg.clone()], &[]), video_unit()]),
            &lib,
            &mut ledger,
            apple(),
        );
        let still_dir = p
            .files()
            .find(|f| f.file == mov.file)
            .expect("video")
            .target
            .parent()
            .expect("folder")
            .display()
            .into_owned();
        if taken {
            fs::write(lib.join(format!("{still_dir}/IMG_1.HEIC")), b"someone else").expect("w");
        }
        let after = s.input(vec![
            group_unit(1, vec![heic.clone()], &[&jpg]),
            video_unit(),
        ]);
        // Without upgrade: covered, nothing copied.
        let p0 = plan_only(&after, &lib, &ledger, apple());
        assert_eq!(p0.totals.files, 0, "{tag}");
        assert_eq!(p0.totals.better_copies, 1, "{tag}");
        let tree = snapshot(&lib, &[&lib.join(".photosquirrel")]);
        let (p, r) = run(&after, &lib, &mut ledger, apple().with_upgrade(true));
        let targets: Vec<String> = p.files().map(|f| f.target.display().into_owned()).collect();
        if taken {
            assert_eq!(
                targets,
                vec![
                    format!("{still_dir}/IMG_1 (2).HEIC"),
                    format!("{still_dir}/IMG_1 (2).MOV")
                ],
                "the pair is copied together under the next suffix"
            );
            assert_eq!(r.copied, 2);
        } else {
            assert_eq!(targets, vec![format!("{still_dir}/IMG_1.HEIC")]);
            assert_eq!(r.copied, 1, "the video is not copied again");
        }
        // Every earlier file is byte-identical at its path, nothing removed.
        let now = snapshot(&lib, &[&lib.join(".photosquirrel")]);
        for (k, v) in &tree {
            assert_eq!(now.get(k), Some(v), "{tag}: {}", k.display());
        }
    }
}

#[test]
fn scenario_export_raw_jpeg_keeper_set_upgrade_keeps_stems() {
    let mut s = Sources::new("better-rawjpeg", 2);
    let small = s.add(0, "Web/IMG_5.JPG", &bytes(50, 1500), 0, day(2017, 5, 5));
    let raw = s.add(1, "DCIM/IMG_5.DNG", &bytes(51, 9000), 0, day(2017, 5, 5));
    let big = s.add(1, "DCIM/IMG_5.JPG", &bytes(52, 4000), 0, day(2017, 5, 5));
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    run(
        &s.input(vec![group_unit(1, vec![small.clone()], &[])]),
        &lib,
        &mut ledger,
        lightroom(),
    );
    let tree = snapshot(&lib, &[&lib.join(".photosquirrel")]);
    let after = s.input(vec![group_unit(
        1,
        vec![raw.clone(), big.clone()],
        &[&small],
    )]);
    let (p, r) = run(&after, &lib, &mut ledger, lightroom().with_upgrade(true));
    assert_eq!(r.copied, 2);
    let mut targets: Vec<String> = p.files().map(|f| f.target.display().into_owned()).collect();
    targets.sort();
    assert_eq!(
        targets,
        vec![
            "2017/2017-05-05/IMG_5 (2).DNG".to_string(),
            "2017/2017-05-05/IMG_5 (2).JPG".to_string()
        ]
    );
    let now = snapshot(&lib, &[&lib.join(".photosquirrel")]);
    for (k, v) in &tree {
        assert_eq!(now.get(k), Some(v), "{}", k.display());
    }
    assert_eq!(
        fs::read(lib.join("2017/2017-05-05/IMG_5.JPG")).expect("earlier"),
        bytes(50, 1500)
    );
}

fn facts_of(f: &KeeperFile) -> FileFacts {
    FileFacts {
        file: f.file,
        source: f.source,
        rel_path: f.rel_path.clone(),
        size: f.size,
        content: Some(f.content),
        class: Some(PhotoClass::Photo),
        unreadable: false,
        missing: false,
        user_excluded: false,
        quarantined: None,
    }
}

#[test]
fn scenario_completion_report_group_covered_by_an_earlier_copy_is_accounted_for() {
    let u = jpeg_then_heic("report-covered", google());
    let lib = u.s.library();
    // A third member: an exact copy of nothing in the library.
    let other = KeeperFile {
        file: FileId(99),
        content: ContentId::full([9; 32]),
        rel_path: rel("Web/IMG_1234 small.jpg"),
        ..u.jpg.clone()
    };
    let files = vec![facts_of(&u.heic), facts_of(&u.jpg), facts_of(&other)];
    let groups = vec![GroupFacts {
        members: vec![u.heic.file, u.jpg.file, other.file],
        keeper_set: vec![u.heic.file],
    }];
    let rows = u.ledger.entries().expect("rows");
    let checks = check_library(&lib, &rows, false);
    let d = derive_dispositions(&DispositionFacts {
        files: &files,
        groups: &groups,
        ledger: &rows,
        checks: &checks,
    });
    let jpg_path = Some(rel("2019/2019-07-04/IMG_1234.JPG"));
    let by: HashMap<FileId, _> = d.iter().map(|x| (x.file, x)).collect();
    assert_eq!(by[&u.jpg.file].disposition, Disposition::Exported);
    assert_eq!(by[&u.jpg.file].library_path, jpg_path);
    for f in [u.heic.file, other.file] {
        assert_eq!(by[&f].disposition, Disposition::Covered);
        assert_eq!(by[&f].library_path, jpg_path);
    }
    let rep = report(&files, &d, &checks);
    assert!(
        rep.sources
            .iter()
            .all(|s| s.badge == ps_export::Badge::FullyAccounted)
    );
    assert!(d.iter().all(|x| x.disposition != Disposition::Pending));
}

#[test]
fn scenario_completion_report_partly_exported_keeper_set_stays_pending() {
    let mut s = Sources::new("report-partial", 1);
    let jpg = s.add(0, "DCIM/IMG_8.JPG", &bytes(60, 3000), 0, day(2018, 3, 3));
    let raw = s.add(0, "DCIM/IMG_8.DNG", &bytes(61, 9000), 0, day(2018, 3, 3));
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    run(
        &s.input(vec![group_unit(1, vec![jpg.clone()], &[])]),
        &lib,
        &mut ledger,
        google(),
    );
    let files = vec![facts_of(&raw), facts_of(&jpg)];
    let groups = vec![GroupFacts {
        members: vec![raw.file, jpg.file],
        keeper_set: vec![raw.file, jpg.file],
    }];
    let rows = ledger.entries().expect("rows");
    let checks = check_library(&lib, &rows, false);
    let d = derive_dispositions(&DispositionFacts {
        files: &files,
        groups: &groups,
        ledger: &rows,
        checks: &checks,
    });
    for x in &d {
        assert_eq!(x.disposition, Disposition::Pending, "{x:?}");
    }
}
