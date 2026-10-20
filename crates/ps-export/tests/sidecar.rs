//! Sidecars, manifest and profiles.

mod common;

use std::fs;
use std::sync::atomic::AtomicBool;

use common::*;
use ps_export::{
    CopyRef, ExecConfig, FsLibrary, Ledger, MemoryLedger, NoHooks, ProfileSettings, execute, plan,
    xmp,
};
use ps_model::{ExportProfile, ImageInsight, PeopleEstimate, SceneCategory};

fn export(
    input: &ps_export::ExportInput,
    lib: &std::path::Path,
    ledger: &mut MemoryLedger,
    settings: ProfileSettings,
) -> (ps_export::Plan, ps_export::ExportReport) {
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
    (p, r)
}

/// The sidecar writer on a hand-built unit (the scenario itself goes through the catalog in
/// `ps-service/tests/export_service.rs`).
#[test]
fn sidecar_lists_insight_keywords_user_tags_and_provenance() {
    let mut s = Sources::new("xmp", 1);
    let f = s.add(0, "Pets/IMG_3000.JPG", &bytes(70, 2000), 0, day(2020, 5, 5));
    let mut u = unit(vec![f.clone()]);
    u.insight = Some(ImageInsight {
        subject: "A dog on the lawn".into(),
        category: SceneCategory::Pets,
        also: vec![],
        is_photo: true,
        people: PeopleEstimate::default(),
        pets: vec!["dog".into()],
        tags: vec!["lawn".into()],
        setting: Some("outdoor garden".into()),
        confidence: 0.9,
    });
    u.user_tags = vec!["Rex".into()];
    let input = s.input(vec![u]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let (_, r) = export(
        &input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::Lightroom),
    );
    assert_eq!((r.copied, r.sidecars), (1, 1));
    let bytes = fs::read(lib.join("2020/2020-05-05/IMG_3000.xmp")).expect("sidecar");
    let side = xmp::parse(&bytes).expect("valid XMP");
    for k in ["pets", "dog", "Rex", "1 pet"] {
        assert!(
            side.subjects.iter().any(|x| x == k),
            "{k} in {:?}",
            side.subjects
        );
    }
    assert_eq!(side.content_hash, f.content.to_hex());
    assert_eq!(side.description.as_deref(), Some("A dog on the lawn"));
    assert_eq!(side.pets, Some(1));
    assert!(
        side.sources
            .iter()
            .any(|p| p.ends_with("Pets/IMG_3000.JPG"))
    );
}

#[test]
fn scenario_export_manifest_record_per_file() {
    let mut s = Sources::new("manifest", 2);
    let mut units = Vec::new();
    for i in 0..10 {
        let data = bytes(80 + i, 1000);
        let k = s.add(0, &format!("IMG_{i}.JPG"), &data, 0, day(2021, 3, 3));
        let dup = s.add(1, &format!("Backup/IMG_{i}.JPG"), &data, 0, day(2021, 3, 3));
        let mut u = unit(vec![k]);
        u.copies.push(CopyRef {
            file: dup.file,
            source: dup.source,
            rel_path: dup.rel_path.clone(),
        });
        units.push(u);
    }
    let input = s.input(units);
    let lib = s.library();
    // A previous line must stay.
    fs::create_dir_all(lib.join(".photosquirrel")).expect("meta");
    let mut ledger = MemoryLedger::new();
    export(
        &input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::GooglePhotos),
    );
    let text = fs::read_to_string(lib.join(".photosquirrel/manifest.jsonl")).expect("manifest");
    assert_eq!(text.lines().count(), 10);
    let recs = ps_export::manifest::read_manifest(&lib).expect("parse");
    for (r, u) in recs.iter().zip(&input.units) {
        assert_eq!(r.source_paths.len(), 2, "{r:?}");
        let _ = u;
        assert!(r.source_paths.iter().any(|p| p.contains("drive-1/Backup/")));
        assert!(r.source_paths.iter().any(|p| p.contains("drive-0/IMG_")));
        assert_eq!(r.full_hash.len(), 64);
        assert_eq!(r.profile, ExportProfile::GooglePhotos);
        assert!(!r.keeper_reasons.is_empty());
    }
    // Appended, never rewritten: a second run with nothing new adds nothing.
    export(
        &input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::GooglePhotos),
    );
    assert_eq!(
        fs::read_to_string(lib.join(".photosquirrel/manifest.jsonl")).expect("m"),
        text
    );
}

#[test]
fn scenario_export_google_profile_omits_sidecars() {
    let c = small_corpus();
    let m = corpus_model(c, &[], &[]);
    let d = scratch("google");
    let lib = d.path().join("library");
    let mut ledger = MemoryLedger::new();
    let (_, r) = export(
        &m.input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::GooglePhotos),
    );
    assert!(r.copied > 50);
    assert_eq!(r.sidecars, 0);
    assert!(
        files_under(&lib)
            .iter()
            .all(|p| p.extension().is_none_or(|e| e != "xmp"))
    );
}

#[test]
fn scenario_export_lightroom_profile_keeps_raw_pairs() {
    let c = small_corpus();
    let m = corpus_model(c, &[], &[]);
    let pairs = m.input.units.iter().filter(|u| u.files.len() == 2).count();
    assert!(pairs > 0, "the small corpus has RAW+JPEG keeper sets");
    let d = scratch("lightroom");
    let lib = d.path().join("library");
    let mut ledger = MemoryLedger::new();
    let (p, r) = export(
        &m.input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::Lightroom),
    );
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    for b in p
        .bundles
        .iter()
        .filter(|b| b.kind == ps_export::BundleKind::KeeperSet)
    {
        assert_eq!(b.files.len(), 2);
        let stems: Vec<_> = b
            .files
            .iter()
            .map(|f| {
                ps_export::layout::split_name(f.target.file_name())
                    .0
                    .to_vec()
            })
            .collect();
        assert_eq!(stems[0].to_ascii_lowercase(), stems[1].to_ascii_lowercase());
        for f in &b.files {
            assert!(f.target.under(&lib).is_file());
        }
        let side = b.sidecar.as_ref().expect("sidecar planned");
        xmp::parse(&fs::read(side.under(&lib)).expect("sidecar")).expect("valid");
    }
    assert_eq!(
        p.bundles
            .iter()
            .filter(|b| b.kind == ps_export::BundleKind::KeeperSet)
            .count(),
        pairs
    );
}

// ---- sidecars in the ledger (ADR-0045) ----

const SIDECAR_KILL_ENV: &str = "PS_EXPORT_SIDECAR_KILL_DIR";

/// Kills the process when the first sidecar is about to be renamed into place.
struct AbortBeforeSidecarRename;

impl ps_export::CopyHooks for AbortBeforeSidecarRename {
    fn before_rename(&self, _f: ps_model::FileId, partial: &std::path::Path) {
        if partial.to_string_lossy().ends_with(".xmp.ps-partial") {
            std::process::abort();
        }
    }
}

/// The same three-photo input in the parent and the child.
fn sidecar_kill_input(dir: &std::path::Path) -> ps_export::ExportInput {
    let root = dir.join("src");
    let src = ps_export::SourceInfo {
        id: ps_model::SourceId(1),
        root: root.clone(),
        online: true,
        name: "drive-s".into(),
    };
    let units = (0..3)
        .map(|i| {
            let name = format!("IMG_{i}.JPG");
            let p = root.join(&name);
            let data = fs::read(&p).expect("read");
            unit(vec![ps_export::KeeperFile {
                file: ps_model::FileId(i + 1),
                source: src.id,
                rel_path: rel(&name),
                size: data.len() as u64,
                mtime_ns: mtime_ns(&p),
                content: ps_hash::identity_from_buffer(&data),
                format: format_of(&name),
                capture: day(2018, 4, 4),
                undated: false,
                date_origin: ps_export::DateOrigin::OWN_EXIF,
                content_identifier: None,
                library_name: None,
            }])
        })
        .collect();
    ps_export::ExportInput {
        sources: vec![src],
        units,
    }
}

fn lightroom_run(
    input: &ps_export::ExportInput,
    lib: &std::path::Path,
    ledger: &mut dyn Ledger,
    hooks: &dyn ps_export::CopyHooks,
) -> ps_export::ExportReport {
    let rows = ledger.entries().expect("rows");
    let settings = ProfileSettings::defaults(ExportProfile::Lightroom);
    let p = plan(input, &rows, lib, &FsLibrary::new(lib), settings).expect("plan");
    execute(
        &p,
        input,
        ledger,
        &ExecConfig::default(),
        hooks,
        &AtomicBool::new(false),
    )
    .expect("export")
}

/// A kill while a sidecar is being written leaves its partial and an `in_progress` sidecar row; the next run
/// removes the partial and writes the sidecar, so the resumed export is complete with every `.xmp`.
#[test]
fn scenario_export_interrupted_sidecar_is_finished_on_resume() {
    if let Some(dir) = std::env::var_os(SIDECAR_KILL_ENV) {
        let dir = std::path::PathBuf::from(dir);
        let input = sidecar_kill_input(&dir);
        let mut ledger = FileLedger::new(dir.join("ledger.jsonl"));
        lightroom_run(
            &input,
            &dir.join("library"),
            &mut ledger,
            &AbortBeforeSidecarRename,
        );
        unreachable!("the child is killed before the first sidecar rename");
    }
    let tmp = scratch("sidecar-kill");
    let dir = tmp.path();
    fs::create_dir(dir.join("src")).expect("src");
    for i in 0..3u64 {
        let p = dir.join("src").join(format!("IMG_{i}.JPG"));
        fs::write(&p, bytes(500 + i, 30_000)).expect("write");
        set_mtime(&p, 1_520_000_000);
    }
    let status = std::process::Command::new(std::env::current_exe().expect("exe"))
        .args([
            "scenario_export_interrupted_sidecar_is_finished_on_resume",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SIDECAR_KILL_ENV, dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn child");
    assert!(!status.success(), "child was killed");
    let lib = dir.join("library");
    let partials = partials_under(&lib);
    assert_eq!(partials.len(), 1, "{partials:?}");
    assert!(partials[0].to_string_lossy().ends_with(".xmp.ps-partial"));

    let input = sidecar_kill_input(dir);
    let mut ledger = FileLedger::new(dir.join("ledger.jsonl"));
    let r = lightroom_run(&input, &lib, &mut ledger, &NoHooks);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.stale_partials_removed, 1);
    assert!(partials_under(&lib).is_empty());
    for i in 0..3 {
        let side = lib.join(format!("2018/2018-04-04/IMG_{i}.xmp"));
        xmp::parse(&fs::read(&side).expect("sidecar written")).expect("valid XMP");
        assert!(lib.join(format!("2018/2018-04-04/IMG_{i}.JPG")).is_file());
    }
    assert_eq!(library_by_hash(&lib).len(), 3, "each photo exactly once");
    let rows = ledger.entries().expect("rows");
    let sidecars = rows
        .iter()
        .filter(|e| e.kind == ps_model::LedgerKind::Sidecar)
        .filter(|e| e.state == ps_model::LedgerState::Done)
        .count();
    assert_eq!(sidecars, 3);
    // Nothing is left to do.
    let again = lightroom_run(&input, &lib, &mut ledger, &NoHooks);
    assert_eq!((again.copied, again.adopted, again.sidecars), (0, 0, 0));
}

/// A library copy the user deleted is copied again under its original name: its sidecar, which the ledger
/// records as ours, does not push the photo to "IMG (2).JPG", and is left as it is.
#[test]
fn scenario_export_deleted_copy_restored_to_its_name() {
    let mut s = Sources::new("restore", 1);
    let f = s.add(
        0,
        "DCIM/IMG_4000.JPG",
        &bytes(77, 9_000),
        0,
        day(2021, 1, 2),
    );
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let settings = ProfileSettings::defaults(ExportProfile::ApplePhotos);
    let (_, r) = export(&input, &lib, &mut ledger, settings);
    assert_eq!((r.copied, r.sidecars), (1, 1));
    let photo = lib.join("2021/2021-01-02/IMG_4000.JPG");
    let side = lib.join("2021/2021-01-02/IMG_4000.xmp");
    let side_before = fs::read(&side).expect("sidecar");
    fs::remove_file(&photo).expect("the user deletes the library copy");
    let (p, r) = export(&input, &lib, &mut ledger, settings);
    assert_eq!(p.totals.collisions, 0, "no suffix");
    assert_eq!((r.copied, r.sidecars), (1, 0));
    assert!(photo.is_file());
    assert!(!lib.join("2021/2021-01-02/IMG_4000 (2).JPG").exists());
    assert_eq!(fs::read(&side).expect("kept"), side_before);
}

/// Sidecars turned on for a library first exported without them: each exported bundle gets its sidecar
/// next to its copy, and nothing is copied again.
#[test]
fn sidecars_are_added_to_an_exported_library() {
    let mut s = Sources::new("add-sidecars", 1);
    let f = s.add(
        0,
        "DCIM/IMG_4100.JPG",
        &bytes(78, 9_000),
        0,
        day(2021, 1, 3),
    );
    let input = s.input(vec![unit(vec![f])]);
    let lib = s.library();
    let mut ledger = MemoryLedger::new();
    let (_, r) = export(
        &input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::GooglePhotos),
    );
    assert_eq!((r.copied, r.sidecars), (1, 0));
    let (p, r) = export(
        &input,
        &lib,
        &mut ledger,
        ProfileSettings::defaults(ExportProfile::ApplePhotos),
    );
    assert_eq!((p.totals.files, p.totals.sidecars), (0, 1));
    assert_eq!((r.copied, r.sidecars), (0, 1));
    let side = xmp::parse(&fs::read(lib.join("2021/2021-01-03/IMG_4100.xmp")).expect("sidecar"))
        .expect("valid");
    assert!(side.files[0].starts_with("IMG_4100.JPG "));
}
