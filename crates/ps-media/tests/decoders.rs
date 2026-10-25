//! The decoder registry (ADR-0089): HEIC decodes through whatever decoder is passed in.

use std::sync::Arc;

use ps_media::decoders::fake::FakeHeif;
use ps_media::{DecodeError, Decoders, decode_for_analysis, probe_bytes};
use ps_model::{DecoderKey, Orientation};

const TINY: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

fn probe(b: &[u8]) -> ps_model::ProbeRecord {
    probe_bytes(b, b.len() as u64, None)
}

#[test]
fn reboxed_heif_carries_the_written_rotation() {
    let r = ps_fixtures::heif::heif_with_transforms(TINY, 1, None).expect("rebox");
    let p = probe(&r);
    assert_eq!(p.format, Some(ps_model::Format::Heic));
    assert_eq!(
        p.orientation,
        Orientation(8),
        "irot 1: 90 degrees anti-clockwise"
    );
    assert_eq!(p.stored, probe(TINY).stored);
    let m = ps_fixtures::heif::heif_with_transforms(TINY, 0, Some(0)).expect("rebox");
    assert_eq!(
        probe(&m).orientation,
        Orientation(2),
        "imir axis 0: left-right"
    );
    let both = ps_fixtures::heif::heif_with_transforms(TINY, 3, Some(0)).expect("rebox");
    assert_eq!(probe(&both).orientation, Orientation(5));
}

#[test]
fn scenario_decoder_support_orientation_applied_exactly_once() {
    let rotated = ps_fixtures::heif::heif_with_transforms(TINY, 1, None).expect("rebox");
    let p = probe(&rotated);
    let with = Decoders::with_heif(Arc::new(FakeHeif::new()));
    let without = Decoders::with_heif(Arc::new(FakeHeif::without_transforms()));
    let a = decode_for_analysis(&rotated, &p, &with)
        .expect("decode")
        .image;
    let b = decode_for_analysis(&rotated, &p, &without)
        .expect("decode")
        .image;
    // Stored 640x480 (landscape); upright after one 90 degree turn.
    assert_eq!((a.width, a.height), (480, 640));
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert_eq!(a, b, "the same upright image either way");
    // Unrotated input: neither path turns it.
    let p0 = probe(TINY);
    let c = decode_for_analysis(TINY, &p0, &without)
        .expect("decode")
        .image;
    assert_eq!((c.width, c.height), (640, 480));
}

#[test]
fn missing_and_failing_decoders() {
    let p = probe(TINY);
    assert_eq!(
        decode_for_analysis(TINY, &p, &Decoders::none()).err(),
        Some(DecodeError::DecoderMissing(DecoderKey::Heif))
    );
    let failing = Decoders::with_heif(Arc::new(FakeHeif::failing("corrupt HEVC data")));
    assert!(matches!(
        decode_for_analysis(TINY, &p, &failing),
        Err(DecodeError::Failed(m)) if m.contains("corrupt")
    ));
    assert!(Decoders::none().can_decode(DecoderKey::Avif));
    assert!(!Decoders::none().can_decode(DecoderKey::Heif));
    assert!(failing.can_decode(DecoderKey::Heif));
    assert_eq!(
        ps_media::decoders::missing_note(DecoderKey::Heif),
        "No HEIC decoder on this system"
    );
}

/// A given image per input is returned as is (the pattern a JPEG was resized from).
#[test]
fn fake_heif_returns_a_given_image() {
    let fake = FakeHeif::new();
    let img = ps_media::RgbImage {
        width: 4,
        height: 3,
        data: vec![7; 36],
    };
    fake.give(TINY, img.clone());
    let d = Decoders::with_heif(Arc::new(fake));
    let p = probe(TINY);
    assert_eq!(
        decode_for_analysis(TINY, &p, &d).expect("decode").image,
        img
    );
}
