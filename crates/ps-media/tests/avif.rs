//! AVIF through the bundled AV1 decoder (ADR-0090), with no system library and no HEIC decoder in the registry.
//! Fixtures and their reference colours: `tests/data/README.md`.

use std::time::{Duration, Instant};

use ps_media::{Decoders, decode_for_analysis, probe_bytes};
use ps_model::Format;

const TINY: &[u8] = include_bytes!("data/tiny.avif");
const TINY10: &[u8] = include_bytes!("data/tiny10.avif");
const GRID: &[u8] = include_bytes!("data/grid.avif");
const ROT: &[u8] = include_bytes!("data/rot.avif");

const RED: [u8; 3] = [200, 40, 40];
const GREEN: [u8; 3] = [40, 180, 60];
const BLUE: [u8; 3] = [40, 60, 200];
const GREY: [u8; 3] = [128, 128, 128];
const TOLERANCE: i32 = 24;

fn decode(bytes: &[u8]) -> ps_media::RgbImage {
    let p = probe_bytes(bytes, bytes.len() as u64, None);
    assert_eq!(p.format, Some(Format::Avif));
    decode_for_analysis(bytes, &p, &Decoders::none())
        .expect("AVIF decodes")
        .image
}

fn near(got: [u8; 3], want: [u8; 3], what: &str) {
    for c in 0..3 {
        assert!(
            (i32::from(got[c]) - i32::from(want[c])).abs() <= TOLERANCE,
            "{what}: {got:?} vs {want:?}"
        );
    }
}

/// The four quadrant centres of an upright image of the pattern.
fn quadrants(img: &ps_media::RgbImage, what: &str) {
    let (w, h) = (img.width, img.height);
    near(img.pixel(w / 4, h / 4), RED, what);
    near(img.pixel(3 * w / 4, h / 4), GREEN, what);
    near(img.pixel(w / 4, 3 * h / 4), BLUE, what);
    near(img.pixel(3 * w / 4, 3 * h / 4), GREY, what);
}

#[test]
fn fixtures_probe_as_avif_with_their_sizes() {
    for (b, w, h) in [
        (TINY, 64, 48),
        (TINY10, 64, 48),
        (GRID, 128, 96),
        (ROT, 64, 48),
    ] {
        let p = probe_bytes(b, b.len() as u64, None);
        assert_eq!(p.format, Some(Format::Avif));
        assert_eq!(p.stored, Some(ps_model::Dimensions::new(w, h)));
    }
    let p = probe_bytes(ROT, ROT.len() as u64, None);
    assert_eq!(p.orientation, ps_model::Orientation(8), "irot 1 = 90 CCW");
}

#[test]
fn scenario_decoder_support_avif_decoded_without_system_libraries() {
    let img = decode(TINY);
    assert_eq!((img.width, img.height), (64, 48));
    quadrants(&img, "8-bit");
}

#[test]
fn scenario_decoder_support_ten_bit_avif_decoded() {
    let img = decode(TINY10);
    assert_eq!((img.width, img.height), (64, 48));
    assert_eq!(img.data.len(), 64 * 48 * 3, "8-bit RGB");
    quadrants(&img, "10-bit");
}

#[test]
fn scenario_decoder_support_grid_avif_assembled() {
    let img = decode(GRID);
    assert_eq!(
        (img.width, img.height),
        (128, 96),
        "the grid's declared size"
    );
    // Each tile is one quadrant of the source pattern.
    quadrants(&img, "grid");
    // Inside each tile, away from its centre too.
    near(img.pixel(8, 8), RED, "tile 0 corner");
    near(img.pixel(120, 88), GREY, "tile 3 corner");
}

#[test]
fn scenario_decoder_support_rotated_avif_upright() {
    let img = decode(ROT);
    assert_eq!(
        (img.width, img.height),
        (48, 64),
        "width and height swapped"
    );
    // 90 degrees anti-clockwise: the stored top row becomes the left column, read bottom up.
    near(img.pixel(12, 48), RED, "red bottom left");
    near(img.pixel(12, 15), GREEN, "green top left");
    near(img.pixel(36, 48), BLUE, "blue bottom right");
    near(img.pixel(36, 15), GREY, "grey top right");
}

/// A small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn scenario_decoder_support_malformed_avif_contained() {
    let samples = [TINY, TINY10, GRID, ROT];
    let mut rng = Rng(0x5EED_A1F0);
    let mut slowest = Duration::ZERO;
    let mut decoded = 0;
    let only: Option<usize> = std::env::var("AVIF_FUZZ_CASE")
        .ok()
        .and_then(|v| v.parse().ok());
    let n: usize = std::env::var("AVIF_FUZZ_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    for i in 0..n {
        let mut b = samples[i % samples.len()].to_vec();
        match rng.below(4) {
            // Flip a few bytes anywhere.
            0 | 1 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(b.len());
                    b[at] = rng.next() as u8;
                }
            }
            // Truncate.
            2 => b.truncate(rng.below(b.len())),
            // Overwrite a run with garbage.
            _ => {
                let at = rng.below(b.len());
                let n = rng.below(64).min(b.len() - at);
                for x in &mut b[at..at + n] {
                    *x = rng.next() as u8;
                }
            }
        }
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let t = Instant::now();
        let p = probe_bytes(&b, b.len() as u64, None);
        if decode_for_analysis(&b, &p, &Decoders::none()).is_ok() {
            decoded += 1;
        }
        let el = t.elapsed();
        slowest = slowest.max(el);
        assert!(el < Duration::from_secs(1), "case {i} took {el:?}");
    }
    eprintln!("slowest mutated AVIF: {slowest:?}; {decoded} of {n} still decoded");
}
