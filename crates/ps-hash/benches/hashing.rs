//! Criterion benchmarks for the hashing budgets (ADR-0026): pHash + dHash per 512 px decode, near-duplicate
//! lookup over 1M hashes and index build. The budget checks themselves read `tests/budgets.rs`.
#![allow(clippy::unwrap_used)]

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use ps_hash::{
    ImageView, NearDupIndex, PixelLayout, Thresholds, identity_from_buffer, perceptual_hash,
};
use ps_model::PerceptualHash;
use rand::{Rng, RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;

fn image_512(seed: u64) -> Vec<u8> {
    // Smooth random content; the cost does not depend on pixel values.
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let (w, h) = (512usize, 384usize);
    let (a, b, c): (f32, f32, f32) = (rng.random(), rng.random(), rng.random());
    let mut v = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let t = (x as f32 * a / 37.0).sin() + (y as f32 * b / 23.0).cos();
            v.extend_from_slice(&[(t * 60.0 + 128.0) as u8, (c * 255.0) as u8, (x ^ y) as u8]);
        }
    }
    v
}

fn hashes(n: usize, seed: u64) -> Vec<(u32, PerceptualHash)> {
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

fn bench(c: &mut Criterion) {
    let px = image_512(1);
    let view = ImageView::new(512, 384, PixelLayout::Rgb8, &px).unwrap();
    c.bench_function("phash-dhash-per-image", |b| {
        b.iter(|| perceptual_hash(black_box(&view)))
    });
    c.bench_function("luminance32-512", |b| {
        b.iter(|| ps_hash::perceptual::luminance32(black_box(&view)))
    });
    let gray: Vec<u8> = px.as_chunks::<3>().0.iter().map(|p| p[0]).collect();
    let gray_view = ImageView::new(512, 384, PixelLayout::Luma8, &gray).unwrap();
    c.bench_function("luminance32-512-gray", |b| {
        b.iter(|| ps_hash::perceptual::luminance32(black_box(&gray_view)))
    });
    let luma = ps_hash::perceptual::luminance32(&view);
    c.bench_function("phash-from-luma", |b| {
        b.iter(|| ps_hash::perceptual::phash_from_luma(black_box(&luma)))
    });
    c.bench_function("dhash-from-luma", |b| {
        b.iter(|| ps_hash::perceptual::dhash_from_luma(black_box(&luma)))
    });

    let entries = hashes(1_000_000, 2);
    let index = NearDupIndex::build(1, entries.iter().copied()).unwrap();
    let queries = hashes(1024, 3);
    let mut i = 0;
    let mut out = Vec::new();
    c.bench_function("near-dup-lookup-1m", |b| {
        b.iter(|| {
            i = (i + 1) % queries.len();
            out.clear();
            index
                .query_into(black_box(&queries[i].1), Thresholds::DERIVATIVE, &mut out)
                .unwrap();
            out.len()
        })
    });

    let mut g = c.benchmark_group("slow");
    g.sample_size(10);
    g.bench_function("near-dup-index-build-1m", |b| {
        b.iter_batched(
            || entries.clone(),
            |e| NearDupIndex::build(1, e).unwrap(),
            BatchSize::LargeInput,
        )
    });
    let mut buf = vec![0u8; 8 << 20];
    ChaCha8Rng::seed_from_u64(4).fill_bytes(&mut buf);
    g.throughput(criterion::Throughput::Bytes(buf.len() as u64));
    g.bench_function("blake3-identity-8mb", |b| {
        b.iter(|| identity_from_buffer(black_box(&buf)))
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
