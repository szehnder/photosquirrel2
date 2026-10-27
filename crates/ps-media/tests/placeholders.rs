//! `thumbnails` scenarios of `preview-placeholders` (ADR-0109): every placeholder says why, with the decoder
//! generation that made it. Fixture files are written by the ps-fixtures writers into temp dirs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_fixtures::media as fx;
use ps_media::decoders::GENERATIONS;
use ps_media::{Decoders, make_thumbnail, probe_bytes};
use ps_model::{DecodeFamily, PlaceholderInfo, PlaceholderReason, ThumbKind};

/// Write `bytes` into a temp dir and read them back, as the scanner reads a source file.
fn on_disk(name: &str, bytes: &[u8]) -> (tempfile::TempDir, Vec<u8>) {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join(name);
    ps_fixtures::scratch::write(&path, bytes).expect("write");
    let read = std::fs::read(&path).expect("read");
    (tmp, read)
}

#[test]
fn scenario_thumbnails_bigtiff_placeholder_names_the_variant() {
    let (_tmp, bytes) = on_disk(
        "scan.tif",
        &ps_fixtures::tiff_image::bigtiff_rgb(640, 480, 9),
    );
    let probe = probe_bytes(&bytes, bytes.len() as u64, None);
    assert_eq!(probe.format, Some(ps_model::Format::Tiff), "recognised");
    assert_eq!(
        probe.display.map(|d| (d.width, d.height)),
        Some((640, 480)),
        "dimensions read"
    );
    let (t, _) = make_thumbnail(&bytes, &probe, &Decoders::none()).expect("a placeholder");
    assert_eq!(t.kind, ThumbKind::Placeholder);
    let p = t.placeholder.expect("info");
    assert_eq!(
        p.reason,
        Some(PlaceholderReason::UnsupportedVariant {
            detail: "BigTIFF".into()
        })
    );
    assert_eq!(p.generation, GENERATIONS.of(DecodeFamily::Tiff));
    assert_eq!(p.generation, 1, "the current TIFF generation");
}

#[test]
fn scenario_thumbnails_damaged_jpeg_placeholder_keeps_the_error() {
    let full = fx::jpeg(800, 600, 31, 90);
    // The data ends right after the scan header: every marker segment is there, no entropy-coded data.
    let sos = full
        .windows(2)
        .position(|w| w == [0xFF, 0xDA])
        .expect("SOS");
    let len = u16::from_be_bytes([full[sos + 2], full[sos + 3]]) as usize;
    let (_tmp, bytes) = on_disk("IMG_0001.JPG", &full[..sos + 2 + len]);
    let probe = probe_bytes(&bytes, bytes.len() as u64, None);
    assert!(probe.display.is_some(), "the headers give its dimensions");
    let err = ps_media::decode_for_analysis(&bytes, &probe, &Decoders::none())
        .expect_err("not decodable");
    let (t, _) = make_thumbnail(&bytes, &probe, &Decoders::none()).expect("a placeholder");
    assert_eq!(t.kind, ThumbKind::Placeholder);
    let p = t.placeholder.expect("info");
    match &p.reason {
        Some(PlaceholderReason::Unreadable { message }) => {
            assert!(!message.is_empty());
            assert!(
                err.to_string().contains(message.as_str()),
                "the decoder's error: {message} / {err}"
            );
        }
        other => panic!("expected could not be read, got {other:?}"),
    }
    assert_eq!(p.generation, GENERATIONS.of(DecodeFamily::Jpeg));
}

#[test]
fn scenario_thumbnails_old_placeholders_read_as_an_older_generation() {
    // A placeholder record as 0.2.0 stored it (no reason, no generation).
    let old = r#"{"media_kind":"image","format":"tiff","duration_ms":null,"capture_date":null,
        "note":"preview unavailable (file too large)"}"#;
    let p: PlaceholderInfo = serde_json::from_str(old).expect("old placeholder");
    assert_eq!(p.generation, 0);
    assert_eq!(p.note, "preview unavailable (file too large)");
    assert_eq!(p.reason, None);
    // And one written by heic-system-decoders, with a reason but no generation.
    let heic = r#"{"media_kind":"image","format":"heic","duration_ms":null,"capture_date":null,
        "note":"No HEIC decoder on this system","reason":{"kind":"decoder_missing","key":"heif"}}"#;
    let p: PlaceholderInfo = serde_json::from_str(heic).expect("heic placeholder");
    assert_eq!(p.generation, 0);
    assert_eq!(p.note, "No HEIC decoder on this system");
    // A reason this version does not know (written by a newer one) still reads.
    let newer = r#"{"media_kind":"image","format":"jpeg","duration_ms":null,"capture_date":null,
        "note":"x","reason":{"kind":"hdr_gain_map","layers":2},"generation":4}"#;
    let p: PlaceholderInfo = serde_json::from_str(newer).expect("newer placeholder");
    assert_eq!(p.reason, Some(PlaceholderReason::NeedsNewerVersion));
    assert_eq!(p.generation, 4);
}
