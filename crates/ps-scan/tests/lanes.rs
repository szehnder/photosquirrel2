//! Adaptive I/O lanes and bounded memory (scanning tasks 3.4, 3.5).

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use ps_model::{DeviceClass, Stage};
use ps_scan::ScanOptions;
use ps_scan::lane::{LaneController, SimulatedDevice, Window, simulate};

#[test]
fn scenario_scanning_lane_starts_by_device_class() {
    let _cores = shared();
    let c = corpus();
    let e = env_with(ScanOptions {
        // drive-a is "detected" as an SSD, drive-c as unknown.
        device_class: Some(Arc::new(|p: &Path| {
            if p.ends_with("drive-a") {
                DeviceClass::SolidState
            } else {
                DeviceClass::Unknown
            }
        })),
        read_delay: Some(Duration::from_millis(20)),
        ..ScanOptions::default()
    });
    let a = e.add(&c.root.join("drive-a"));
    let b = e.add(&c.root.join("drive-c"));
    e.s().start_scan(&[]).expect("start");
    let snap = wait_snapshot(e.s(), Duration::from_secs(60), "lanes started", |s| {
        s.sources.iter().all(|p| p.readers > 0)
    });
    let readers = |id| {
        snap.sources
            .iter()
            .find(|p| p.source == id)
            .expect("source")
            .readers
    };
    assert_eq!(readers(a), 8, "solid-state lane");
    assert_eq!(readers(b), 2, "unknown-device lane");
    let devices: Vec<_> = snap.sources.iter().map(|p| p.device).collect();
    assert_eq!(devices, vec![DeviceClass::SolidState, DeviceClass::Unknown]);
    e.s().cancel().expect("cancel");
    e.s().wait();
}

/// Throughput rises linearly up to 6 readers and is flat after; per-chunk latency is constant until the
/// device saturates and then grows with the queue.
struct RisesToSix;

impl SimulatedDevice for RisesToSix {
    fn window(&self, readers: usize) -> Window {
        let effective = readers.min(6) as f64;
        Window {
            bytes_per_s: effective * 100e6,
            p90_latency: Duration::from_secs_f64(0.004 * readers as f64 / effective),
        }
    }
}

#[test]
fn scenario_scanning_lane_grows_while_throughput_improves() {
    let mut c = LaneController::new(DeviceClass::Unknown.initial_readers());
    let trace = simulate(&RisesToSix, &mut c, 80);
    println!("readers per window: {trace:?}");
    assert!(
        trace[..12].contains(&6),
        "reaches the knee quickly: {trace:?}"
    );
    let settled = &trace[20..];
    assert!(
        settled.iter().all(|r| (5..=7).contains(r)),
        "settles between 5 and 7 readers: {settled:?}"
    );
}

/// A spinning disk: two readers keep it streaming; more make it seek, halving throughput while per-chunk
/// latency more than doubles.
struct Rotational;

impl SimulatedDevice for Rotational {
    fn window(&self, readers: usize) -> Window {
        let t = 120e6;
        let thr = if readers <= 2 {
            t * readers as f64 / 2.0
        } else {
            t / 2.0
        };
        let chunk = 1_048_576.0;
        Window {
            bytes_per_s: thr,
            p90_latency: Duration::from_secs_f64(readers as f64 * chunk / thr),
        }
    }
}

#[test]
fn scenario_scanning_lane_shrinks_on_latency_rise() {
    let mut c = LaneController::new(4);
    let trace = simulate(&Rotational, &mut c, 80);
    println!("readers per window: {trace:?}");
    let first = trace.iter().position(|r| *r <= 2).expect("shrinks");
    assert!(first < 15, "shrinks to 2 within 15 windows: {trace:?}");
    let settled = &trace[first..];
    let at_most_two = settled.iter().filter(|r| **r <= 2).count();
    assert!(
        at_most_two * 10 >= settled.len() * 8,
        "stays at <= 2 readers (brief probes aside): {settled:?}"
    );
    assert!(settled.iter().all(|r| *r <= 3), "{settled:?}");
    assert!(*trace.last().expect("last") <= 2 || trace[trace.len() - 2] <= 2);
}

/// Camera JPEGs padded (sparsely) to `size` bytes: full reads of `size` bytes, cheap to decode.
fn padded_jpegs(dir: &Path, n: usize, size: u64, seed: u64) {
    std::fs::create_dir_all(dir).expect("mkdir");
    for i in 0..n {
        let jpeg = ps_fixtures::media::jpeg_with_exif(
            640,
            480,
            seed * 1000 + i as u64,
            85,
            &ps_fixtures::media::ExifSpec::canon_5d(),
        );
        let p = dir.join(format!("p{i:03}.jpg"));
        ps_fixtures::scratch::write(&p, &jpeg).expect("write");
        ps_fixtures::scratch::pad(&p, size).expect("pad");
    }
}

#[test]
fn scenario_scanning_memory_cap_respected() {
    let _cores = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let size = 40 * 1024 * 1024;
    let e = env_with(ScanOptions {
        device_class: Some(Arc::new(|_: &Path| DeviceClass::SolidState)),
        // Slow consumers so buffers pile up against the caps.
        cpu_threads: 1,
        ..ScanOptions::default()
    });
    let mut ids = Vec::new();
    for lane in 0..3u64 {
        let root = tmp.path().join(format!("lane-{lane}"));
        padded_jpegs(&root, 16, size, lane);
        ids.push(e.add(&root));
    }
    let summary = e.scan(&ids);
    completed(&summary);
    let (peak, per_reader) = e.s().buffer_peaks().expect("peaks");
    println!("peak in-flight {peak} bytes, peak {per_reader} buffers per reader");
    assert!(
        peak <= 512 * 1024 * 1024,
        "in-flight bytes {peak} exceed 512 MB"
    );
    assert!(per_reader <= 2, "{per_reader} buffers for one reader");
    assert!(
        peak >= 8 * size,
        "the cap was actually under pressure ({peak} bytes peak)"
    );
    for id in ids {
        assert!(e.files(id).values().all(|f| f.stage == Stage::Thumbed));
    }
}
