//! `thumbnails` scenarios of `damaged-raw-reason` (ADR-0115): a RAW file cut short is damaged ("could not be
//! read"), told apart from RAWs this version does not support. Fixture files are written by the ps-fixtures writers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_fixtures::dng::{Cut, cut, raw_ranges};
use ps_media::decode::{DecodeError, RawlerOutcome, decode_raw_preview, raw_error, rawler_preview};
use ps_media::decoders::GENERATIONS;
use ps_media::raw_check::{size_text, truncation};
use ps_media::{Decoders, analyze_still, make_thumbnail, probe_bytes};
use ps_model::{DecodeFamily, PlaceholderReason, ThumbKind};

fn burst_cr2() -> Vec<u8> {
    ps_fixtures::dng::converted_burst_frame(5, 0, 0, 960).cr2
}

fn probe(b: &[u8]) -> ps_model::ProbeRecord {
    probe_bytes(
        &b[..b.len().min(ps_media::PROBE_LIMIT)],
        b.len() as u64,
        None,
    )
}

/// The placeholder `make_thumbnail` gives `bytes`, with its reason.
fn placeholder(bytes: &[u8]) -> ps_model::PlaceholderInfo {
    let p = probe(bytes);
    let (t, _) = make_thumbnail(bytes, &p, &Decoders::none()).expect("a thumbnail record");
    assert_eq!(t.kind, ThumbKind::Placeholder, "no pixels");
    t.placeholder.expect("placeholder info")
}

fn unreadable_message(p: &ps_model::PlaceholderInfo) -> String {
    match &p.reason {
        Some(PlaceholderReason::Unreadable { message }) => message.clone(),
        other => panic!("expected could not be read, got {other:?}"),
    }
}

#[test]
fn scenario_thumbnails_raw_cut_inside_its_preview_is_damaged() {
    for full in [burst_cr2(), ps_fixtures::dng::cr2_unknown_lens(11)] {
        let b = cut(&full, Cut::InPreview).unwrap();
        let t = truncation(&b, &probe(&b)).expect("evidence");
        let p = placeholder(&b);
        let m = unreadable_message(&p);
        assert!(m.contains(&size_text(b.len() as u64)), "{m}");
        assert!(m.contains(&size_text(t.needed)), "{m}");
        assert!(m.contains("cut short"), "{m}");
        assert_eq!(p.generation, GENERATIONS.of(DecodeFamily::Raw));
        assert_eq!(p.generation, 1, "RAW decoder generation 1");
    }
}

#[test]
fn raw_cut_after_an_intact_preview_has_no_image_and_no_hashes() {
    let full = burst_cr2();
    let b = cut(&full, Cut::AfterPreview).unwrap();
    let ((p_off, p_len), _) = raw_ranges(&full).unwrap();
    let pr = probe(&b);
    // The probe still finds the whole preview, and it would decode.
    let range = pr.preview.expect("the preview is whole").range;
    assert_eq!((range.offset, range.len), (p_off as u64, p_len as u64));
    assert!(ps_media::decode::decode_jpeg(range.slice(&b).unwrap(), 512).is_ok());
    // The analysis gives no image (so no perceptual hashes), and the placeholder says why.
    let err = analyze_still(&b, &pr, &Decoders::none()).expect_err("damaged");
    assert!(matches!(err, DecodeError::Failed(_)), "{err:?}");
    let m = unreadable_message(&placeholder(&b));
    assert!(m.contains("image data"), "{m}");
    // The complete file decodes its preview.
    assert!(analyze_still(&full, &probe(&full), &Decoders::none()).is_ok());
}

#[test]
fn scenario_thumbnails_unknown_camera_is_still_not_supported() {
    let b = ps_fixtures::dng::cr2_unknown_camera(3);
    let pr = probe(&b);
    assert_eq!(
        pr.format,
        Some(ps_model::Format::Raw(ps_model::RawFormat::Cr2))
    );
    assert!(pr.preview.is_none(), "no usable embedded preview");
    assert_eq!(truncation(&b, &pr), None, "complete");
    let p = placeholder(&b);
    match &p.reason {
        Some(PlaceholderReason::UnsupportedVariant { detail }) => {
            assert!(detail.contains("Canon"), "{detail}");
            assert!(
                detail.contains(ps_fixtures::dng::UNKNOWN_CAMERA_MODEL),
                "{detail}"
            );
        }
        other => panic!("expected a variant not supported, got {other:?}"),
    }
}

/// Every complete RAW the ps-fixtures writers make, and every one in the corpus presets' writers.
fn complete_raws() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    for seed in [1u64, 7, 42] {
        let f = ps_fixtures::dng::converted_burst_frame(seed, 1, 30, 1200);
        v.push((format!("burst cr2 {seed}"), f.cr2));
        v.push((format!("burst dng {seed}"), f.dng));
        let c = ps_fixtures::dng::converted_burst_frame_camera_info(seed, 2, 0, 800);
        v.push((format!("camera-info cr2 {seed}"), c.cr2));
        v.push((format!("camera-info dng {seed}"), c.dng));
        let r = ps_fixtures::dng::converted_burst_frame_with(seed, 0, 10, 640, 1);
        v.push((format!("rendered-as dng {seed}"), r.dng));
        v.push((
            format!("unknown lens {seed}"),
            ps_fixtures::dng::cr2_unknown_lens(seed),
        ));
        v.push((
            format!("unknown camera {seed}"),
            ps_fixtures::dng::cr2_unknown_camera(seed),
        ));
    }
    v
}

#[test]
fn scenario_thumbnails_complete_raw_fixtures_are_never_damaged() {
    let raws = complete_raws();
    assert!(raws.len() >= 20);
    for (name, b) in &raws {
        let pr = probe(b);
        assert!(pr.format.is_some_and(|f| f.is_raw()), "{name}: a RAW");
        assert_eq!(truncation(b, &pr), None, "{name}");
        let (t, _) = make_thumbnail(b, &pr, &Decoders::none()).expect(name);
        let reason = t.placeholder.and_then(|p| p.reason);
        assert!(
            !matches!(reason, Some(PlaceholderReason::Unreadable { .. })),
            "{name}: {reason:?}"
        );
    }
}

/// The corpus writer's RAWs (the small preset's CR2s and DNGs): none damaged.
#[test]
fn corpus_raws_are_never_damaged() {
    let root = ps_fixtures::shared_corpus(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "cli-corpus-small-42",
        42,
        ps_fixtures::Preset::Small,
    );
    let mut n = 0;
    for e in walk(&root) {
        let b = std::fs::read(&e).unwrap();
        let pr = probe(&b);
        if !pr.format.is_some_and(|f| f.is_raw()) {
            continue;
        }
        n += 1;
        assert_eq!(truncation(&b, &pr), None, "{}", e.display());
    }
    assert!(n > 0, "the small corpus has RAWs");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// The D2 outcomes, one per test: a damaged in-file preview with no rawler image is `Failed`; rawler's unknown
/// camera, no preview and other failures map as the design says.
#[test]
fn raw_outcomes_map_in_order() {
    let jpeg = Some(DecodeError::Failed("bad Huffman code".into()));
    match raw_error(jpeg, RawlerOutcome::NoPreview) {
        DecodeError::Failed(m) => assert!(m.contains("bad Huffman code"), "{m}"),
        e => panic!("{e:?}"),
    }
    let jpeg = Some(DecodeError::Failed("x".into()));
    assert!(matches!(
        raw_error(
            jpeg,
            RawlerOutcome::UnknownCamera {
                make: "Canon".into(),
                model: "EOS R100".into()
            }
        ),
        DecodeError::Failed(_)
    ));
    match raw_error(
        None,
        RawlerOutcome::UnknownCamera {
            make: "Canon".into(),
            model: "EOS R100".into(),
        },
    ) {
        DecodeError::Unsupported(d) => assert_eq!(d, "RAW files from this camera (Canon EOS R100)"),
        e => panic!("{e:?}"),
    }
    match raw_error(
        None,
        RawlerOutcome::UnknownCamera {
            make: "Canon".into(),
            model: "Canon EOS R100".into(),
        },
    ) {
        DecodeError::Unsupported(d) => assert_eq!(d, "RAW files from this camera (Canon EOS R100)"),
        e => panic!("{e:?}"),
    }
    assert!(matches!(
        raw_error(None, RawlerOutcome::NoPreview),
        DecodeError::NoRawPreview
    ));
    match raw_error(None, RawlerOutcome::Failed("Missing tag".into())) {
        DecodeError::Unsupported(d) => assert_eq!(
            d,
            "RAW files whose preview this version cannot extract: Missing tag"
        ),
        e => panic!("{e:?}"),
    }
}

/// A damaged embedded preview inside a complete file (its entropy-coded data garbled) and no image from rawler:
/// `Failed`, as a damaged JPEG (ADR-0055).
#[test]
fn damaged_in_file_preview_is_failed() {
    let mut b = ps_fixtures::dng::cr2_unknown_lens(4);
    let ((p_off, p_len), _) = raw_ranges(&b).unwrap();
    // Keep the headers (SOI..SOS), overwrite the scan data with garbage.
    let sos = b[p_off..p_off + p_len]
        .windows(2)
        .position(|w| w == [0xFF, 0xDA])
        .unwrap()
        + p_off;
    for (i, x) in b[sos + 20..p_off + p_len - 2].iter_mut().enumerate() {
        *x = if i % 7 == 0 {
            0xFF
        } else {
            (i * 37 % 251) as u8
        };
    }
    let pr = probe(&b);
    assert!(pr.preview.is_some(), "the preview lies inside the file");
    assert_eq!(truncation(&b, &pr), None, "complete");
    let j = pr.preview.unwrap().range.slice(&b).unwrap();
    assert!(ps_media::decode::decode_jpeg(j, 512).is_err(), "garbled");
    // rawler's own JPEG decoder is lenient and still makes an image of it (D2: then that image is used).
    assert!(matches!(rawler_preview(&b), RawlerOutcome::Image(_)));
    assert!(decode_raw_preview(&b, &pr, 512).is_ok());
    // From a camera rawler does not know, rawler gives no image: the damaged preview decides.
    let model = b"Canon EOS 5D Mark III";
    let at = b.windows(model.len()).position(|w| w == model).unwrap();
    b[at..at + model.len()].copy_from_slice(b"Canon EOS 5D Mark IX\0");
    let pr = probe(&b);
    assert!(matches!(
        rawler_preview(&b),
        RawlerOutcome::UnknownCamera { .. }
    ));
    let e = decode_raw_preview(&b, &pr, 512).expect_err("damaged preview");
    match e {
        DecodeError::Failed(m) => assert!(m.contains("embedded preview could not be read"), "{m}"),
        e => panic!("{e:?}"),
    }
}

/// What rawler makes of each fixture (the outcomes it can give for synthetic files).
#[test]
fn rawler_outcomes_of_the_fixtures() {
    match rawler_preview(&ps_fixtures::dng::cr2_unknown_camera(3)) {
        RawlerOutcome::UnknownCamera { make, model } => {
            assert_eq!(make, "Canon");
            assert_eq!(model, ps_fixtures::dng::UNKNOWN_CAMERA_MODEL);
        }
        o => panic!("{o:?}"),
    }
    // Not a RAW at all: rawler fails with a message, which is not evidence of damage.
    match rawler_preview(b"not a raw file at all, just some bytes") {
        RawlerOutcome::Failed(m) => assert!(!m.is_empty()),
        RawlerOutcome::UnknownCamera { .. } => {}
        o => panic!("{o:?}"),
    }
}

/// Task 1.3, run by hand over a folder of real camera files (read-only): no complete RAW is judged cut short.
/// `PHOTOSQUIRREL_SAMPLE_DIR=/path cargo test -p ps-media --test damaged_raw -- --ignored --nocapture`.
#[test]
#[ignore = "needs PHOTOSQUIRREL_SAMPLE_DIR with real camera files"]
fn real_raws_are_never_damaged() {
    let Some(dir) = std::env::var_os("PHOTOSQUIRREL_SAMPLE_DIR").map(std::path::PathBuf::from)
    else {
        eprintln!("PHOTOSQUIRREL_SAMPLE_DIR not set; skipping");
        return;
    };
    let (mut files, mut raws, mut found) = (0, 0, Vec::new());
    for p in walk(&dir) {
        files += 1;
        let Ok(b) = ps_media::read_all(&p) else {
            continue;
        };
        let pr = probe(&b);
        if !pr.format.is_some_and(|f| f.is_raw()) {
            continue;
        }
        raws += 1;
        if let Some(t) = truncation(&b, &pr) {
            found.push(format!("{}: {}", p.display(), t.message()));
        }
    }
    eprintln!(
        "{files} files, {raws} RAW files, {} judged cut short",
        found.len()
    );
    assert!(found.is_empty(), "{found:#?}");
}
