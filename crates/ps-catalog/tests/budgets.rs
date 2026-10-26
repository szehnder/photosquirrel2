//! Performance budgets (catalog tasks 4.1, 4.2; ADR-0026/0036). `cargo xtask bench` runs these in release
//! and reads the `measure <budget>=<value> <unit>` lines. Budget assertions apply to optimized builds only:
//! debug builds report the measurement but do not fail on it.

mod common;

use std::time::{Duration, Instant};

use common::*;
use ps_catalog::{Catalog, DecisionWrite, FileRow, Results, UserDecision};
use ps_model::{
    AiKind, ClassReason, ExclusionSelection, FileId, Filter, Format, ItemId, PhotoClass, Sort,
    SortField, SourceId,
};

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

#[test]
fn scenario_catalog_insert_50k_rows_per_second() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/bench");
    let rows = rows();
    let t = Instant::now();
    // The walk's insert path (ADR-0007): new paths with their stat fingerprints, from two producer
    // threads. Stage results arrive later as updates at analysis speed (~150 files/s), far below this.
    write_rows(&cat, src, 0..rows, 2, discovered_row);
    let e = t.elapsed();
    let stats = cat.writer().stats();
    assert_eq!(stats.errors, 0);
    assert!(stats.rows_committed >= rows);
    let us_per_row = e.as_secs_f64() * 1e6 / rows as f64;
    println!(
        "inserted {rows} rows in {:.2} s = {:.0} rows/s ({} commits)",
        e.as_secs_f64(),
        rows as f64 / e.as_secs_f64(),
        stats.commits
    );
    budget("catalog-insert", us_per_row, "us", 20.0);
}

#[test]
fn scenario_catalog_id_list_for_1m_within_300_ms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/bench");
    let rows = rows();
    write_rows(&cat, src, 0..rows, 2, full_row);
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let l = cat
            .read()
            .id_list(&Filter::default(), &Sort::default())
            .expect("id list");
        times.push(t.elapsed());
        assert_eq!(l.len(), rows as usize);
    }
    println!("id-list 1M runs: {times:?}");
    times.sort();
    let median: Duration = times[times.len() / 2];
    budget(
        "catalog-id-list-1m",
        median.as_secs_f64() * 1e3,
        "ms",
        300.0,
    );
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

/// `excluded-view-open` (ADR-0103): the chip counts, the default view's ID list (every chip but "not a supported
/// file", by name) and its first 200-row details page, on 1M files with 200,000 excluded (180,000 by the classifier
/// in five categories, 20,000 by accepted AI suggestions of four kinds). Median of 5 runs. The Excluded view asks
/// for exactly these three when it opens; later pages are read only as it scrolls.
#[test]
fn scenario_performance_budgets_excluded_view_open_within_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
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
    let view = Filter {
        exclusion: Some(ExclusionSelection::default_view()),
        ..Filter::default()
    };
    let sort = Sort::by(SortField::FileName);
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let r = cat.read();
        let (counts, list) = r.excluded_view(&view, &sort).expect("counts and list");
        let page = r.details(&list.ids[..200.min(list.len())]).expect("page");
        times.push(t.elapsed());
        assert_eq!(counts.total(), rows / 5, "200,000 excluded at full scale");
        assert_eq!(
            list.len() as u64,
            counts.shown(&ExclusionSelection::default_view())
        );
        assert_eq!(
            list.len() as u64,
            rows / 5 - rows / 50,
            "unsupported hidden"
        );
        assert_eq!(page.len(), 200);
    }
    println!("excluded-view-open runs: {times:?}");
    times.sort();
    let median: Duration = times[times.len() / 2];
    budget(
        "excluded-view-open",
        median.as_secs_f64() * 1e3,
        "ms",
        150.0,
    );
}
