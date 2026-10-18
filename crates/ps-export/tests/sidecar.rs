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

#[test]
fn scenario_export_sidecar_contains_ai_keywords() {
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
