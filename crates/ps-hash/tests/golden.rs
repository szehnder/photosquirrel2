//! Golden perceptual hash values for algorithm version 1. Changing the algorithm requires bumping
//! `PERCEPTUAL_HASH_VERSION` and these values together (ADR-0012).
#![allow(clippy::unwrap_used)]

mod common;

use ps_hash::{ImageView, PERCEPTUAL_HASH_VERSION, PixelLayout, perceptual_hash};
use ps_model::{Orientation, PerceptualHash};

/// Reference fixtures, generated deterministically (no files, no randomness outside fixed seeds).
fn fixtures() -> Vec<(&'static str, PerceptualHash)> {
    // Luma gradient with a bright square.
    let (w, h) = (64u32, 48u32);
    let mut luma = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let inside = (20..40).contains(&x) && (10..30).contains(&y);
            luma.push(if inside { 250 } else { (x * 3 + y) as u8 });
        }
    }
    let gradient = perceptual_hash(&ImageView::new(w, h, PixelLayout::Luma8, &luma).unwrap());

    // Coloured checkerboard, RGBA.
    let (w, h) = (100u32, 75u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let on = ((x / 10) + (y / 15)) % 2 == 0;
            rgba.extend_from_slice(if on {
                &[200, 40, 40, 255]
            } else {
                &[20, 60, 180, 128]
            });
        }
    }
    let checker = perceptual_hash(&ImageView::new(w, h, PixelLayout::Rgba8, &rgba).unwrap());

    let photo = common::hash_rgb(&common::photo(1, 512, 384), 1);
    let portrait = common::hash_rgb(&common::photo(2, 333, 517), 8);
    let tiny = {
        let img = common::photo(3, 7, 5);
        let view = ImageView::new(7, 5, PixelLayout::Rgb8, img.as_raw())
            .unwrap()
            .with_orientation(Orientation(3));
        perceptual_hash(&view)
    };
    vec![
        ("gradient", gradient),
        ("checker", checker),
        ("photo", photo),
        ("portrait", portrait),
        ("tiny", tiny),
    ]
}

const GOLDEN: &[(&str, u64, u64)] = &[
    ("gradient", 0xb3b34cf34c4cb231, 0xffffffc7c7c7cfff),
    ("checker", 0x895180be49bf01ff, 0x5a5aa55a5aa55a5a),
    ("photo", 0x05baa503e974cf4d, 0xfcffffff7bf9fb73),
    ("portrait", 0x9269967d151551e7, 0x606141000203e3e3),
    ("tiny", 0x9cc82c37638cec67, 0xc9991366ec997666),
];

#[test]
fn scenario_perceptual_hashing_known_image_has_known_hashes() {
    let got = fixtures();
    for (name, h) in &got {
        // Printed so a deliberate version bump can paste the new values.
        println!("(\"{name}\", 0x{:016x}, 0x{:016x}),", h.phash, h.dhash);
        assert_eq!(h.version, PERCEPTUAL_HASH_VERSION);
    }
    assert_eq!(
        PERCEPTUAL_HASH_VERSION, 1,
        "bump the golden values with the version"
    );
    assert_eq!(got.len(), GOLDEN.len());
    for ((name, h), &(gname, phash, dhash)) in got.iter().zip(GOLDEN) {
        assert_eq!(*name, gname);
        assert_eq!(h.phash, phash, "{name} pHash");
        assert_eq!(h.dhash, dhash, "{name} dHash");
    }
}
