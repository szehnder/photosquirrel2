//! Grouping cost at catalog scale: 1M synthetic inputs (ADR-0026 style `measure` line; no budget yet).
//! Run in release: `cargo test --release -p ps-dedup --test scale -- --ignored --nocapture`.
#![allow(clippy::unwrap_used)]

use std::time::Instant;

use ps_dedup::{Constraints, DedupInput, DedupMeta, Grouper, LiveGrouping};
use ps_hash::PERCEPTUAL_HASH_VERSION;
use ps_model::{CaptureTime, ContentId, FileId, Format, PerceptualHash, PhotoClass};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Shots with an exact copy and a resized copy each (~1/3 of the inputs are distinct shots).
fn synthetic(n: usize, seed: u64) -> Vec<DedupInput> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(n);
    let mut shot = 0u64;
    while out.len() < n {
        shot += 1;
        let (p, d): (u64, u64) = (rng.random(), rng.random());
        let t = CaptureTime {
            local_seconds: 1_300_000_000 + rng.random_range(0..400_000_000),
            millis: 0,
            offset_minutes: None,
        };
        let mut cid = [0u8; 32];
        cid[..8].copy_from_slice(&shot.to_le_bytes());
        let base = DedupInput {
            file: FileId(out.len() as i64 + 1),
            content: Some(ContentId::full(cid)),
            hash: Some(PerceptualHash {
                phash: p,
                dhash: d,
                version: PERCEPTUAL_HASH_VERSION,
            }),
            format: Format::Jpeg,
            class: PhotoClass::Photo,
            path: format!("Pictures/{}/IMG_{shot}.JPG", shot % 997),
            size: 4_000_000,
            mtime_ns: t.local_seconds * 1_000_000_000,
            meta: DedupMeta {
                width: Some(4000),
                height: Some(3000),
                capture_time: Some(t),
                camera_model: Some("Model".into()),
                has_maker_notes: true,
                camera_original: true,
                jpeg_quality: Some(93),
                ..Default::default()
            },
        };
        let mut copy = base.clone();
        copy.file = FileId(base.file.0 + 1);
        copy.path = format!("Backup/{}", base.path);
        let mut small = base.clone();
        small.file = FileId(base.file.0 + 2);
        cid[8] = 1;
        small.content = Some(ContentId::full(cid));
        small.hash = Some(PerceptualHash {
            phash: p ^ 0b101,
            dhash: d ^ 0b11,
            version: PERCEPTUAL_HASH_VERSION,
        });
        small.meta.width = Some(1024);
        small.meta.height = Some(768);
        small.meta.has_maker_notes = false;
        small.meta.camera_original = false;
        small.path = format!("Exports/IMG_{shot}_1024.jpg");
        out.extend([base, copy, small]);
    }
    out.truncate(n);
    out
}

#[test]
#[ignore = "1M inputs; run explicitly in release"]
fn group_one_million_inputs() {
    let inputs = synthetic(1_000_000, 7);
    let cons = Constraints::default();
    let t = Instant::now();
    let grouper = Grouper::new(&inputs, &cons);
    let built = t.elapsed().as_secs_f64() * 1e3;
    let g = grouper.group_all();
    let total = t.elapsed().as_secs_f64() * 1e3;
    println!("measure dedup-group-1m={total:.1} ms (index {built:.1} ms)");
    assert!(g.groups.len() >= 333_000, "{} groups", g.groups.len());
    // Incremental: 1,000 new files regrouped against the full grouping.
    let more = synthetic(1_003, 8)
        .into_iter()
        .enumerate()
        .map(|(i, mut x)| {
            x.file = FileId(2_000_000 + i as i64);
            x.content = x.content.map(|mut c| {
                c.digest[31] = 0xAA;
                c
            });
            x
        })
        .collect::<Vec<_>>();
    let changed: Vec<FileId> = more.iter().map(|x| x.file).collect();
    let mut all = inputs.clone();
    all.extend(more.iter().cloned());
    let t = Instant::now();
    let grouper = Grouper::new(&all, &cons);
    let inc = grouper.regroup(&g, &changed);
    println!(
        "measure dedup-regroup-1k-of-1m-rebuild={:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    assert!(inc.groups.len() > g.groups.len());

    // The long-lived engine path (ADR-0026 budget `dedup-regroup-1k-of-1m`): indexes updated in place, only
    // the affected groups recomputed.
    let mut owned = Grouper::owned(inputs, cons.clone());
    let mut live = LiveGrouping::from_grouping(&g);
    let t = Instant::now();
    for x in more {
        owned.upsert(x);
    }
    let delta = owned.regroup_live(&mut live, &changed);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!("measure dedup-regroup-1k-of-1m={ms:.1} ms");
    assert_eq!(live.len(), inc.groups.len());
    assert!(delta.groups.len() >= 333);
    assert!(ms < 1000.0, "live regroup took {ms:.1} ms");
}

/// Budget `dedup-regroup-1k-of-1m` (ADR-0026): the scan's post-stage regroup of 1,000 new files against a
/// grouped catalog of 1M, with the long-lived engine (indexes updated in place, only affected groups
/// recomputed). Debug builds use 30k inputs and report without asserting.
#[test]
fn regroup_1k_of_1m_within_budget() {
    let n = if cfg!(debug_assertions) {
        30_000
    } else {
        1_000_000
    };
    let inputs = synthetic(n, 7);
    let cons = Constraints::default();
    let mut owned = Grouper::owned(inputs, cons);
    let g = owned.group_all();
    let mut live = LiveGrouping::from_grouping(&g);
    let more: Vec<DedupInput> = synthetic(1_000, 8)
        .into_iter()
        .enumerate()
        .map(|(i, mut x)| {
            x.file = FileId(2_000_000 + i as i64);
            x.content = x.content.map(|mut c| {
                c.digest[31] = 0xAA;
                c
            });
            x
        })
        .collect();
    let changed: Vec<FileId> = more.iter().map(|x| x.file).collect();
    let t = Instant::now();
    for x in more {
        owned.upsert(x);
    }
    let delta = owned.regroup_live(&mut live, &changed);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!("measure dedup-regroup-1k-of-1m={ms:.3} ms");
    assert!(delta.groups.len() >= 333);
    if !cfg!(debug_assertions) {
        assert!(ms <= 250.0, "regroup took {ms:.1} ms (budget 250 ms)");
    }
}
