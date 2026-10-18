//! Budget scenarios (ADR-0026). Each prints a `measure <budget>=<value> ms` line read by
//! `cargo xtask bench --check` (sources in perf-budgets.toml) and asserts the absolute budget.
//! Meaningful in release: `cargo test --release -p ps-hash --test budgets -- --test-threads=1`. Debug builds
//! (debug assertions, tests running in parallel) print the measurement but do not assert the budget.
#![allow(clippy::unwrap_used)]

mod common;

use std::hint::black_box;
use std::time::Instant;

use ps_hash::{ImageView, NearDupIndex, PixelLayout, Thresholds, perceptual_hash};
use ps_model::PerceptualHash;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Assert a budget in optimized builds only.
fn check_budget(ok: bool, msg: String) {
    if cfg!(debug_assertions) {
        if !ok {
            eprintln!("note (debug build, not asserted): {msg}");
        }
    } else {
        assert!(ok, "{msg}");
    }
}

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn random_hashes(n: usize, seed: u64) -> Vec<(u32, PerceptualHash)> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..n as u32)
        .map(|i| {
            (
                i,
                PerceptualHash {
                    phash: rng.random(),
                    dhash: rng.random(),
                    version: 1,
                },
            )
        })
        .collect()
}

#[test]
fn scenario_perceptual_hashing_hash_cost_within_budget() {
    // 512 px decodes (512x384 landscape, RGB8), as produced by the analysis downscale.
    let images: Vec<_> = (0..16).map(|s| common::photo(s, 512, 384)).collect();
    let mut per_image = Vec::new();
    // Round 0 warms caches, the cosine table and the CPU clock; it is not recorded.
    for round in 0..21 {
        for img in &images {
            let view =
                ImageView::new(img.width(), img.height(), PixelLayout::Rgb8, img.as_raw()).unwrap();
            let t = Instant::now();
            black_box(perceptual_hash(black_box(&view)));
            if round > 0 {
                per_image.push(t.elapsed().as_secs_f64() * 1e3);
            }
        }
    }
    let median = percentile(per_image, 0.5);
    println!("measure phash-dhash-per-image={median:.4} ms");
    check_budget(
        median <= 0.3,
        format!("median {median:.4} ms per image exceeds 0.3 ms"),
    );
}

#[test]
fn scenario_perceptual_hashing_lookup_within_budget() {
    let entries = random_hashes(1_000_000, 1);
    let index = NearDupIndex::build(1, entries.iter().copied()).unwrap();
    let queries = random_hashes(10_000, 2);
    let mut out = Vec::new();
    let mut times = Vec::with_capacity(queries.len());
    for (_, q) in &queries {
        out.clear();
        let t = Instant::now();
        index
            .query_into(black_box(q), Thresholds::DERIVATIVE, &mut out)
            .unwrap();
        times.push(t.elapsed().as_secs_f64() * 1e3);
        black_box(&out);
    }
    let p95 = percentile(times, 0.95);
    println!("measure near-dup-lookup-p95={p95:.4} ms");
    check_budget(p95 <= 1.0, format!("p95 lookup {p95:.4} ms exceeds 1 ms"));
}

#[test]
fn scenario_perceptual_hashing_index_build_under_one_second() {
    let entries = random_hashes(1_000_000, 3);
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let index = NearDupIndex::build(1, entries.iter().copied()).unwrap();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(black_box(index).len(), 1_000_000);
    }
    println!("measure near-dup-index-build={best:.2} ms");
    check_budget(best < 1000.0, format!("build took {best:.2} ms"));
}
