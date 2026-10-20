//! The library facade (gallery task 1.1, ai-insights task 3.3, export task 6.3): complete ordered ID lists and
//! 512-item detail pages over a synthetic catalog, and the filter-aware ID lists for AI insights (effective,
//! corrections win) and completion-report dispositions.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use ps_fixtures::{DirWriter, Params, Preset};
use ps_model::library::{GalleryQuery, InsightCriteria};
use ps_model::{Disposition, ExportProfile, ItemId, SceneCategory};
use ps_service::export_progress::{ExecConfig, ProfileSettings};
use ps_service::insights::{self, Correction};
use ps_service::library::{Library, library_filter, synthetic};
use ps_service::scan::{ScanOptions, ScanService};

fn query() -> GalleryQuery {
    GalleryQuery {
        filter: library_filter(),
        stacks: true,
        ..GalleryQuery::default()
    }
}

fn synthetic_library(dir: &Path, items: u64) -> Library {
    synthetic::build(dir, items).expect("synthetic catalog");
    Library::open(dir).expect("open")
}

#[test]
fn library_id_list_and_512_item_pages() {
    let tmp = tempfile::tempdir().expect("tmp");
    let lib = synthetic_library(tmp.path(), 1_500);
    let list = lib.id_list(&query()).expect("id list");
    assert_eq!(list.len(), 1_500);
    // Capture-date order: primaries never decrease (the scrubber binary-searches them).
    assert!(
        list.keys
            .windows(2)
            .all(|w| w[0].primary() <= w[1].primary())
    );
    let page = lib.summaries(&list.ids[700..1_212]).expect("page");
    assert_eq!(page.len(), 512);
    assert_eq!(page[0].id, list.ids[700]);
    assert_eq!(page[511].id, list.ids[1_211]);
    // The first 3,000 items have stored thumbnails and dominant colours.
    assert!(
        page.iter()
            .all(|s| s.thumb.is_some() && s.dominant.is_some())
    );
    let thumb = lib.thumbnail(&page[0].thumb.expect("key")).expect("thumb");
    assert!(thumb.is_some_and(|j| j.starts_with(&[0xFF, 0xD8])));
    let detail = lib.detail(list.ids[0]).expect("detail").expect("present");
    assert_eq!(detail.camera_make.as_deref(), Some("Fujifilm"));
    assert!(detail.insight.is_none(), "not analysed yet");
    assert!(lib.summaries(&list.ids[..513]).is_err(), "pages hold 512");
}

/// Task 1.1 at full size: the library queries over a 1M catalog. Builds (once) the synthetic catalog under
/// `PHOTOSQUIRREL_SYNTHETIC_DIR` (or the user cache dir):
/// `cargo test --release -p ps-service --test library -- --ignored library_queries_over_1m`.
#[test]
#[ignore = "builds a 1.2 GB catalog on first run"]
fn library_queries_over_1m() {
    let dir = synthetic::ensure(1_000_000).expect("synthetic catalog");
    let lib = Arc::new(Library::open(&dir).expect("open"));
    let t = Instant::now();
    let list = lib.id_list(&query()).expect("id list");
    println!("id list of {} in {:?}", list.len(), t.elapsed());
    assert_eq!(list.len(), 1_000_000);
    let t = Instant::now();
    let page = lib.summaries(&list.ids[500_000..500_512]).expect("page");
    println!("512-item page in {:?}", t.elapsed());
    assert_eq!(page.len(), 512);
    assert_eq!(page[0].id, list.ids[500_000]);
    let first = lib.summaries(&list.ids[..512]).expect("first page");
    assert!(
        first
            .iter()
            .all(|s| s.thumb.is_some() && s.dominant.is_some())
    );
}

/// The filter bench (ai-insights 3.3, export 6.3): filter-aware ID lists on the 1M catalog with an insight
/// for every photo. Seeds the demo insights once (marker file next to the catalog).
/// `PHOTOSQUIRREL_SYNTHETIC_DIR=target/synthetic cargo test --release -p ps-service --test library --
/// --ignored --nocapture filtered_id_lists_over_1m`.
#[test]
#[ignore = "builds a 1.2 GB catalog and 1M insights on first run"]
fn filtered_id_lists_over_1m() {
    let dir = synthetic::ensure(1_000_000).expect("synthetic catalog");
    let lib = Library::open(&dir).expect("open");
    let marker = dir.join(".insights-seeded");
    if !marker.exists() {
        let t = Instant::now();
        let n = insights::seed_demo(lib.catalog().expect("catalog"), 1_000_000).expect("seed");
        println!("seeded {n} insights in {:?}", t.elapsed());
        std::fs::write(&marker, b"ok").expect("marker");
    }
    let time = |label: &str, q: &GalleryQuery| {
        let t = Instant::now();
        let l = lib.id_list(q).expect("id list");
        println!("{label}: {} items in {:?}", l.len(), t.elapsed());
        l.len()
    };
    time("unfiltered", &query());
    let children = GalleryQuery {
        insight: InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        },
        ..query()
    };
    // The demo insights cycle through 8 kinds; 3 of them have children.
    let n = time("children (first: builds the insight index)", &children);
    assert_eq!(n, 375_000);
    time("children (again)", &children);
    let pets = GalleryQuery {
        insight: InsightCriteria {
            pets: true,
            ..InsightCriteria::default()
        },
        ..query()
    };
    time("pets (index cached, new criteria)", &pets);
    let travel = GalleryQuery {
        insight: InsightCriteria {
            category: Some(SceneCategory::Travel),
            people: true,
            ..InsightCriteria::default()
        },
        ..query()
    };
    time("travel with people", &travel);
    // A live AI queue: one new result (re-describing a content in place) is applied to the index, not rebuilt.
    let cat = lib.catalog().expect("catalog").clone();
    let first = cat
        .read()
        .ai_results(1)
        .expect("results")
        .pop()
        .expect("a result");
    cat.writer()
        .ai_complete(ps_catalog::AiResultRow {
            at_ms: ps_export::now_ms(),
            ..first
        })
        .expect("store");
    let before = lib.insight_index_stats();
    time(
        "children after one new insight (in-place update)",
        &children,
    );
    let after = lib.insight_index_stats();
    assert_eq!(after.full_builds, before.full_builds, "{after:?}");
    assert_eq!(after.updates, before.updates + 1);
    let pending = GalleryQuery {
        dispositions: vec![Disposition::Pending],
        ..query()
    };
    let n = time(
        "pending (first: derives every disposition, no library)",
        &pending,
    );
    assert_eq!(n, 1_000_000);
    time("pending (again)", &pending);
    let both = GalleryQuery {
        dispositions: vec![Disposition::Pending],
        ..children.clone()
    };
    time("pending children", &both);
}

#[test]
fn insight_filter_uses_the_effective_insight() {
    let tmp = tempfile::tempdir().expect("tmp");
    let lib = synthetic_library(tmp.path(), 400);
    let cat = lib.catalog().expect("catalog").clone();
    // 160 analysed photos; demo kinds 0, 3 and 5 (of 8) have children: 60 photos.
    assert_eq!(insights::seed_demo(&cat, 160).expect("seed"), 160);
    let children = GalleryQuery {
        insight: InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        },
        stacks: false,
        ..query()
    };
    let list = lib.id_list(&children).expect("list");
    assert_eq!(list.len(), 60);
    for id in &list.ids {
        let d = lib.detail(*id).expect("detail").expect("present");
        let i = d.insight.expect("analysed");
        assert!(i.effective.people.children > 0);
        assert_eq!(i.provider, insights::DEMO_PROVIDER);
        assert!(i.demo);
    }
    // Ordered like the unfiltered list.
    let all = lib.id_list(&query()).expect("all");
    let pos: Vec<usize> = list
        .ids
        .iter()
        .map(|i| all.ids.iter().position(|x| x == i).expect("in the library"))
        .collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]));

    // Corrections win: one child photo corrected to no children, one landscape corrected to two children.
    let store = lib.insight_store().expect("store");
    let first = lib.detail(list.ids[0]).expect("d").expect("p");
    let no_kids = ps_model::PeopleEstimate {
        total: 1,
        adults: 1,
        children: 0,
    };
    store
        .correct(
            first.content_hash.as_deref().expect("hash"),
            Correction {
                people: Some(no_kids),
                ..Correction::default()
            },
        )
        .expect("save correction");
    let landscape = all
        .ids
        .iter()
        .copied()
        .find(|id| {
            lib.detail(*id)
                .ok()
                .flatten()
                .and_then(|d| d.insight)
                .is_some_and(|i| i.ai.category == SceneCategory::Landscape)
        })
        .expect("a landscape");
    let hash = lib
        .detail(landscape)
        .expect("d")
        .expect("p")
        .content_hash
        .expect("hash");
    store
        .correct(
            &hash,
            Correction {
                people: Some(ps_model::PeopleEstimate {
                    total: 2,
                    adults: 0,
                    children: 2,
                }),
                category: Some(SceneCategory::Travel),
                ..Correction::default()
            },
        )
        .expect("save");
    let list = lib.id_list(&children).expect("list");
    assert_eq!(list.len(), 60);
    assert!(!list.ids.contains(&first.id));
    assert!(list.ids.contains(&landscape));
    let d = lib.detail(landscape).expect("d").expect("p");
    let i = d.insight.expect("insight");
    assert_eq!(
        i.ai.category,
        SceneCategory::Landscape,
        "the AI value stays"
    );
    assert_eq!(i.effective.category, SceneCategory::Travel);
    assert_eq!(i.corrected, vec!["category", "people"]);
    // Combined with other filters (AND), and the category filter sees the correction.
    let travel_kids = GalleryQuery {
        insight: InsightCriteria {
            category: Some(SceneCategory::Travel),
            children: true,
            ..InsightCriteria::default()
        },
        ..children.clone()
    };
    assert!(
        lib.id_list(&travel_kids)
            .expect("list")
            .ids
            .contains(&landscape)
    );
    // Files without an insight never match.
    let unanalysed: Vec<ItemId> = all.ids[..]
        .iter()
        .copied()
        .filter(|id| {
            lib.detail(*id)
                .ok()
                .flatten()
                .is_some_and(|d| d.insight.is_none())
        })
        .collect();
    assert_eq!(unanalysed.len(), 240);
    let people = GalleryQuery {
        insight: InsightCriteria {
            people: true,
            ..InsightCriteria::default()
        },
        ..children
    };
    let p = lib.id_list(&people).expect("people");
    assert!(unanalysed.iter().all(|id| !p.ids.contains(id)));
}

fn tiny() -> Params {
    Params {
        standalone: 10,
        sets_per_kind: 2,
        burst_sets: 2,
        videos: 1,
        scans: 1,
        icons: 2,
        ui_graphics: 2,
        tiny_web: 2,
        junk_videos: 1,
        raw_share: 0.25,
        dim_scale: (1, 2),
        twelve_mp: 0,
        backups: (1, 2),
    }
}

#[test]
fn disposition_filter_lists_exactly_the_files_with_that_disposition() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("corpus");
    ps_fixtures::generate_with(
        11,
        Preset::Small,
        tiny(),
        &DirWriter::new(&root).expect("w"),
    )
    .expect("generate");
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default()).expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    let lib = Library::with_service(&data, svc);
    let by = |d: &[Disposition]| {
        let q = GalleryQuery {
            dispositions: d.to_vec(),
            ..query()
        };
        let mut ids = lib.id_list(&q).expect("list").ids;
        ids.sort();
        ids
    };
    // Before any export: no library, nothing exported, the Library's photos are pending.
    assert!(!lib.has_export_library().expect("library"));
    assert!(by(&[Disposition::Exported]).is_empty());
    let pending = by(&[Disposition::Pending]);
    assert!(!pending.is_empty());
    // Export every keeper into a temp library, then each filter lists exactly its files.
    let library = tmp.path().join("library");
    let (_, report) = lib
        .scan()
        .expect("scan")
        .exports()
        .run(
            &library,
            ProfileSettings::defaults(ExportProfile::ApplePhotos),
            &ExecConfig::default(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .expect("export");
    assert!(report.copied > 0);
    lib.forget_dispositions();
    let ex = lib.scan().expect("scan").exports();
    let lib_root = ex.last_library().expect("last").expect("a library");
    let (files, d, _) = ex.dispositions(&lib_root, false).expect("dispositions");
    let no_stacks = |wanted: &[Disposition]| {
        let q = GalleryQuery {
            dispositions: wanted.to_vec(),
            stacks: false,
            ..query()
        };
        let mut ids = lib.id_list(&q).expect("list").ids;
        ids.sort();
        ids
    };
    for wanted in Disposition::ALL {
        let mut expected: Vec<ItemId> = files
            .iter()
            .zip(&d)
            .filter(|(_, x)| x.disposition == wanted)
            .filter_map(|(f, _)| ItemId::try_from(f.file).ok())
            .collect();
        expected.sort();
        assert_eq!(no_stacks(&[wanted]), expected, "{wanted:?}");
    }
    assert!(!no_stacks(&[Disposition::Exported]).is_empty());
    assert!(!no_stacks(&[Disposition::Covered]).is_empty());
    assert!(!no_stacks(&[Disposition::Excluded]).is_empty());
    // Several dispositions: any of them.
    let mut either = no_stacks(&[Disposition::Exported]);
    either.extend(no_stacks(&[Disposition::Covered]));
    either.sort();
    assert_eq!(
        no_stacks(&[Disposition::Exported, Disposition::Covered]),
        either
    );
}

/// Stack badges count the files the Library shows: an excluded member leaves its stack's count and its
/// expansion, so badges plus singles always add up to the unstacked list (the toolbar's and navigator's count).
#[test]
fn stack_counts_and_members_leave_out_excluded_files() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("corpus");
    ps_fixtures::generate_with(
        11,
        Preset::Small,
        tiny(),
        &DirWriter::new(&root).expect("w"),
    )
    .expect("generate");
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default()).expect("open");
    for d in ["drive-a", "drive-b", "drive-c"] {
        svc.add_source(&root.join(d)).expect("add");
    }
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    let lib = Library::with_service(&data, svc);
    let flat = GalleryQuery {
        stacks: false,
        ..query()
    };
    let adds_up = |lib: &Library| {
        let stacked = lib.id_list(&query()).expect("stacked");
        let files: u64 = stacked
            .keys
            .iter()
            .map(|k| u64::from(k.group_data().max(1)))
            .sum();
        assert_eq!(files, lib.id_list(&flat).expect("flat").len() as u64);
        stacked
    };
    let stacked = adds_up(&lib);
    let (keeper, n) = stacked
        .ids
        .iter()
        .zip(&stacked.keys)
        .map(|(id, k)| (*id, k.group_data()))
        .find(|(_, n)| *n >= 3)
        .expect("a stack of three or more");
    let members = lib.stack_members(keeper).expect("members");
    assert_eq!(members.len(), usize::from(n));
    lib.set_decision(
        members[1],
        Some(ps_model::library::Decision::Exclude),
        Some("test".into()),
    )
    .expect("exclude");
    let stacked = adds_up(&lib);
    let at = stacked
        .ids
        .iter()
        .position(|i| *i == keeper)
        .expect("keeper");
    assert_eq!(stacked.keys[at].group_data(), n - 1, "the badge drops it");
    let excluded = members[1];
    let members = lib.stack_members(keeper).expect("members");
    assert_eq!(members.len(), usize::from(n - 1));
    assert_eq!(members[0], keeper);
    assert!(!members.contains(&excluded), "the expansion leaves it out");
}

/// A live AI queue must not make every filter query rebuild the 1M-item insight index (2.4 s): one new
/// insight and one correction are applied in place, and the filtered list matches what a full build gives.
#[test]
fn a_new_insight_updates_the_insight_index_in_place() {
    let tmp = tempfile::tempdir().expect("tmp");
    let lib = synthetic_library(tmp.path(), 400);
    let cat = lib.catalog().expect("catalog").clone();
    assert_eq!(insights::seed_demo(&cat, 160).expect("seed"), 160);
    let children = GalleryQuery {
        insight: InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        },
        stacks: false,
        ..query()
    };
    assert_eq!(lib.id_list(&children).expect("list").len(), 60);
    let s = lib.insight_index_stats();
    assert_eq!((s.full_builds, s.updates), (1, 0));

    // A photo the queue has not analysed yet gets an insight with children (demo kind 0).
    let r = cat.read();
    let mut fresh = None;
    r.for_each_file(|f| {
        if fresh.is_none()
            && f.class == Some(ps_model::PhotoClass::Photo)
            && let Some(c) = f.identity
            && r.ai_result_for(&c).ok().flatten().is_none()
        {
            fresh = Some(c);
        }
    })
    .expect("files");
    let content = fresh.expect("an unanalysed photo");
    let all = lib.id_list(&query()).expect("all");
    let new_ids: Vec<ItemId> = r
        .files_with_content(&content)
        .expect("files")
        .into_iter()
        .filter_map(|f| ItemId::try_from(f).ok())
        .filter(|id| all.ids.contains(id))
        .collect();
    assert!(!new_ids.is_empty());
    cat.writer()
        .ai_complete(ps_catalog::AiResultRow {
            content,
            provider: insights::DEMO_PROVIDER.into(),
            model: "demo-vision".into(),
            schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
            prompt_version: 2,
            insight: serde_json::to_string(&insights::sample_insight(0)).expect("json"),
            latency_ms: 1,
            requests: 1,
            at_ms: ps_export::now_ms(),
        })
        .expect("store result");
    let list = lib.id_list(&children).expect("list");
    assert_eq!(list.len(), 60 + new_ids.len());
    assert!(new_ids.iter().all(|id| list.ids.contains(id)));
    let s = lib.insight_index_stats();
    assert_eq!((s.full_builds, s.updates), (1, 1), "no full rebuild");
    assert!(
        s.last_update_contents <= 2,
        "only the new row (and rows of the previous newest millisecond) are re-read: {s:?}"
    );

    // A correction (no children after all) is applied in place too.
    lib.insight_store()
        .expect("store")
        .correct(
            &content.to_hex(),
            Correction {
                people: Some(ps_model::PeopleEstimate {
                    total: 1,
                    adults: 1,
                    children: 0,
                }),
                ..Correction::default()
            },
        )
        .expect("correct");
    let list = lib.id_list(&children).expect("list");
    assert_eq!(list.len(), 60);
    assert!(new_ids.iter().all(|id| !list.ids.contains(id)));
    let s = lib.insight_index_stats();
    assert_eq!(
        (s.full_builds, s.updates, s.last_update_contents),
        (1, 2, 1)
    );
    // No change: no work.
    lib.id_list(&children).expect("list");
    assert_eq!(lib.insight_index_stats().updates, 2);
}

/// Store `insight` as the result of `content` at `at_ms` (provider "test", so it is not demo data).
fn store_result(
    cat: &ps_catalog::Catalog,
    content: ps_model::ContentId,
    insight: &ps_model::ImageInsight,
    at_ms: i64,
) {
    cat.writer()
        .ai_complete(ps_catalog::AiResultRow {
            content,
            provider: "test".into(),
            model: "m".into(),
            schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
            prompt_version: ps_llm::PROMPT_VERSION,
            insight: serde_json::to_string(insight).expect("json"),
            latency_ms: 1,
            requests: 1,
            at_ms,
        })
        .expect("store result");
}

fn children_query() -> GalleryQuery {
    GalleryQuery {
        insight: InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        },
        stacks: false,
        ..query()
    }
}

/// A scan that adds another copy of a described photo: the insight filter lists the new copy too (the index
/// maps files, and a scan changes files without touching any AI result).
#[test]
fn scenario_ai_review_filter_stays_current_after_a_scan() {
    let tmp = tempfile::tempdir().expect("tmp");
    let src = tmp.path().join("Camera");
    std::fs::create_dir_all(&src).expect("source");
    let photo = ps_fixtures::media::jpeg(640, 480, 11, 90);
    std::fs::write(src.join("IMG_0001.JPG"), &photo).expect("photo");
    std::fs::write(
        src.join("IMG_0002.JPG"),
        ps_fixtures::media::jpeg(640, 480, 12, 90),
    )
    .expect("photo");
    let lib = Library::open(&tmp.path().join("data")).expect("open");
    let scan = lib.scan().expect("engine");
    scan.add_source(&src).expect("add");
    scan.start(&[]).expect("scan");
    scan.wait().expect("wait");
    let cat = lib.catalog().expect("catalog").clone();
    let mut content = None;
    cat.read()
        .for_each_file(|f| {
            if f.key.path.file_name() == b"IMG_0001.JPG" {
                content = f.identity;
            }
        })
        .expect("files");
    let content = content.expect("analysed");
    store_result(
        &cat,
        content,
        &insights::sample_insight(0),
        ps_export::now_ms(),
    );
    assert_eq!(lib.id_list(&children_query()).expect("list").len(), 1);

    std::fs::write(src.join("Copy of IMG_0001.JPG"), &photo).expect("a second copy");
    scan.start(&[]).expect("rescan");
    scan.wait().expect("wait");
    let copies = cat.read().files_with_content(&content).expect("copies");
    assert_eq!(copies.len(), 2);
    let list = lib.id_list(&children_query()).expect("list");
    assert_eq!(list.len(), 2, "the new copy is listed: {:?}", list.ids);
}

/// A photo described again in place (same content, provider and model) with a time that is not newer than
/// the index's newest result: the filter still follows the new description.
#[test]
fn insight_filter_follows_an_in_place_re_description_with_an_older_time() {
    let tmp = tempfile::tempdir().expect("tmp");
    let lib = synthetic_library(tmp.path(), 200);
    let cat = lib.catalog().expect("catalog").clone();
    let mut contents = Vec::new();
    cat.read()
        .for_each_file(|f| {
            if contents.len() < 2
                && f.class == Some(ps_model::PhotoClass::Photo)
                && let Some(c) = f.identity
                && !contents.contains(&c)
            {
                contents.push(c);
            }
        })
        .expect("files");
    let (a, b) = (contents[0], contents[1]);
    store_result(&cat, a, &insights::sample_insight(0), 2_000);
    store_result(&cat, b, &insights::sample_insight(0), 5_000);
    let before = lib.id_list(&children_query()).expect("list").len();
    assert!(before >= 2);
    // `a` is re-described (no children now) at a time older than `b`'s result.
    store_result(&cat, a, &insights::sample_insight(1), 1_000);
    let after = lib.id_list(&children_query()).expect("list");
    let a_files = cat.read().files_with_content(&a).expect("files");
    assert!(
        a_files
            .iter()
            .filter_map(|f| ItemId::try_from(*f).ok())
            .all(|id| !after.ids.contains(&id)),
        "the re-described photo left the filter"
    );
    assert!(after.len() < before);
    assert_eq!(lib.insight_index_stats().full_builds, 1, "updated in place");
}
