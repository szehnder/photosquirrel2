//! Performance budgets (scanning tasks 7.1-7.3; ADR-0026/0036). `cargo xtask bench` runs these in release
//! and reads the `measure <budget>=<value> <unit>` lines. Budget assertions apply to optimized builds only:
//! debug builds report the measurement but do not fail on it. Each test prints the load average, since the
//! reference machine is often shared.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use ps_model::{RunStatus, Stage};
use ps_scan::ScanOptions;

fn budget(name: &str, value: f64, unit: &str, max: f64) {
    println!("measure {name}={value:.3} {unit}");
    println!("load average: {}", load_average());
    if !cfg!(debug_assertions) {
        assert!(
            value <= max,
            "{name}: {value:.3} {unit} exceeds the budget of {max} {unit}"
        );
    }
}

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "unknown".into())
}

fn env_count(var: &str, release: usize, debug: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) {
            debug
        } else {
            release
        })
}

/// A cached folder under the target directory, built once by `make` (`ps_fixtures::shared_dir`).
fn cached(name: &str, make: impl FnOnce(&Path)) -> PathBuf {
    ps_fixtures::shared_dir(&PathBuf::from(env!("CARGO_TARGET_TMPDIR")), name, make)
}

/// Read every file once so the scan measures a warm cache.
fn warm(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let _ = std::fs::read(&p);
            }
        }
    }
}

#[test]
fn scenario_scanning_analysis_throughput_within_budget() {
    let _cores = exclusive();
    let files = env_count("PS_SCAN_BENCH_FILES", 2_000, 200);
    // 50 distinct 12 MP camera JPEGs, hard-linked to `files` paths: every path is analyzed in full while the
    // page cache holds only the distinct contents.
    let distinct = cached("ps-scan-bench-12mp-distinct", |d| {
        for i in 0..50u64 {
            let jpeg = ps_fixtures::media::jpeg_with_exif(
                4000,
                3000,
                i,
                90,
                &ps_fixtures::media::ExifSpec::canon_5d(),
            );
            ps_fixtures::scratch::write(d.join(format!("d{i:02}.jpg")), &jpeg).expect("write");
        }
    });
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tmp");
    let root = tmp.path().join("12mp");
    for i in 0..files {
        let dir = root.join(format!("{:02}", i / 100));
        std::fs::create_dir_all(&dir).expect("mkdir");
        ps_fixtures::scratch::hard_link(
            distinct.join(format!("d{:02}.jpg", i % 50)),
            dir.join(format!("IMG_{i:05}.JPG")),
        )
        .expect("link");
    }
    warm(&distinct);
    let e = env_with(ScanOptions {
        device_class: Some(std::sync::Arc::new(|_: &Path| {
            ps_model::DeviceClass::SolidState
        })),
        ..ScanOptions::default()
    });
    let id = e.add(&root);
    let t = Instant::now();
    let summary = e.scan(&[id]);
    let wall = t.elapsed();
    completed(&summary);
    assert_eq!(summary.stages.thumbed as usize, files, "{summary:?}");
    let ms_per_file = wall.as_secs_f64() * 1e3 / files as f64;
    println!(
        "analyzed {files} 12 MP JPEGs in {:.2} s = {:.1} files/s on {} CPU workers",
        wall.as_secs_f64(),
        files as f64 / wall.as_secs_f64(),
        ps_scan::default_cpu_threads()
    );
    budget("scan-analysis-12mp", ms_per_file, "ms", 6.67);
}

#[test]
fn scenario_incremental_rescan_no_change_rescan_under_4_s() {
    let _cores = exclusive();
    let files = env_count("PS_SCAN_RESCAN_FILES", 100_000, 10_000);
    let root = cached(&format!("ps-scan-bench-tree-{files}"), |d| {
        // 100 files per folder, small non-media files (skipped after a probe on the first scan).
        for i in 0..files {
            let dir = d.join(format!("{:03}/{:02}", i / 10_000, (i / 100) % 100));
            if i % 100 == 0 {
                std::fs::create_dir_all(&dir).expect("mkdir");
            }
            ps_fixtures::scratch::write(dir.join(format!("f{i:06}.txt")), format!("file {i}\n"))
                .expect("write");
        }
    });
    let e = env();
    let id = e.add(&root);
    let first = e.scan(&[id]);
    completed(&first);
    assert_eq!(first.stages.total() as usize, files, "{first:?}"); // the marker sits beside the tree
    warm(&root);
    let mut times = Vec::new();
    for _ in 0..3 {
        e.instr.reset();
        let t = Instant::now();
        let s = e.scan(&[id]);
        times.push(t.elapsed());
        completed(&s);
        assert_eq!(s.diff.unchanged as usize, files);
        assert_eq!(e.instr.open_count(), 0, "nothing read");
    }
    println!("no-change rescans of {files} files: {times:?}");
    times.sort();
    let median = times[times.len() / 2];
    // Budget for 100,000 files; smaller (debug) trees are scaled up linearly.
    let scaled = median.as_secs_f64() * 100_000.0 / files as f64;
    budget("rescan-nochange-100k", scaled, "s", 4.0);
}

#[test]
fn scenario_scan_control_control_latency_within_budget() {
    let _cores = exclusive();
    let trials = env_count("PS_SCAN_LATENCY_TRIALS", 20, 20);
    let roots: Vec<PathBuf> = {
        let c = corpus();
        c.truth.drives.iter().map(|d| c.root.join(d)).collect()
    };
    let mut lat = Vec::new();
    let mut seed = 0x5EEDu64;
    for trial in 0..trials {
        let e = env();
        for r in &roots {
            e.add(r);
        }
        e.s().start_scan(&[]).expect("start");
        // A random moment during the analysis phase.
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let target = 0.05 + (seed >> 33) as f64 / (1u64 << 31) as f64 * 0.8;
        wait_snapshot(e.s(), Duration::from_secs(120), "progress", |s| {
            s.to_process > 0 && s.processed as f64 >= target * s.to_process as f64
                || s.status != RunStatus::Running
        });
        let pause = trial % 2 == 1;
        let t = Instant::now();
        let d = if pause {
            // Pause: until every reader and worker is idle.
            e.s().pause().expect("pause");
            wait_snapshot(e.s(), Duration::from_secs(10), "idle", |s| {
                s.active_workers == 0
            });
            let d = t.elapsed();
            e.s().cancel().expect("cancel");
            e.s().wait();
            d
        } else {
            e.s().cancel().expect("cancel");
            e.s().wait();
            t.elapsed()
        };
        lat.push(d);
        let snap = e.s().snapshot().expect("snapshot");
        assert_eq!(snap.active_workers, 0);
        assert!(
            e.files(e.s().sources().expect("s")[0].id)
                .values()
                .all(|f| f.stage != Stage::Analyzed || f.identity.is_some())
        );
    }
    println!("control latencies: {lat:?}");
    lat.sort();
    let p95 = lat[((lat.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)];
    budget("control-latency-p95", p95.as_secs_f64() * 1e3, "ms", 250.0);
}
