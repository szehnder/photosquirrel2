//! Near-duplicate index scenarios, checked against brute force.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use proptest::prelude::*;
use ps_hash::{NearDupIndex, Thresholds, hamming};
use ps_model::PerceptualHash;
use rand::seq::index::sample;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

fn h(phash: u64, dhash: u64) -> PerceptualHash {
    PerceptualHash {
        phash,
        dhash,
        version: 1,
    }
}

/// `x` with exactly `k` distinct random bits flipped.
fn flip(rng: &mut ChaCha8Rng, x: u64, k: usize) -> u64 {
    sample(rng, 64, k)
        .iter()
        .fold(x, |acc, b| acc ^ (1u64 << b))
}

fn brute_force(
    entries: &[(u32, PerceptualHash)],
    q: &PerceptualHash,
    t: Thresholds,
) -> BTreeSet<u32> {
    entries
        .iter()
        .filter(|(_, e)| {
            hamming(e.phash, q.phash) <= t.phash && hamming(e.dhash, q.dhash) <= t.dhash
        })
        .map(|(k, _)| *k)
        .collect()
}

#[test]
fn scenario_perceptual_hashing_index_recall_at_distance_7() {
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let mut entries: Vec<(u32, PerceptualHash)> = (0..100_000)
        .map(|i| (i, h(rng.random(), rng.random())))
        .collect();
    // 250 query hashes, each with planted neighbours at pHash distances 0 to 7.
    let queries: Vec<PerceptualHash> = (0..250).map(|_| h(rng.random(), rng.random())).collect();
    let mut planted = Vec::new();
    let mut next = 1_000_000u32;
    for (qi, q) in queries.iter().enumerate() {
        for d in 0..=7 {
            let n = h(flip(&mut rng, q.phash, d), rng.random());
            planted.push((qi, next, d));
            entries.push((next, n));
            next += 1;
        }
    }
    // Build from all but the last 2,000 entries; insert those incrementally (exercises both paths).
    let split = entries.len() - 2_000;
    let mut index = NearDupIndex::build(1, entries[..split].iter().copied()).unwrap();
    for &(k, e) in &entries[split..] {
        index.insert(k, e).unwrap();
    }
    assert_eq!(index.len(), entries.len());

    let t = Thresholds {
        phash: 7,
        dhash: 64,
    };
    let results: Vec<BTreeSet<u32>> = queries
        .iter()
        .map(|q| {
            let got = index.query(q, t).unwrap();
            let set: BTreeSet<u32> = got.iter().map(|c| c.key).collect();
            assert_eq!(set.len(), got.len(), "no duplicate candidates");
            set
        })
        .collect();
    for &(qi, key, d) in &planted {
        assert!(
            results[qi].contains(&key),
            "planted neighbour at distance {d} missed"
        );
    }
    // And nothing beyond brute force.
    for (q, got) in queries.iter().zip(&results).take(20) {
        assert_eq!(*got, brute_force(&entries, q, t));
    }
}

#[test]
fn scenario_perceptual_hashing_only_verified_candidates_returned() {
    let mut rng = ChaCha8Rng::seed_from_u64(8);
    let mut entries: Vec<(u32, PerceptualHash)> = (0..50_000)
        .map(|i| (i, h(rng.random(), rng.random())))
        .collect();
    let queries: Vec<PerceptualHash> = (0..200).map(|_| h(rng.random(), rng.random())).collect();
    let mut next = 1_000_000;
    for q in &queries {
        // Neighbours on both sides of both thresholds.
        for _ in 0..20 {
            let pd = rng.random_range(0..=7);
            let dd = rng.random_range(0..=14);
            entries.push((
                next,
                h(flip(&mut rng, q.phash, pd), flip(&mut rng, q.dhash, dd)),
            ));
            next += 1;
        }
    }
    let index = NearDupIndex::build(1, entries.iter().copied()).unwrap();
    let t = Thresholds::DERIVATIVE;
    assert_eq!((t.phash, t.dhash), (6, 8));
    let mut returned = 0;
    for q in &queries {
        let got = index.query(q, t).unwrap();
        for c in &got {
            let e = entries.iter().find(|(k, _)| *k == c.key).unwrap().1;
            assert_eq!(c.phash_distance, hamming(e.phash, q.phash));
            assert_eq!(c.dhash_distance, hamming(e.dhash, q.dhash));
            assert!(c.phash_distance <= 6 && c.dhash_distance <= 8, "{c:?}");
        }
        let keys: BTreeSet<u32> = got.iter().map(|c| c.key).collect();
        assert_eq!(keys, brute_force(&entries, q, t));
        returned += got.len();
    }
    assert!(
        returned > 1000,
        "the test plants enough true candidates ({returned})"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, cases: 64, ..ProptestConfig::default() })]

    /// The index returns exactly the brute-force answer for any thresholds within the recall radius,
    /// whether entries were bulk-built or inserted.
    #[test]
    fn index_matches_brute_force(
        seed in any::<u64>(),
        n in 0usize..3000,
        inserted in 0usize..500,
        tp in 0u32..=7,
        td in 0u32..=64,
    ) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let centre: u64 = rng.random();
        let dcentre: u64 = rng.random();
        // Clustered around one centre so many entries fall inside the thresholds.
        let entries: Vec<(u32, PerceptualHash)> = (0..(n + inserted) as u32)
            .map(|i| {
                let k = rng.random_range(0..=12);
                let kd = rng.random_range(0..=20);
                (i, h(flip(&mut rng, centre, k), flip(&mut rng, dcentre, kd)))
            })
            .collect();
        let mut index = NearDupIndex::build(1, entries[..n].iter().copied()).unwrap();
        for &(k, e) in &entries[n..] {
            index.insert(k, e).unwrap();
        }
        let t = Thresholds { phash: tp, dhash: td };
        for _ in 0..5 {
            let (kp, kd) = (rng.random_range(0..=6), rng.random_range(0..=6));
            let q = h(flip(&mut rng, centre, kp), flip(&mut rng, dcentre, kd));
            let got = index.query(&q, t).unwrap();
            let keys: BTreeSet<u32> = got.iter().map(|c| c.key).collect();
            prop_assert_eq!(keys.len(), got.len());
            prop_assert_eq!(keys, brute_force(&entries, &q, t));
        }
    }
}
