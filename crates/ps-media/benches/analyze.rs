//! Analysis-stage benchmarks on 12 MP JPEGs (ADR-0026): probe, analysis decode, decode + grid thumbnail.
//! `cargo bench -p ps-media --bench analyze`. The budget itself is checked by the
//! `scenario_thumbnails_decode_within_per_file_budget` test (`cargo xtask bench --check`).

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use ps_fixtures::media as fx;

fn bench(c: &mut Criterion) {
    let jpeg = fx::jpeg_with_exif(4000, 3000, 1, 90, &fx::ExifSpec::canon_5d());
    let probe = ps_media::probe_bytes(&jpeg, jpeg.len() as u64, None);
    let mut g = c.benchmark_group("analyze-12mp-jpeg");
    g.sample_size(30);
    g.bench_function("probe", |b| {
        b.iter(|| {
            ps_media::probe_bytes(
                black_box(&jpeg[..ps_media::PROBE_LIMIT.min(jpeg.len())]),
                jpeg.len() as u64,
                None,
            )
        })
    });
    g.bench_function("analysis-decode", |b| {
        b.iter(|| {
            ps_media::decode_for_analysis(black_box(&jpeg), &probe, &ps_media::Decoders::none())
        })
    });
    g.bench_function("decode-thumb", |b| {
        b.iter(|| ps_media::analyze_still(black_box(&jpeg), &probe, &ps_media::Decoders::none()))
    });
    g.bench_function("metadata", |b| {
        b.iter(|| {
            ps_media::extract_metadata(
                black_box(&jpeg),
                &probe,
                None,
                &ps_media::DateContext {
                    rel_path: None,
                    zone: &ps_media::FixedZone(0),
                },
            )
        })
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
