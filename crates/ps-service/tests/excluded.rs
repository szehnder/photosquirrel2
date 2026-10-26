//! The Excluded view's data through the service (excluded-view-filters, ADR-0102): stored categories on a scanned
//! corpus after scans, re-classification and analysis refresh; old catalogs migrated without reading files; put
//! back for classifier and AI exclusions, and an AI run after a put back.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ps_catalog::{Catalog, UserDecision, db};
use ps_fixtures::{DirWriter, Params, Preset};
use ps_llm::queue::{PreviewLoader, QueueConfig};
use ps_llm::testing::{MockServer, Reply, insight_json, openai_reply, preview};
use ps_llm::{PRESETS, PreviewError, PreviewJpeg, ProviderConfig, RetryPolicy};
use ps_model::{
    AiKind, ClassReason, ContentId, ExclusionCategory, ExclusionSelection, FileId, Filter, ItemId,
    PhotoClass, Sort, SortField,
};
use ps_scan::SourceFs;
use ps_service::ai::AiService;
use ps_service::insights::{self, InsightFilter, SuggestionStatus};
use ps_service::library::Library;
use ps_service::scan::{ScanOptions, ScanService};

fn tiny() -> Params {
    Params {
        standalone: 8,
        sets_per_kind: 2,
        burst_sets: 1,
        videos: 1,
        scans: 1,
        icons: 2,
        ui_graphics: 2,
        tiny_web: 3,
        junk_videos: 1,
        raw_share: 0.0,
        dim_scale: (1, 2),
        twelve_mp: 0,
        backups: (1, 1),
    }
}

/// Files PhotoSquirrel cannot read at all, and one TIFF whose directory is cut off.
const UNSUPPORTED: &[&str] = &["notes.txt", "data.bin", "archive.dat"];
const BROKEN_TIFF: &str = "scan-broken.tif";

fn write_extras(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for (i, name) in UNSUPPORTED.iter().enumerate() {
        let bytes: Vec<u8> = (0..4096u32)
            .map(|b| (b * 31 + i as u32 * 7) as u8)
            .collect();
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    // A little-endian TIFF signature whose first directory lies past the end of the file.
    let mut tiff = b"II*\x00".to_vec();
    tiff.extend_from_slice(&0x00ff_ff00u32.to_le_bytes());
    tiff.extend_from_slice(&[0u8; 64]);
    std::fs::write(dir.join(BROKEN_TIFF), tiff).unwrap();
}

/// A tiny corpus plus the extras in `drive-a/Downloads`, scanned with `opts`.
fn corpus(dir: &Path, opts: ScanOptions) -> ScanService {
    let root = dir.join("corpus");
    ps_fixtures::generate_with(3, Preset::Small, tiny(), &DirWriter::new(&root).unwrap()).unwrap();
    write_extras(&root.join("drive-a/Downloads"));
    let svc = ScanService::open(&dir.join("data"), opts).unwrap();
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).unwrap();
    }
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    svc
}

fn rescan(svc: &ScanService) {
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
}

fn stored(
    cat: &Catalog,
) -> Vec<(
    FileId,
    Option<PhotoClass>,
    Option<String>,
    Option<String>,
    bool,
)> {
    cat.read()
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, class, exclusion, json_extract(reasons, '$[0]'), format IS NOT NULL FROM file",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    FileId(r.get(0)?),
                    r.get::<_, Option<i64>>(1)?
                        .and_then(|c| PhotoClass::from_code(c as u8)),
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap()
}

/// Every row's stored category is the one its class, first reason and format give.
fn assert_categories_derive(cat: &Catalog, when: &str) -> usize {
    let rows = stored(cat);
    let mut excluded = 0;
    for (id, class, exclusion, first, known) in &rows {
        let want =
            ExclusionCategory::of(*class, first.as_deref(), *known).map(ExclusionCategory::key);
        assert_eq!(
            exclusion.as_deref(),
            want,
            "{when}: file {id:?} ({class:?}, {first:?})"
        );
        excluded += usize::from(want.is_some());
    }
    assert!(excluded > 0, "{when}: the corpus has excluded files");
    excluded
}

fn category_of(cat: &Catalog, name: &str) -> Option<ExclusionCategory> {
    let id = cat
        .read()
        .with(|c| {
            Ok(c.query_row(
                "SELECT id FROM file WHERE name = ?1",
                [name.as_bytes()],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    cat.read()
        .exclusion_of(&[FileId(id)])
        .unwrap()
        .get(&FileId(id))
        .map(|x| x.0)
}

#[test]
fn stored_category_matches_the_rule_after_scan_reclassify_and_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = corpus(tmp.path(), ScanOptions::default());
    let cat = svc.engine().catalog().clone();
    let before = assert_categories_derive(&cat, "after the scan");
    drop(cat);
    drop(svc);
    // A lower minimum side re-classifies the small web images from their stored facts.
    let mut opts = ScanOptions::default();
    opts.classifier.min_side_px = 20;
    opts.classifier.min_pixels = 400;
    let svc = ScanService::open(&tmp.path().join("data"), opts).unwrap();
    rescan(&svc);
    let cat = svc.engine().catalog().clone();
    let after = assert_categories_derive(&cat, "after re-classification");
    assert!(
        after < before,
        "re-classification took files out ({before} -> {after})"
    );
    // Rows from before stored class facts: an analysis refresh probes them again and writes the class.
    cat.writer().flush().unwrap();
    {
        let c = rusqlite::Connection::open(cat.path()).unwrap();
        // Analysed rows (the ones a refresh re-reads), with their category cleared to see it rewritten.
        let n = c
            .execute(
                "UPDATE file SET class_facts = NULL, classifier_hash = NULL, exclusion = NULL WHERE stage IN (2, 3) AND exclusion IS NOT NULL",
                [],
            )
            .unwrap();
        assert!(n > 0, "analysed excluded rows (caches, graphics)");
        c.execute(
            "UPDATE file SET class_facts = NULL, classifier_hash = NULL WHERE stage IN (2, 3)",
            [],
        )
        .unwrap();
    }
    svc.start_refreshing_analysis(&[]).unwrap();
    svc.wait().unwrap();
    assert_categories_derive(&cat, "after an analysis refresh");
}

#[test]
fn scenario_catalog_recognised_format_that_cannot_be_read_is_not_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = corpus(tmp.path(), ScanOptions::default());
    let cat = svc.engine().catalog();
    let tiff = cat
        .read()
        .with(|c| {
            Ok(c.query_row(
                "SELECT format, class, reasons FROM file WHERE name = ?1",
                [BROKEN_TIFF.as_bytes()],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?)
        })
        .unwrap();
    assert_eq!(
        tiff.0.as_deref(),
        Some("tiff"),
        "a TIFF signature: {tiff:?}"
    );
    assert_eq!(tiff.1, i64::from(PhotoClass::Junk.code()));
    assert_eq!(
        category_of(cat, BROKEN_TIFF),
        Some(ExclusionCategory::Unreadable)
    );
    for name in UNSUPPORTED {
        assert_eq!(
            category_of(cat, name),
            Some(ExclusionCategory::Unsupported),
            "{name}"
        );
    }
}

#[test]
fn scenario_catalog_category_follows_re_classification() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let root = tmp.path().join("source");
    std::fs::create_dir_all(&root).unwrap();
    // 150 px on its shorter side: too small under the default 200 px minimum.
    std::fs::write(
        root.join("narrow.jpg"),
        ps_fixtures::media::jpeg(150, 1000, 9, 90),
    )
    .unwrap();
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    svc.add_source(&root).unwrap();
    rescan(&svc);
    let small = ExclusionSelection::only(ExclusionCategory::Small);
    let listed = |svc: &ScanService, sel: ExclusionSelection| {
        svc.engine()
            .catalog()
            .read()
            .id_list(
                &Filter {
                    exclusion: Some(sel),
                    ..Filter::default()
                },
                &Sort::by(SortField::FileName),
            )
            .unwrap()
            .len()
    };
    assert_eq!(
        category_of(svc.engine().catalog(), "narrow.jpg"),
        Some(ExclusionCategory::Small)
    );
    assert_eq!(listed(&svc, small.clone()), 1);
    drop(svc);
    // The user lowers the minimum side setting; the next scan re-classifies it as a likely photo.
    let mut opts = ScanOptions::default();
    opts.classifier.min_side_px = 100;
    let svc = ScanService::open(&data, opts).unwrap();
    rescan(&svc);
    let cat = svc.engine().catalog();
    assert_eq!(category_of(cat, "narrow.jpg"), None);
    let stored: Option<String> = cat
        .read()
        .with(|c| Ok(c.query_row("SELECT exclusion FROM file", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(stored, None, "the stored category is cleared");
    assert_eq!(
        listed(&svc, ExclusionSelection::all()),
        0,
        "not in the Excluded view"
    );
}

#[test]
fn scenario_catalog_old_catalog_gets_categories() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let path = data.join(ps_catalog::CATALOG_FILE);
    db::create_catalog_at_version(&path, db::EXCLUSION_CATEGORY_MIGRATION.version - 1).unwrap();
    // A real source folder whose files the migration must not read.
    let root = tmp.path().join("source");
    write_extras(&root);
    for i in 0..4 {
        std::fs::write(
            root.join(format!("IMG_{i}.jpg")),
            ps_fixtures::media::jpeg(800, 600, i, 90),
        )
        .unwrap();
    }
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, ?1, 100)",
            [ps_model::path_to_bytes(&root)],
        )
        .unwrap();
        let mut id = 0;
        let mut row = |name: &str, class: PhotoClass, format: Option<&str>, reason: ClassReason| {
            id += 1;
            let size = std::fs::metadata(root.join(name)).unwrap().len() as i64;
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format, kflags)
                 VALUES (?1, 1, ?2, ?2, ?3, 5, 1, ?4, ?5, ?6, ?4)",
                rusqlite::params![
                    id,
                    name.as_bytes(),
                    size,
                    class.code(),
                    serde_json::to_string(&[reason.to_string()]).unwrap(),
                    format
                ],
            )
            .unwrap();
            id
        };
        let mut ai = Vec::new();
        for i in 0..4 {
            ai.push(row(
                &format!("IMG_{i}.jpg"),
                PhotoClass::LikelyPhoto,
                Some("jpeg"),
                ClassReason::NoCameraMetadata,
            ));
        }
        for name in UNSUPPORTED {
            row(
                name,
                PhotoClass::Junk,
                None,
                ClassReason::Unsupported {
                    reason: "unrecognised content".into(),
                },
            );
        }
        for (file, label) in ai
            .iter()
            .zip(["screenshot", "document", "graphic", "not a photo"])
        {
            c.execute(
                "INSERT INTO user_inclusion (file_id, decision, reason, at) VALUES (?1, 'exclude', ?2, 1)",
                rusqlite::params![file, format!("{label} (AI suggestion)")],
            )
            .unwrap();
        }
    }
    // Opened through the service with a file layer that counts every open and every byte read.
    let (fs, inst) = SourceFs::instrumented();
    let svc = ScanService::open(
        &data,
        ScanOptions {
            fs,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    let cat = svc.engine().catalog();
    assert_eq!(
        cat.read().schema_version().unwrap(),
        db::EXCLUSION_CATEGORY_MIGRATION.version
    );
    let counts = cat.read().exclusion_counts(&Filter::default()).unwrap();
    assert_eq!(counts.of(ExclusionCategory::Unsupported), 3);
    assert_eq!(counts.of(ExclusionCategory::Ai), 4);
    for k in AiKind::ALL {
        assert_eq!(counts.ai(k), 1, "{k:?}");
    }
    let kinds: Vec<Option<AiKind>> = cat
        .read()
        .user_decisions()
        .unwrap()
        .into_iter()
        .map(|d| d.ai_kind)
        .collect();
    assert_eq!(kinds, AiKind::ALL.map(Some).to_vec());
    assert_eq!(inst.open_count(), 0, "{:?}", inst.opens());
    assert_eq!(inst.total_bytes(), 0);
}

fn library(svc: ScanService, dir: &Path) -> Library {
    Library::with_service(&dir.join("data"), svc)
}

fn reason_of(lib: &Library, id: ItemId) -> String {
    lib.detail(id).unwrap().unwrap().reasons[0].clone()
}

fn in_library(lib: &Library, id: ItemId) -> bool {
    lib.id_list(&ps_model::library::GalleryQuery {
        filter: ps_service::library::library_filter(),
        ..Default::default()
    })
    .unwrap()
    .ids
    .contains(&id)
}

#[test]
fn scenario_gallery_put_back_a_classifier_exclusion() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = library(corpus(tmp.path(), ScanOptions::default()), tmp.path());
    let caches = Filter {
        exclusion: Some(ExclusionSelection::only(ExclusionCategory::Cache)),
        ..Filter::default()
    };
    let (counts, list) = lib
        .excluded_open(&caches, &Sort::by(SortField::FileName))
        .unwrap();
    let before = counts.of(ExclusionCategory::Cache);
    assert!(list.len() >= 2, "the corpus has thumbnail caches");
    let two = vec![list.ids[0], list.ids[1]];
    assert_eq!(lib.put_back(&two).unwrap(), 2);
    for id in &two {
        assert!(in_library(&lib, *id));
        assert_eq!(reason_of(&lib, *id), "included by you");
    }
    let after = lib.exclusion_counts(&Filter::default()).unwrap();
    assert_eq!(after.of(ExclusionCategory::Cache), before - 2);
    // Rescans respect it, and excluding again undoes it.
    rescan(lib.scan().unwrap());
    assert!(two.iter().all(|id| in_library(&lib, *id)));
    lib.set_decision(two[0], Some(ps_model::library::Decision::Exclude), None)
        .unwrap();
    assert!(!in_library(&lib, two[0]));
    assert_eq!(
        lib.exclusion_counts(&Filter::default())
            .unwrap()
            .of(ExclusionCategory::You),
        1
    );
}

fn cfg_for(url: &str) -> ProviderConfig {
    let mut cfg = ProviderConfig::from_preset(&PRESETS[0]);
    cfg.base_url = format!("{url}/v1");
    cfg.model = "recording-vlm".into();
    cfg.retry = RetryPolicy {
        max_retries: 1,
        rate_limit_base: Duration::from_millis(10),
        server_backoff: vec![Duration::from_millis(10)],
        max_retry_after: Duration::from_secs(1),
    };
    cfg
}

struct Loader;

impl PreviewLoader for Loader {
    fn load(&self, c: &ContentId) -> Result<PreviewJpeg, PreviewError> {
        Ok(preview(u64::from(c.digest[0])))
    }
}

/// The recorded reply: every photo is a document (not a photo, confident).
fn document_reply() -> Reply {
    let mut v = insight_json("document", 0, 0, &[]);
    v["is_photo"] = serde_json::Value::Bool(false);
    v["confidence"] = serde_json::json!(0.95);
    Reply::json(200, &openai_reply(&v.to_string()))
}

#[test]
fn scenario_ai_review_put_back_stays_put_after_another_ai_run() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = library(corpus(tmp.path(), ScanOptions::default()), tmp.path());
    let cat = lib.catalog().unwrap().clone();
    let server = MockServer::with_handler(|_| document_reply());
    let cfg = cfg_for(&server.url());
    let mut qc = QueueConfig::local(&cfg.name);
    qc.concurrency = 1;
    let ai = AiService::with_loader(cat.clone(), cfg, qc, Arc::new(Loader)).unwrap();
    assert!(ai.start(false).unwrap() > 0);
    assert!(ai.wait_idle(Duration::from_secs(60)));
    let store = lib.insight_store().unwrap();
    let open = |store: &insights::InsightStore| {
        insights::items(
            &cat,
            store,
            &InsightFilter {
                suggestions: true,
                ..InsightFilter::default()
            },
            10_000,
        )
        .unwrap()
    };
    let item = open(store)
        .into_iter()
        .next()
        .expect("a document suggestion");
    let s = item.suggestion.clone().unwrap();
    assert_eq!(s.kind, "exclude_document");
    assert!(insights::decide(&cat, store, &item.content, true).unwrap() > 0);
    let files: Vec<ItemId> = item
        .files
        .iter()
        .map(|f| ItemId::try_from(*f).unwrap())
        .collect();
    for f in &item.files {
        let d = cat.read().user_decision(*f).unwrap().unwrap();
        assert_eq!(
            (d.decision, d.ai_kind),
            (UserDecision::Exclude, Some(AiKind::Document))
        );
    }
    let docs = Filter {
        exclusion: Some(ExclusionSelection::only_ai(AiKind::Document)),
        ..Filter::default()
    };
    assert_eq!(
        lib.exclusion_counts(&docs).unwrap().ai(AiKind::Document),
        files.len() as u64
    );

    lib.put_back(&files).unwrap();
    assert!(files.iter().all(|f| in_library(&lib, *f)));
    // Another AI run re-describes the unchanged file with the same result.
    let content = cat
        .read()
        .file_by_id(item.files[0])
        .unwrap()
        .unwrap()
        .identity
        .unwrap();
    cat.writer().ai_requeue(vec![content.digest], 0).unwrap();
    let requests = server.request_count();
    ai.start(false).unwrap();
    assert!(ai.wait_idle(Duration::from_secs(60)));
    assert!(
        server.request_count() > requests,
        "the file was described again"
    );
    let again = open(store);
    assert!(
        !again.iter().any(|i| i.content == item.content),
        "no exclusion suggestion is open for the file"
    );
    let decided = insights::items(&cat, store, &InsightFilter::default(), 10_000)
        .unwrap()
        .into_iter()
        .find(|i| i.content == item.content)
        .unwrap();
    assert_eq!(
        decided.suggestion.unwrap().status,
        SuggestionStatus::Rejected
    );
    assert!(
        files.iter().all(|f| in_library(&lib, *f)),
        "it stays in the Library"
    );
}
