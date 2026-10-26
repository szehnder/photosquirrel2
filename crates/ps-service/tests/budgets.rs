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
