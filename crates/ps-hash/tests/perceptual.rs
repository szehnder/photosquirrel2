//! Perceptual hashing scenarios (openspec/changes/content-hashing/specs/perceptual-hashing).
#![allow(clippy::unwrap_used)]

mod common;

use common::{decode_jpeg, encode_jpeg, hash_rgb, photo};
use image::imageops::{self, FilterType};
use proptest::prelude::*;
use ps_hash::{VersionMismatch, compare, hamming};
use ps_model::PerceptualHash;

#[test]
fn scenario_perceptual_hashing_orientation_tag_matches_rotated_pixels() {
    for seed in 0..10 {
        let landscape = photo(100 + seed, 640, 480);
        // Stored landscape, tagged orientation 6 (display = rotate 90 degrees clockwise).
        let tagged = decode_jpeg(&encode_jpeg(&landscape, 90));
        // The same picture stored physically rotated, orientation 1.
        let rotated = decode_jpeg(&encode_jpeg(&imageops::rotate90(&landscape), 90));
        let a = hash_rgb(&tagged, 6);
        let b = hash_rgb(&rotated, 1);
        let d = compare(&a, &b).unwrap();
        assert!(d.phash <= 2 && d.dhash <= 2, "seed {seed}: {d:?}");
        // Without the tag the stored image is a different picture.
        let untagged = hash_rgb(&tagged, 1);
        assert!(hamming(untagged.phash, b.phash) > 6, "seed {seed}");
    }
}

#[test]
fn scenario_perceptual_hashing_resized_copy_stays_close() {
    let (mut worst_p, mut worst_d) = (0, 0);
    for seed in 0..40 {
        let original = decode_jpeg(&encode_jpeg(&photo(200 + seed, 1024, 768), 92));
        let small = imageops::resize(&original, 384, 288, FilterType::Triangle);
        let copy = decode_jpeg(&encode_jpeg(&small, 60));
        let d = compare(&hash_rgb(&original, 1), &hash_rgb(&copy, 1)).unwrap();
        worst_p = worst_p.max(d.phash);
        worst_d = worst_d.max(d.dhash);
        assert!(d.phash <= 6, "seed {seed}: pHash distance {}", d.phash);
        assert!(d.dhash <= 8, "seed {seed}: dHash distance {}", d.dhash);
    }
    println!("worst resized distances: pHash {worst_p}, dHash {worst_d}");
}

#[test]
fn scenario_perceptual_hashing_different_photos_far_apart() {
    let hashes: Vec<PerceptualHash> = (0..1000)
        .map(|s| hash_rgb(&photo(10_000 + s, 160, 120), 1))
        .collect();
    let mut min_p = u32::MAX;
    let mut min_d = u32::MAX;
    for i in 0..hashes.len() {
        for j in i + 1..hashes.len() {
            let d = compare(&hashes[i], &hashes[j]).unwrap();
            min_p = min_p.min(d.phash);
            min_d = min_d.min(d.dhash);
            assert!(
                !(d.phash <= 6 && d.dhash <= 8),
                "photos {i} and {j} collide: {d:?}"
            );
        }
    }
    println!("closest distinct pair distances: pHash {min_p}, dHash {min_d}");
}

#[test]
fn scenario_perceptual_hashing_version_mismatch_not_compared() {
    let v1 = hash_rgb(&photo(1, 64, 48), 1);
    assert_eq!(v1.version, ps_hash::PERCEPTUAL_HASH_VERSION);
    let v2 = PerceptualHash { version: 2, ..v1 };
    assert_eq!(
        compare(&v1, &v2),
        Err(VersionMismatch { left: 1, right: 2 })
    );
    assert_eq!(
        compare(&v2, &v1),
        Err(VersionMismatch { left: 2, right: 1 })
    );
    assert!(compare(&v1, &v1).is_ok());
}

fn arb_hash() -> impl Strategy<Value = PerceptualHash> {
    (any::<u64>(), any::<u64>()).prop_map(|(phash, dhash)| PerceptualHash {
        phash,
        dhash,
        version: 1,
    })
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn distance_is_zero_to_self(h in arb_hash()) {
        let d = compare(&h, &h).unwrap();
        prop_assert_eq!((d.phash, d.dhash), (0, 0));
    }

    #[test]
    fn distance_is_symmetric(a in arb_hash(), b in arb_hash()) {
        prop_assert_eq!(compare(&a, &b).unwrap(), compare(&b, &a).unwrap());
    }

    #[test]
    fn distance_obeys_triangle_inequality(a in any::<u64>(), b in any::<u64>(), c in any::<u64>()) {
        prop_assert!(hamming(a, c) <= hamming(a, b) + hamming(b, c));
        prop_assert!(hamming(a, b) <= 64);
    }

    #[test]
    fn hashing_is_deterministic(seed in 0u64..1000, w in 1u32..80, h in 1u32..80) {
        let img = photo(seed, w, h);
        prop_assert_eq!(hash_rgb(&img, 1), hash_rgb(&img.clone(), 1));
    }
}
