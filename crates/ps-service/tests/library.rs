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
