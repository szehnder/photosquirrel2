//! Corrupt and truncated JPEGs (ADR-0055): the fast mozjpeg path must not turn them into "successful" grey-padded
//! decodes; they fail exactly as jpeg-decoder fails, as before ADR-0054. Intact files keep the fast path.

use ps_fixtures::media as fx;
use ps_media::decode::{self, DecodeError, decode_jpeg};
use ps_media::{decode_for_analysis, probe_bytes};

const W: u32 = 2400;
const H: u32 = 1800;

/// Encode with libjpeg (mozjpeg's encoder): baseline or progressive, chroma sampled as `chroma` pixel sizes.
fn encode(progressive: bool, chroma: (u8, u8), seed: u64) -> Vec<u8> {
    let rgb = fx::photo_rgb(W, H, seed);
    std::panic::catch_unwind(|| {
        let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        c.set_fastest_defaults();
        c.set_size(W as usize, H as usize);
        c.set_quality(88.0);
        c.set_chroma_sampling_pixel_sizes(chroma, chroma);
        if progressive {
            c.set_progressive_mode();
        }
        let mut s = c.start_compress(Vec::new()).expect("start");
        s.write_scanlines(&rgb).expect("scanlines");
        s.finish().expect("finish")
    })
    .expect("encode")
}

/// Every layout a camera or editor writes, as (label, bytes).
fn intact() -> Vec<(String, Vec<u8>)> {
    let mut out = vec![("image-crate baseline".to_owned(), fx::jpeg(W, H, 3, 85))];
    for progressive in [false, true] {
        for (name, chroma) in [("4:2:0", (2, 2)), ("4:2:2", (2, 1)), ("4:4:4", (1, 1))] {
            let kind = if progressive {
                "progressive"
            } else {
                "baseline"
            };
            out.push((format!("{kind} {name}"), encode(progressive, chroma, 5)));
        }
    }
    out
}

/// What jpeg-decoder (the decoder before ADR-0054) makes of `bytes` at 1/`denom`.
fn jpeg_decoder_fails(bytes: &[u8], denom: u32) -> bool {
    let r = std::panic::catch_unwind(|| {
        let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
        d.read_info()?;
        let info = d.info().expect("info");
        d.scale(
            info.width.div_ceil(denom as u16),
            info.height.div_ceil(denom as u16),
        )?;
        d.decode()
    });
    !matches!(r, Ok(Ok(_)))
}

/// The bytes up to and including the first start-of-scan segment.
fn through_sos(j: &[u8]) -> usize {
    let at = j
        .windows(2)
        .position(|w| w == [0xFF, 0xDA])
        .expect("SOS marker");
    let len = usize::from(u16::from_be_bytes([j[at + 2], j[at + 3]]));
    at + 2 + len
}

/// Corrupted copies of `j`: truncated at 10/50/90/99%, header plus 5 KB garbage, and garbage over the middle of
/// the scan with the tail (and EOI) intact.
fn corrupted(j: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for pct in [10usize, 50, 90, 99] {
        out.push((
            format!("truncated at {pct}%"),
            fx::truncate(j, j.len() * pct / 100),
        ));
    }
    let head = through_sos(j);
    let mut garbage = j[..head].to_vec();
    garbage.extend_from_slice(&fx::random_bytes(5 * 1024, 11));
    out.push(("header + 5 KB garbage".into(), garbage));
    let mut middle = j.to_vec();
    let mid = head + (j.len() - head) / 2;
    middle[mid..mid + 4096].copy_from_slice(&fx::random_bytes(4096, 12));
    out.push(("garbage mid-scan, EOI intact".into(), middle));
    out
}

#[test]
fn intact_jpegs_take_the_fast_path() {
    for (label, j) in intact() {
        let fast = decode::decode_mozjpeg_scaled(&j, 4).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!((fast.width, fast.height), (W / 4, H / 4), "{label}");
        let (img, denom) = decode_jpeg(&j, 512).expect(&label);
        assert_eq!(denom, 4, "{label}");
        assert_eq!(img, fast, "{label}: decode_jpeg returns the mozjpeg decode");
        // Data after EOI (cameras and editors leave padding) is not corruption.
        let padded = fx::pad_to(&j, j.len() + 4096);
        assert!(
            decode::decode_mozjpeg_scaled(&padded, 4).is_ok(),
            "{label}: trailing padding"
        );
    }
}

#[test]
fn truncated_and_corrupt_jpegs_fail_as_jpeg_decoder_does() {
    for (label, j) in intact() {
        for (how, bad) in corrupted(&j) {
            assert!(
                decode::decode_mozjpeg_scaled(&bad, 4).is_err(),
                "{label}, {how}: the fast path must not pad corrupt data with grey"
            );
            let ours = decode_jpeg(&bad, 512);
            let before = jpeg_decoder_fails(&bad, 4);
            assert_eq!(
                ours.is_err(),
                before,
                "{label}, {how}: fails exactly when jpeg-decoder fails ({ours:?})"
            );
            if how.starts_with("truncated") || how.starts_with("header") {
                assert!(
                    matches!(ours, Err(DecodeError::Failed(_))),
                    "{label}, {how}: {ours:?}"
                );
                let p = probe_bytes(&bad, bad.len() as u64, None);
                assert!(
                    decode_for_analysis(&bad, &p, &ps_media::Decoders::none()).is_err(),
                    "{label}, {how}: no analysis decode"
                );
            }
        }
    }
}

#[test]
fn small_truncated_jpegs_fail_too() {
    // Under 1023 px on the long edge analysis decodes at full size through zune-jpeg, not mozjpeg; it must not pad
    // a truncated file either.
    for (w, h) in [(640u32, 480u32), (800, 600), (1000, 750)] {
        let j = fx::jpeg(w, h, 9, 85);
        let p = probe_bytes(&j, j.len() as u64, None);
        assert!(
            decode_for_analysis(&j, &p, &ps_media::Decoders::none()).is_ok(),
            "{w}x{h} intact"
        );
        for pct in [10usize, 50, 90, 99] {
            let bad = fx::truncate(&j, j.len() * pct / 100);
            assert!(
                decode_jpeg(&bad, 512).is_err(),
                "{w}x{h} truncated at {pct}%"
            );
            let p = probe_bytes(&bad, bad.len() as u64, None);
            assert!(
                decode_for_analysis(&bad, &p, &ps_media::Decoders::none()).is_err(),
                "{w}x{h} truncated at {pct}%: no analysis decode"
            );
        }
    }
}

#[test]
fn large_progressive_jpegs_are_serialized_or_refused() {
    use decode::JpegMemoryPlan::{Parallel, Serialized, TooLarge};
    // Sequential decodes stream MCU rows: no whole-image buffer at any size.
    assert_eq!(decode::jpeg_memory_plan(20_000, 12_500, 3, false), Parallel);
    // 12 MP progressive: 72 MB of coefficients (4:4:4 bound), decoded on every worker at once.
    assert_eq!(
        decode::progressive_coefficient_bytes(4000, 3000, 3),
        4000 * 3000 * 6
    );
    assert_eq!(decode::jpeg_memory_plan(4000, 3000, 3, true), Parallel);
    // 50 MP progressive: 300 MB, one at a time.
    assert_eq!(decode::jpeg_memory_plan(8660, 5773, 3, true), Serialized);
    // 250 MP 4:4:4 progressive: 1.5 GB, still decoded, alone.
    assert_eq!(
        decode::jpeg_memory_plan(20_000, 12_500, 3, true),
        Serialized
    );
    // A hostile 65535 x 65535 progressive header: 25 GB, refused before any decoder allocates.
    assert_eq!(decode::jpeg_memory_plan(65_535, 65_535, 3, true), TooLarge);
    assert_eq!(decode::jpeg_memory_plan(65_535, 65_535, 1, true), TooLarge);
    // The thresholds are inclusive upper bounds.
    let edge = decode::PROGRESSIVE_SERIAL_BYTES / 128;
    let side = (edge as f64).sqrt() as u32 * 8;
    assert!(
        decode::progressive_coefficient_bytes(side, side, 1) <= decode::PROGRESSIVE_SERIAL_BYTES
    );
    assert_eq!(decode::jpeg_memory_plan(side, side, 1, true), Parallel);
}
