//! Export through the service (export tasks 1.1, 5.1 and the 7.2 gate): the catalog-built input, the
//! catalog-backed ledger, dispositions derived from catalog facts, and completion reports.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ps_export::ExecConfig;
use ps_model::{Disposition, ExportProfile};
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn small_corpus() -> PathBuf {
    // Under CARGO_TARGET_TMPDIR, not the workspace's ./target, which need not exist (CARGO_TARGET_DIR elsewhere).
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("export-service-corpus-small-42");
    if !ps_fixtures::Truth::read(&root).is_ok_and(|t| t.seed == 42 && t.preset == "small") {
        let _ = std::fs::remove_dir_all(&root);
        let w = ps_fixtures::DirWriter::new(&root).expect("writer");
        ps_fixtures::generate(42, ps_fixtures::Preset::Small, &w).expect("generate");
    }
    root.canonicalize().expect("canonical corpus root")
}

fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.clone(), std::fs::metadata(&p).map_or(0, |m| m.len())));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn export_small_corpus_reexport_copies_nothing_and_every_drive_is_accounted_for() {
    let corpus = small_corpus();
    let before = tree(&corpus);
    let tmp = tempfile::tempdir().expect("tmp");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&corpus.join(d)).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("scan");
    let ex = svc.exports();
    let library = tmp.path().join("Library");
    let settings = profile_settings("lightroom").expect("profile");
    assert_eq!(settings.profile, ExportProfile::Lightroom);

    // Dry run: a plan, nothing written.
    let (plan, input) = ex.plan(&library, settings).expect("plan");
    assert!(plan.totals.files > 0);
    assert_eq!(
        input.units.len(),
        plan.bundles.iter().map(|b| b.units.len()).sum::<usize>()
    );
    assert!(!library.exists(), "the dry run writes nothing");

    let cancel = AtomicBool::new(false);
    let (_, report) = ex
        .run(&library, settings, &ExecConfig::default(), &cancel)
        .expect("export");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        report.copied + report.adopted,
        plan.totals.files + plan.totals.adopted
    );

    // Re-export: nothing new to copy (the ledger lives in the catalog).
    let (again, _) = ex.plan(&library, settings).expect("replan");
    assert_eq!(again.totals.files, 0, "{:?}", again.totals);
    assert_eq!(again.totals.already_exported, plan.totals.files);
    let (_, r2) = ex
        .run(&library, settings, &ExecConfig::default(), &cancel)
        .expect("re-export");
    assert_eq!(r2.copied, 0);

    // Every drive 100% accounted for, verified by re-hashing every library copy.
    let c = ex.completion(None, true).expect("completion");
    assert!(c.report.mismatches.is_empty());
    for s in &c.report.sources {
        assert!(
            s.counts.fully_accounted(),
            "source {} pending {} unreadable {}",
            s.source,
            s.counts.files_of(Disposition::Pending),
            s.counts.files_of(Disposition::Unreadable)
        );
    }
    assert!(c.totals.files_of(Disposition::Exported) > 0);
    assert!(c.totals.files_of(Disposition::Covered) > 0);
    assert!(c.totals.files_of(Disposition::Excluded) > 0);
    // Sources untouched.
    assert_eq!(tree(&corpus), before);

    // Deleting a library copy makes its files pending again (dispositions are derived, never stored).
    let (_, d, _) = ex.dispositions(&library, false).expect("dispositions");
    let exported = d
        .iter()
        .find(|x| x.disposition == Disposition::Exported)
        .and_then(|x| x.library_path.clone())
        .expect("an exported file");
    std::fs::remove_file(exported.under(&library)).expect("delete library copy");
    let c = ex.completion(Some(&library), false).expect("completion");
    assert!(c.totals.files_of(Disposition::Pending) >= 1);
    let pending = ex
        .files_with(&library, Disposition::Pending, 10)
        .expect("drill down");
    assert!(!pending.is_empty() && pending.iter().all(|p| p.path.starts_with(&corpus)));
}

/// ADR-0043: a keeper that lost its EXIF is filed by the capture date another copy in its group remembers, and
/// the plan and the manifest say so.
#[test]
fn scenario_export_stripped_keeper_filed_by_group_capture_date() {
    use ps_fixtures::media::{ExifSpec, encode_jpeg, exif_tiff, insert_exif, photo_rgb};

    let tmp = tempfile::tempdir().expect("tmp");
    let src = tmp.path().join("phone");
    std::fs::create_dir_all(src.join("chat")).expect("mkdir");
    let rgb = photo_rgb(1200, 900, 77);
    let pixels = encode_jpeg(&rgb, 1200, 900, 90);
    let spec = ExifSpec {
        date_time_original: Some("2023:06:26 10:15:00".into()),
        ..ExifSpec::canon_5d()
    };
    let original = insert_exif(&pixels, &exif_tiff(&spec));
    // The same picture re-encoded by a messaging app: no EXIF at all, modified in 2024.
    let stripped = encode_jpeg(&rgb, 1200, 900, 89);
    let orig_path = src.join("IMG_0042.JPG");
    let stripped_path = src.join("chat/photo_42.jpg");
    std::fs::write(&orig_path, &original).expect("write");
    std::fs::write(&stripped_path, &stripped).expect("write");
    // 2024-03-01T12:00:00Z
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_709_294_400);
    std::fs::File::options()
        .write(true)
        .open(&stripped_path)
        .and_then(|f| f.set_modified(t))
        .expect("mtime");

    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    svc.add_source(&src).expect("add");
    svc.start(&[]).expect("start");
    svc.wait().expect("scan");

    let groups = svc
        .groups()
        .list(ps_service::groups::GroupOrder::Id, 0, 10)
        .expect("groups");
    assert_eq!(groups.len(), 1, "the two copies form one group: {groups:?}");
    let detail = svc.groups().show(groups[0].id).expect("show");
    let member = |p: &Path| {
        detail
            .members
            .iter()
            .find(|m| {
                m.path
                    .ends_with(p.strip_prefix(&src).expect("under source"))
            })
            .unwrap_or_else(|| panic!("{} in {:?}", p.display(), detail.members))
            .file
    };
    let (stripped_id, orig_id) = (member(&stripped_path), member(&orig_path));
    // Keep the EXIF-less copy (as a user might, or as the automatic choice would for a larger stripped copy).
    let detail = svc
        .groups()
        .set_keeper(detail.id, stripped_id)
        .expect("set keeper");
    assert!(
        detail
            .members
            .iter()
            .any(|m| m.file == stripped_id && m.keeper)
    );

    let library = tmp.path().join("Library");
    let settings = profile_settings("apple").expect("profile");
    let (plan, _) = svc.exports().plan(&library, settings).expect("plan");
    let planned: Vec<_> = plan.files().collect();
    assert_eq!(planned.len(), 1, "{planned:?}");
    let f = planned[0];
    assert_eq!(f.file, stripped_id);
    assert_eq!(f.target.display(), "2023/2023-06-26/photo_42.jpg");
    assert_eq!(f.date_origin.copy, Some(orig_id));
    assert_eq!(
        f.date_origin.source,
        ps_export::DateSource::Embedded {
            provenance: ps_model::DateProvenance::DateTimeOriginal
        }
    );
    assert_eq!(f.date_origin.describe(), "DateTimeOriginal of another copy");
    assert_eq!(plan.totals.dates.other_copy, 1);
    assert!(
        plan.totals
            .dates
            .note()
            .is_some_and(|n| n.contains("1 filed by the date of another copy"))
    );

    let cancel = AtomicBool::new(false);
    let (_, report) = svc
        .exports()
        .run(&library, settings, &ExecConfig::default(), &cancel)
        .expect("export");
    assert_eq!(report.copied, 1, "{report:?}");
    assert!(library.join("2023/2023-06-26/photo_42.jpg").is_file());
    let manifest =
        std::fs::read_to_string(library.join(".photosquirrel/manifest.jsonl")).expect("manifest");
    let rec: ps_export::manifest::ManifestRecord =
        serde_json::from_str(manifest.lines().next().expect("a line")).expect("record");
    assert_eq!(
        rec.date_source.as_deref(),
        Some("DateTimeOriginal of another copy")
    );
    let from = rec.date_from.expect("date_from");
    assert!(from.ends_with("IMG_0042.JPG"), "{from}");
}

/// A registered export library, a folder inside it and a folder containing it are refused as sources (by
/// `sources add` and the Sources view, which both go through `add_source`), and a source cannot be re-linked
/// into it.
#[test]
fn scenario_export_library_refused_as_a_source() {
    let tmp = tempfile::tempdir().expect("tmp");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    let photos = tmp.path().join("Pictures");
    let library = photos.join("PhotoSquirrel Library");
    std::fs::create_dir_all(library.join("2020/2020-01-01")).expect("library");
    let elsewhere = tmp.path().join("Camera");
    std::fs::create_dir_all(&elsewhere).expect("source");
    std::fs::write(elsewhere.join("IMG_0001.JPG"), b"not really a jpeg").expect("file");
    svc.exports()
        .library_id(&library)
        .expect("register library");
    for refused in [
        library.clone(),
        library.join("2020"),
        photos.clone(),
        tmp.path().to_path_buf(),
    ] {
        let err = svc.add_source(&refused).expect_err("refused");
        assert!(
            matches!(err, ps_service::scan::ScanError::ExportLibrary { .. }),
            "{refused:?}: {err}"
        );
        assert!(err.to_string().contains("export library"), "{err}");
    }
    let added = svc
        .add_source(&elsewhere)
        .expect("a folder outside the library");
    assert!(svc.sources().expect("sources").len() == 1);
    let err = svc
        .relink_source(added.source.id, &library)
        .expect_err("re-link into the library refused");
    assert!(
        matches!(err, ps_service::scan::ScanError::ExportLibrary { .. }),
        "{err}"
    );
}

/// Sidecars through the real path (`Exports::input` from the catalog): a keeper whose stored AI result says
/// "pets" with a dog, and whose user correction adds the tag "Rex", gets a sidecar with all three as subjects;
/// a keeper with only a demo result gets no AI keywords at all.
#[test]
fn scenario_export_sidecar_contains_ai_keywords() {
    use ps_catalog::AiResultRow;
    use ps_service::insights::{Correction, DEMO_PROVIDER, InsightStore, sample_insight};

    let corpus = small_corpus();
    let tmp = tempfile::tempdir().expect("tmp");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    svc.add_source(&corpus.join("drive-a")).expect("add");
    svc.start(&[]).expect("start");
    svc.wait().expect("scan");
    let ex = svc.exports();
    let input = ex.input().expect("input");
    let singles: Vec<usize> = input
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| {
            u.files.len() == 1 && u.files[0].kind() == Some(ps_model::MediaKind::Image)
        })
        .map(|(i, _)| i)
        .collect();
    let (pets, demo) = (singles[0], singles[1]);
    let content = |u: usize| input.units[u].files[0].content;
    let catalog = svc.engine().catalog();
    let mut ai = sample_insight(1); // a dog: category pets, pets ["dog"]
    ai.tags = vec!["rug".into()];
    for (u, provider, insight) in [
        (pets, "lm-studio", ai.clone()),
        (demo, DEMO_PROVIDER, sample_insight(0)),
    ] {
        catalog
            .writer()
            .ai_complete(AiResultRow {
                content: content(u),
                provider: provider.into(),
                model: "m".into(),
                schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
                prompt_version: ps_llm::PROMPT_VERSION,
                insight: serde_json::to_string(&insight).expect("json"),
                latency_ms: 1,
                requests: 1,
                at_ms: 1,
            })
            .expect("result");
    }
    InsightStore::for_catalog(catalog)
        .correct(
            &content(pets).to_hex(),
            Correction {
                tags: Some(vec!["Rex".into(), "rug".into()]),
                ..Correction::default()
            },
        )
        .expect("correction");

    let library = tmp.path().join("Library");
    let settings = profile_settings("lightroom").expect("profile");
    let (plan, report) = ex
        .run(
            &library,
            settings,
            &ExecConfig::default(),
            &AtomicBool::new(false),
        )
        .expect("export");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let sidecar_of = |u: usize| {
        let b = plan
            .bundles
            .iter()
            .find(|b| b.units == [u])
            .expect("bundle");
        let rel = b.sidecar.as_ref().expect("sidecar planned");
        ps_export::xmp::parse(&std::fs::read(rel.under(&library)).expect("sidecar")).expect("valid")
    };
    let side = sidecar_of(pets);
    for k in ["pets", "dog", "Rex"] {
        assert!(
            side.subjects.iter().any(|s| s == k),
            "{k} in {:?}",
            side.subjects
        );
    }
    assert_eq!(side.content_hash, content(pets).to_hex());
    let side = sidecar_of(demo);
    let demo_words = sample_insight(0).keywords();
    assert!(
        side.subjects.iter().all(|s| !demo_words.contains(s)),
        "no demo keywords: {:?}",
        side.subjects
    );
}
