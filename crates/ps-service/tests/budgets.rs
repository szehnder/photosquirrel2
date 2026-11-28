//! The Excluded view's opening budget, `excluded-view-open` (ADR-0103, as amended by ADR-0104): measured through
//! the service calls the view makes when it opens (`Library::excluded_open` and the first `excluded_page`), on a
//! catalog of 1M files with 200,000 excluded. `cargo xtask bench` runs this in release and reads the
//! `measure <budget>=<value> <unit>` line; debug builds report it but do not fail on it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../ps-catalog/tests/common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use common::*;
use ps_catalog::{DecisionWrite, FileRow, Results, UserDecision};
use ps_model::{
    AiKind, ClassReason, ExclusionSelection, FileId, Filter, Format, ItemId, PhotoClass, Sort,
    SortField, SourceId,
};
use ps_service::library::Library;

/// The full scale is 1M rows; see [`scale_rows`].
fn rows() -> u64 {
    scale_rows(1_000_000)
}

fn budget(name: &str, value: f64, unit: &str, max: f64) {
    if reduced_scale() {
        // Not the budget's scale: no `measure` line, so nothing reads it as one.
        println!("{name}: {value:.3} {unit} at reduced scale (not a measurement)");
        return;
    }
    println!("measure {name}={value:.3} {unit}");
    if !cfg!(debug_assertions) {
        assert!(
            value <= max,
            "{name}: {value:.3} {unit} exceeds the budget of {max} {unit}"
        );
    }
}

/// A row of the Excluded view's budget catalog: 18% excluded by the classifier (cache, graphic, too small, could
/// not be read, not supported), the rest the synthetic photos of [`full_row`].
fn excluded_view_row(source: SourceId, i: u64) -> FileRow {
    let mut row = full_row(source, i);
    let (class, format, reason) = match i % 50 {
        0 | 1 => (
            PhotoClass::ThumbnailCache,
            Some(Format::Jpeg),
            ClassReason::CacheFolder {
                rule: "Lightroom previews".into(),
            },
        ),
        2 | 3 => (
            PhotoClass::Graphic,
            Some(Format::Png),
            ClassReason::GraphicFormat {
                detail: "palette image".into(),
            },
        ),
        4 | 5 => (
            PhotoClass::Junk,
            Some(Format::Png),
            ClassReason::ShortSideBelow { side: 32, min: 200 },
        ),
        6 | 7 => (
            PhotoClass::Junk,
            Some(Format::Tiff),
            ClassReason::ProbeFailed {
                message: "truncated".into(),
            },
        ),
        8 => (
            PhotoClass::Junk,
            None,
            ClassReason::Unsupported {
                reason: "unrecognised content".into(),
            },
        ),
        _ => (
            PhotoClass::Photo,
            Some(Format::Jpeg),
            ClassReason::CameraExif,
        ),
    };
    if i % 50 == 8 {
        // Not a photo or video name: an unrecognised photo name would be "could not be read" (ADR-0104).
        row.key =
            ps_catalog::FileKey::new(source, rel(&format!("d{:04}/DOC_{i:07}.dat", i / 1000)));
    }
    match &mut row.results {
        // A photo keeps its analysis; an excluded file is probed only, as the scanner leaves it.
        Results::Analyzed(p, _) if class == PhotoClass::Photo => p.class = class,
        Results::Analyzed(p, _) | Results::Probed(p) => {
            let mut p = p.clone();
            p.class = class;
            p.format = format;
            p.reasons = vec![reason.to_string()];
            p.skip = class == PhotoClass::Junk;
            row.results = Results::Probed(p);
        }
        Results::None => {}
    }
    row
}

/// `excluded-view-open` (ADR-0103, ADR-0104): what the Excluded view asks the service for when it opens, timed
/// from the request: `excluded_open` (every chip's count, the default view's ordered list, every chip but "not a
/// supported file", by name, and the listed files' categories) and `excluded_page` for the first 200 rows (with
/// their paths, categories and decisions), on 1M files with 200,000 excluded (180,000 by the classifier in five
/// categories, 20,000 by accepted AI suggestions of four kinds). The first run (the catalog's pages as the build
/// left them) and the median of 5 are printed; the budget is the median. A search over the same set is printed too
/// and held to the same bound. Later pages are read only as the view scrolls.
#[test]
fn scenario_performance_budgets_excluded_view_open_within_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let lib = Library::open(tmp.path()).expect("open");
    let cat = lib.catalog().expect("catalog").clone();
    let src = add_source(&cat, "/mnt/bench");
    let rows = rows();
    write_rows(&cat, src, 0..rows, 2, excluded_view_row);
    // 20,000 accepted AI suggestions (2% of the files, all photos), written 10,000 per transaction.
    let ai: Vec<u64> = (0..rows).filter(|i| i % 50 == 10).collect();
    let ids = cat
        .read()
        .id_list(&Filter::default(), &Sort::by(SortField::FileName))
        .expect("all ids");
    assert_eq!(ids.len() as u64, rows);
    let mut by_index: Vec<ItemId> = ids.ids.clone();
    by_index.sort();
    for chunk in ai.chunks(10_000) {
        let writes = chunk
            .iter()
            .map(|i| {
                let k = AiKind::ALL[(*i as usize / 50) % 4];
                DecisionWrite {
                    file: FileId::from(by_index[*i as usize]),
                    decision: Some(UserDecision::Exclude),
                    reason: Some(format!("{} (AI suggestion)", k.suggestion_label())),
                    ai_kind: Some(k),
                }
            })
            .collect();
        cat.writer().set_user_decisions(writes).expect("decisions");
    }
    cat.writer().flush().expect("flush");
    let open = |sel: ExclusionSelection| {
        let view = Filter {
            exclusion: Some(sel),
            ..Filter::default()
        };
        let sort = Sort::by(SortField::FileName);
        let t = Instant::now();
        let listing = lib.excluded_open(&view, &sort).expect("counts and list");
        let first: Vec<ItemId> = listing.list.ids.iter().take(200).copied().collect();
        let page = lib.excluded_page(&first).expect("page");
        (t.elapsed(), listing, page)
    };
    let mut times = Vec::new();
    for _ in 0..5 {
        let (took, listing, page) = open(ExclusionSelection::default_view());
        times.push(took);
        assert_eq!(
            listing.counts.total(),
            rows / 5,
            "200,000 excluded at full scale"
        );
        assert_eq!(
            listing.list.len() as u64,
            listing.counts.shown(&ExclusionSelection::default_view())
        );
        assert_eq!(
            listing.list.len() as u64,
            rows / 5 - rows / 50,
            "unsupported hidden"
        );
        assert_eq!(listing.categories.len(), listing.list.len());
        assert_eq!(page.len(), 200);
        assert!(
            page.iter()
                .all(|r| r.category.is_some() && !r.reasons.is_empty())
        );
    }
    let first = times[0];
    println!("excluded-view-open runs: {times:?}");
    println!(
        "excluded-view-open first run: {:.3} ms",
        first.as_secs_f64() * 1e3
    );
    let mut sorted = times.clone();
    sorted.sort();
    let median: Duration = sorted[sorted.len() / 2];
    // A search narrows the same pass: held to the same bound (not a separate budget).
    let search = ExclusionSelection {
        text: Some("img_00".into()),
        ..ExclusionSelection::default_view()
    };
    let mut searched = Vec::new();
    for _ in 0..3 {
        let (took, listing, _) = open(search.clone());
        assert!(!listing.list.is_empty() && (listing.list.len() as u64) < rows / 5);
        searched.push(took);
    }
    searched.sort();
    let search_median = searched[searched.len() / 2];
    println!(
        "excluded-view-open with a search: {:.3} ms (median of 3)",
        search_median.as_secs_f64() * 1e3
    );
    if !reduced_scale() && !cfg!(debug_assertions) {
        assert!(
            search_median.as_secs_f64() * 1e3 <= 150.0,
            "a search {search_median:?} exceeds the excluded-view-open bound"
        );
    }
    budget(
        "excluded-view-open",
        median.as_secs_f64() * 1e3,
        "ms",
        150.0,
    );
}

/// `duplicates-list-reload` (ADR-0133, ai-filtered-duplicates): the Duplicates list listed again after a change
/// that leaves the groups unchanged, through the review cache the app uses. The catalog: 1M files, 100,000 groups
/// of 3 (one in every 10 files), and an AI insight for every keeper (half with people). Measured: a reload after a
/// group is marked reviewed (no filter), and a reload with "contains people" (the gallery's insight index already
/// built). Median of 5 each; the budget is the slower of the two medians. The first, uncached list is printed.
#[test]
fn scenario_performance_budgets_duplicates_list_reload_within_budget() {
    use ps_catalog::groups::{GroupChanges, GroupRow, MemberEdge, MemberRow};
    use ps_model::library::InsightCriteria;
    use ps_service::review::{IndexMatcher, ReviewCache, ReviewFilter, ReviewSort, ReviewStore};

    let tmp = tempfile::tempdir().expect("tempdir");
    let lib = Library::open(tmp.path()).expect("open");
    let cat = lib.catalog().expect("catalog").clone();
    let src = add_source(&cat, "/mnt/bench");
    let rows = rows();
    write_rows(&cat, src, 0..rows, 2, full_row);
    // Files with a content identity (probe-only rows have none), in id order.
    let mut ids: Vec<FileId> = Vec::new();
    cat.read()
        .for_each_content(|file, _| ids.push(file))
        .expect("contents");
    ids.sort();
    let groups = rows / 10;
    assert!(
        ids.len() as u64 >= groups * 3,
        "{} files with content",
        ids.len()
    );
    let group_rows: Vec<GroupRow> = (0..groups)
        .map(|g| {
            let files: Vec<FileId> = (0..3).map(|k| ids[(g * 3 + k) as usize]).collect();
            GroupRow {
                id: files[0],
                keeper: files[0],
                display: files[0],
                keeper_set: vec![files[0]],
                reasons: serde_json::json!([]),
                reclaimable_bytes: 1_000 + g,
                digest: [0; 32],
                members: files
                    .iter()
                    .enumerate()
                    .map(|(rank, f)| MemberRow {
                        file: *f,
                        rank: rank as u32,
                        edge: if rank == 0 {
                            MemberEdge::Keeper
                        } else {
                            MemberEdge::Exact
                        },
                        via: (rank > 0).then_some(files[0]),
                        phash_distance: None,
                        dhash_distance: None,
                        in_keeper_set: rank == 0,
                    })
                    .collect(),
                reviewed_at_ms: None,
            }
        })
        .collect();
    let keepers: Vec<FileId> = group_rows.iter().map(|g| g.keeper).collect();
    cat.writer()
        .apply_groups(GroupChanges {
            clear: true,
            groups: group_rows,
            ..GroupChanges::default()
        })
        .expect("groups");
    let reader = cat.read();
    for (i, k) in keepers.iter().enumerate() {
        let content = reader
            .file_by_id(*k)
            .expect("read")
            .and_then(|r| r.identity)
            .expect("content");
        let people = u32::from(i % 2 == 0);
        cat.writer()
            .ai_complete(ps_catalog::AiResultRow {
                content,
                provider: "bench".into(),
                model: "bench".into(),
                schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
                prompt_version: 1,
                insight: serde_json::json!({
                    "subject": "bench", "category": if people == 1 { "people" } else { "landscape" },
                    "is_photo": true, "people": {"total": people, "adults": people, "children": 0},
                    "pets": [], "confidence": 0.9
                })
                .to_string(),
                latency_ms: 1,
                requests: 1,
                at_ms: i as i64,
            })
            .expect("insight");
    }
    cat.writer().flush().expect("flush");

    let svc = lib.scan().expect("scan");
    let store = ReviewStore::in_memory();
    let cache = ReviewCache::new();
    let people = InsightCriteria {
        people: true,
        ..InsightCriteria::default()
    };
    let list = |filter: ReviewFilter| {
        let mut sets = lib
            .insight_matches_many(&[people, InsightCriteria::default()])
            .expect("sets");
        let index = IndexMatcher {
            described: sets.pop().expect("described"),
            matching: sets.pop().expect("matching"),
        };
        cache
            .list(svc, &store, filter, ReviewSort::Reclaimable, Some(&index))
            .expect("list")
    };
    let filtered = ReviewFilter {
        insight: Some(people),
        ..ReviewFilter::default()
    };
    let t = Instant::now();
    let first = list(ReviewFilter::default());
    println!("duplicates list, first (uncached) load: {:?}", t.elapsed());
    assert_eq!(first.groups, groups);
    assert_eq!(list(filtered).matching, groups.div_ceil(2));
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2].as_secs_f64() * 1e3
    };
    let after_review = median(
        first
            .rows
            .iter()
            .take(5)
            .map(|r| {
                ps_service::review::set_reviewed(svc, &store, r.id, true).expect("review");
                let t = Instant::now();
                let l = list(ReviewFilter::default());
                let e = t.elapsed();
                assert!(l.rows.iter().any(|x| x.id == r.id && x.reviewed));
                e
            })
            .collect(),
    );
    let with_filter = median(
        (0..5)
            .map(|_| {
                let t = Instant::now();
                list(filtered);
                t.elapsed()
            })
            .collect(),
    );
    println!(
        "duplicates list reload: after a review mark {after_review:.1} ms, with an AI filter {with_filter:.1} ms"
    );
    budget(
        "duplicates-list-reload",
        after_review.max(with_filter),
        "ms",
        150.0,
    );
}

// ---- best-of-stacks (ADR-0135) ---------------------------------------------------------------------------------

/// `n` synthetic photos as the stack rule sees them: bursts of 2-12 frames 0.25 s apart whose hashes drift a few
/// bits per frame, between single shots, one shot every 3 s on average, with photo-like hashes.
fn stack_inputs(n: u64) -> Vec<ps_dedup::DedupInput> {
    use ps_model::{CaptureTime, ContentId, PerceptualHash};
    let mut out = Vec::with_capacity(n as usize);
    let mut t_ms: i64 = 0;
    let mut i = 0u64;
    let mut h = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        h
    };
    while i < n {
        let burst = if next() % 4 == 0 { 2 + next() % 11 } else { 1 };
        let base = next();
        t_ms += 1000 + (next() % 4000) as i64;
        for k in 0..burst.min(n - i) {
            let mut t = CaptureTime::from_parts(2020, 1, 1, 0, 0, 0);
            t.local_seconds += (t_ms + k as i64 * 250) / 1000;
            t.millis = ((t_ms + k as i64 * 250) % 1000) as u16;
            let drift = (1u64 << (next() % 64)) | (1u64 << (next() % 64));
            out.push(ps_dedup::DedupInput {
                file: FileId(i as i64 + 1),
                content: Some(ContentId::full(*blake3::hash(&i.to_le_bytes()).as_bytes())),
                hash: Some(PerceptualHash {
                    phash: base ^ (drift.wrapping_mul(k)),
                    dhash: base.rotate_left(7) ^ drift,
                    version: ps_hash::PERCEPTUAL_HASH_VERSION,
                }),
                format: Format::Jpeg,
                class: PhotoClass::Photo,
                path: format!("1/DCIM/IMG_{i:07}.JPG"),
                size: 4_000_000,
                mtime_ns: 0,
                meta: ps_dedup::DedupMeta {
                    width: Some(6000),
                    height: Some(4000),
                    capture_time: Some(t),
                    subsec_present: true,
                    camera_model: Some("EOS R6".into()),
                    has_maker_notes: true,
                    camera_original: true,
                    ..Default::default()
                },
            });
            i += 1;
        }
    }
    out
}

fn time_stacks(inputs: &[ps_dedup::DedupInput], t: ps_dedup::StackThresholds) -> (f64, usize) {
    use std::collections::{BTreeSet, HashMap, HashSet};
    let groups = ps_dedup::GroupMembership::default();
    let (seps, picks, excluded, quality) = (
        BTreeSet::new(),
        BTreeSet::new(),
        HashSet::new(),
        HashMap::new(),
    );
    let d = ps_dedup::StackDecisions {
        separations: &seps,
        picks: &picks,
        excluded: &excluded,
        quality: &quality,
    };
    let mut times = Vec::new();
    let mut stacked = 0;
    for _ in 0..3 {
        let t0 = Instant::now();
        let s = ps_dedup::stacks_of(inputs, &groups, t, &d);
        times.push(t0.elapsed().as_secs_f64());
        stacked = s.stacked_shots();
    }
    times.sort_by(f64::total_cmp);
    (times[1], stacked)
}

/// `stacks-compute`: every stack and pick of 1M photos at the widest thresholds (60 s, loose), from inputs in
/// memory; median of 3.
#[test]
fn scenario_performance_budgets_stacks_computed_within_budget() {
    let inputs = stack_inputs(rows());
    let (s, stacked) = time_stacks(
        &inputs,
        ps_dedup::StackThresholds {
            window_s: 60,
            similarity: ps_dedup::StackSimilarity::Loose,
        },
    );
    println!(
        "stacks of {} photos: {stacked} stacked, {:.0} ms",
        inputs.len(),
        s * 1000.0
    );
    assert!(stacked > 0);
    budget("stacks-compute", s, "s", 1.0);
}

/// `stacks-retune`: the stacks of 100,000 photos after a similarity change (5 s, strict to balanced); median of 3.
#[test]
fn scenario_performance_budgets_retune_within_budget() {
    let inputs = stack_inputs(scale_rows(100_000));
    let (ms, _) = time_stacks(&inputs, ps_dedup::StackThresholds::default());
    budget("stacks-retune", ms * 1000.0, "ms", 300.0);
}

/// `shot-quality-fill`: the scores of 100,000 thumbed photos from the thumbnail store (1,000 distinct 384 px
/// thumbnails shared by the rows, so the store stays small while every row is decoded).
#[test]
fn scenario_performance_budgets_shot_quality_filled_within_budget() {
    let n = scale_rows(100_000);
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = ps_catalog::Catalog::open(tmp.path()).expect("catalog");
    let src = add_source(&cat, "/mnt/bench");
    write_rows(&cat, src, 0..n, 2, full_row);
    let thumbs = ps_catalog::ThumbStore::open(tmp.path()).expect("thumbs");
    let img = ps_fixtures::synth::Rgb {
        width: 384,
        height: 256,
        data: ps_fixtures::media::photo_rgb(384, 256, 9),
    };
    let jpeg = ps_fixtures::jpeg::encode_jpeg(&img, 85);
    let rec = ps_model::ThumbnailRecord {
        kind: ps_model::ThumbKind::Image,
        width: 384,
        height: 256,
        dominant_rgb: [90, 90, 90],
        jpeg,
        placeholder: None,
    };
    let distinct: Vec<ps_model::ContentId> = (0..1000)
        .map(|i| ps_model::ContentId::full(digest_of(i)))
        .collect();
    thumbs
        .put_many(distinct.iter().map(|c| (c, &rec)))
        .expect("thumbnails");
    cat.writer().flush().expect("flush");
    {
        let c = rusqlite::Connection::open(tmp.path().join(ps_catalog::CATALOG_FILE)).unwrap();
        c.execute_batch(
            "UPDATE file SET stage = 3, preview_reason = NULL, shot_quality_version = NULL
             WHERE content_id IS NOT NULL;",
        )
        .unwrap();
        let mut st = c
            .prepare("UPDATE file SET content_id = ?2, identity_kind = 0 WHERE id = ?1")
            .unwrap();
        let ids: Vec<i64> = c
            .prepare("SELECT id FROM file WHERE content_id IS NOT NULL")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for id in &ids {
            st.execute(rusqlite::params![
                id,
                &distinct[(*id as usize) % 1000].digest[..]
            ])
            .unwrap();
        }
    }
    let t0 = Instant::now();
    let r = ps_scan::shot_quality::fill(&cat, &thumbs, &|| false).expect("fill");
    let s = t0.elapsed().as_secs_f64();
    println!(
        "shot quality: {} scored, {} skipped in {s:.1} s",
        r.scored, r.skipped
    );
    assert!(r.scored > 0 && r.skipped == 0);
    budget("shot-quality-fill", s, "s", 60.0);
}
