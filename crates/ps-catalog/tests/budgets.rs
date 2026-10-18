//! Performance budgets (catalog tasks 4.1, 4.2; ADR-0026/0036). `cargo xtask bench` runs these in release
//! and reads the `measure <budget>=<value> <unit>` lines. Budget assertions apply to optimized builds only:
//! debug builds report the measurement but do not fail on it.

mod common;

use std::time::{Duration, Instant};

use common::*;
use ps_catalog::Catalog;
use ps_model::{Filter, Sort};

const ROWS: u64 = 1_000_000;

fn budget(name: &str, value: f64, unit: &str, max: f64) {
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
    let t = Instant::now();
    // The walk's insert path (ADR-0007): new paths with their stat fingerprints, from two producer
    // threads. Stage results arrive later as updates at analysis speed (~150 files/s), far below this.
    write_rows(&cat, src, 0..ROWS, 2, discovered_row);
    let e = t.elapsed();
    let stats = cat.writer().stats();
    assert_eq!(stats.errors, 0);
    assert!(stats.rows_committed >= ROWS);
    let us_per_row = e.as_secs_f64() * 1e6 / ROWS as f64;
    println!(
        "inserted {ROWS} rows in {:.2} s = {:.0} rows/s ({} commits)",
        e.as_secs_f64(),
        ROWS as f64 / e.as_secs_f64(),
        stats.commits
    );
    budget("catalog-insert", us_per_row, "us", 20.0);
}

#[test]
fn scenario_catalog_id_list_for_1m_within_300_ms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/bench");
    write_rows(&cat, src, 0..ROWS, 2, full_row);
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let l = cat
            .read()
            .id_list(&Filter::default(), &Sort::default())
            .expect("id list");
        times.push(t.elapsed());
        assert_eq!(l.len(), ROWS as usize);
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
