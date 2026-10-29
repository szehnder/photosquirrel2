//! Colour spaces (adobe-rgb-color, ADR-0113): detection in the probe and the conversion of thumbnails and large
//! previews to sRGB, checked against a reference conversion computed here from the published matrices (not moxcms).

use ps_fixtures::colour as cfx;
use ps_media::decode::decode_jpeg;
use ps_media::thumb::{ThumbOrigin, analyze_still, make_large_preview, make_thumbnail};
use ps_model::{ColourSpace, Format, ProbeRecord, RawFormat};

fn probe(bytes: &[u8]) -> ProbeRecord {
    ps_media::probe_bytes(bytes, bytes.len() as u64, None)
}

fn decoders() -> ps_media::Decoders {
    ps_media::Decoders::none()
}

/// The centre pixel of a JPEG.
fn centre(jpeg: &[u8]) -> [u8; 3] {
    let (img, _) = decode_jpeg(jpeg, 1).expect("decode");
    img.pixel(img.width / 2, img.height / 2)
}

fn mul(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn srgb_decode(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

const XYZ_TO_SRGB: [[f64; 3]; 3] = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266_0, 1.876_010_8, 0.041_556_0],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];
const ADOBE_TO_XYZ: [[f64; 3]; 3] = [
    [0.576_730_9, 0.185_554_0, 0.188_185_2],
    [0.297_376_9, 0.627_349_1, 0.075_274_1],
    [0.027_034_3, 0.070_687_2, 0.991_108_5],
];
const P3_TO_XYZ: [[f64; 3]; 3] = [
    [0.486_570_9, 0.265_667_7, 0.198_217_3],
    [0.228_974_6, 0.691_738_5, 0.079_286_9],
    [0.0, 0.045_113_4, 1.043_944_4],
];

/// Reference conversion of an 8-bit value in Adobe RGB (1998) (gamma 563/256) or Display P3 (sRGB curve) to sRGB,
/// as floats in 0..=255.
fn reference(rgb: [u8; 3], space: &ColourSpace) -> [f64; 3] {
    let (lin, m): ([f64; 3], _) = match space {
        ColourSpace::AdobeRgb => (
            rgb.map(|c| (f64::from(c) / 255.0).powf(563.0 / 256.0)),
            ADOBE_TO_XYZ,
        ),
        ColourSpace::DisplayP3 => (rgb.map(|c| srgb_decode(f64::from(c) / 255.0)), P3_TO_XYZ),
        other => panic!("no reference for {other:?}"),
    };
    mul(XYZ_TO_SRGB, mul(m, lin)).map(|c| srgb_encode(c) * 255.0)
}

/// CIE L*a*b* (D65) of an sRGB value in 0..=255.
fn lab(rgb: [f64; 3]) -> [f64; 3] {
    const SRGB_TO_XYZ: [[f64; 3]; 3] = [
        [0.412_456_4, 0.357_576_1, 0.180_437_5],
        [0.212_672_9, 0.715_152_2, 0.072_175_0],
        [0.019_333_9, 0.119_192_0, 0.950_304_1],
    ];
    let xyz = mul(SRGB_TO_XYZ, rgb.map(|c| srgb_decode(c / 255.0)));
    let white = [0.950_47, 1.0, 1.088_83];
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let [fx, fy, fz] = [0, 1, 2].map(|i| f(xyz[i] / white[i]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE76 colour difference between two sRGB values.
fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (a, b) = (lab(a), lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn f(rgb: [u8; 3]) -> [f64; 3] {
    rgb.map(f64::from)
}

/// The patch value a decoder reads from the fixture's own pixels (before any conversion).
fn source_value(jpeg: &[u8]) -> [u8; 3] {
    centre(jpeg)
}

/// Asserts `got` is the reference conversion of `src` from `space` within ΔE 2, and that the unconverted value was
/// visibly off (the muted look the change fixes). Returns (ΔE before, ΔE after) for the record.
fn assert_converted(src: [u8; 3], got: [u8; 3], space: &ColourSpace) -> (f64, f64) {
    let want = reference(src, space);
    let after = delta_e(f(got), want);
    let before = delta_e(f(src), want);
    assert!(
        after <= 2.0,
        "{space:?}: {src:?} became {got:?}, the reference is {want:?} (ΔE {after:.2})"
    );
    assert!(
        before > 5.0,
        "{space:?}: the unconverted value should be visibly off (ΔE {before:.2})"
    );
    eprintln!(
        "{space:?}: source {src:?}, unconverted ΔE {before:.2}; converted {got:?} vs reference \
         [{:.1}, {:.1}, {:.1}], ΔE {after:.2}",
        want[0], want[1], want[2]
    );
    (before, after)
}

#[test]
fn scenario_thumbnails_canon_adobe_rgb_jpeg_thumbnail_is_converted() {
    let j = cfx::adobe_rgb_jpeg(cfx::GREEN);
    let p = probe(&j);
    assert_eq!(p.colour, ColourSpace::AdobeRgb);
    assert!(p.icc.is_empty(), "no profile");
    let src = source_value(&j);
    let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
    assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
    // The analysis path's thumbnail too, and the dominant colour follows the converted thumbnail.
    let (analysis, t, origin) = analyze_still(&j, &p, &decoders()).expect("analysis");
    assert_eq!(origin, ThumbOrigin::Analysis);
    assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
    let dominant = t.dominant_rgb;
    assert!(
        delta_e(f(dominant), reference(src, &ColourSpace::AdobeRgb)) <= 3.0,
        "dominant {dominant:?}"
    );
    // The analysis image itself is never converted.
    let a = analysis
        .image
        .pixel(analysis.image.width / 2, analysis.image.height / 2);
    assert!(a.iter().zip(src).all(|(x, y)| x.abs_diff(y) <= 1));
}

#[test]
fn scenario_thumbnails_raw_preview_in_adobe_rgb_is_converted() {
    let raw = cfx::canon_adobe_rgb_cr2(cfx::GREEN);
    let p = probe(&raw);
    assert_eq!(p.format, Some(Format::Raw(RawFormat::Cr2)));
    assert_eq!(p.colour, ColourSpace::AdobeRgb, "from the Canon maker note");
    let preview = p
        .preview
        .expect("preview")
        .range
        .slice(&raw)
        .unwrap()
        .to_vec();
    let src = source_value(&preview);
    let (t, origin) = make_thumbnail(&raw, &p, &decoders()).expect("thumbnail");
    assert_eq!(origin, ThumbOrigin::EmbeddedPreview);
    assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
    let (_, t, origin) = analyze_still(&raw, &p, &decoders()).expect("analysis");
    assert_eq!(origin, ThumbOrigin::EmbeddedPreview);
    assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
}

#[test]
fn scenario_thumbnails_embedded_profile_is_applied() {
    let j = cfx::icc_jpeg(cfx::GREEN, &cfx::display_p3_icc());
    let p = probe(&j);
    assert_eq!(p.colour, ColourSpace::DisplayP3);
    assert!(!p.icc.is_empty());
    let src = source_value(&j);
    let (preview, _, _) = make_large_preview(&j, &p, 2048, &decoders()).expect("large preview");
    assert_converted(src, centre(&preview), &ColourSpace::DisplayP3);
    let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
    assert_converted(src, centre(&t.jpeg), &ColourSpace::DisplayP3);
}

#[test]
fn large_preview_of_an_adobe_rgb_jpeg_is_converted() {
    let j = cfx::adobe_rgb_jpeg(cfx::GREEN);
    let p = probe(&j);
    let (preview, _, _) = make_large_preview(&j, &p, 2048, &decoders()).expect("large preview");
    assert_converted(source_value(&j), centre(&preview), &ColourSpace::AdobeRgb);
}

#[test]
fn a_profile_of_no_named_space_is_applied_and_named() {
    let j = cfx::icc_jpeg(cfx::GREEN, &cfx::pro_photo_icc("Studio Wide"));
    let p = probe(&j);
    assert_eq!(
        p.colour,
        ColourSpace::Profile {
            description: "Studio Wide".into()
        }
    );
    let src = source_value(&j);
    let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
    // ProPhoto green is far more saturated than sRGB can show: the conversion moves it a long way.
    let got = centre(&t.jpeg);
    assert!(delta_e(f(got), f(src)) > 10.0, "{src:?} -> {got:?}");
}

#[test]
fn dng_preview_colour_space_decides() {
    let adobe = cfx::dng_preview_colour(cfx::GREEN, 3);
    let p = probe(&adobe);
    assert_eq!(p.format, Some(Format::Raw(RawFormat::Dng)));
    assert_eq!(p.colour, ColourSpace::AdobeRgb);
    let preview = p
        .preview
        .expect("preview")
        .range
        .slice(&adobe)
        .unwrap()
        .to_vec();
    let (t, _) = make_thumbnail(&adobe, &p, &decoders()).expect("thumbnail");
    assert_converted(
        source_value(&preview),
        centre(&t.jpeg),
        &ColourSpace::AdobeRgb,
    );
    let srgb = cfx::dng_preview_colour(cfx::GREEN, 2);
    assert_eq!(probe(&srgb).colour, ColourSpace::Srgb);
}

#[test]
fn detection_by_source_and_order() {
    let g = cfx::GREEN;
    // "Uncalibrated" alone is no evidence.
    assert_eq!(
        probe(&cfx::exif_jpeg(g, 0xFFFF, None, None)).colour,
        ColourSpace::Srgb
    );
    // The Canon maker note answers when EXIF does not...
    assert_eq!(
        probe(&cfx::exif_jpeg(g, 0xFFFF, None, Some(2))).colour,
        ColourSpace::AdobeRgb
    );
    // ...and EXIF sRGB comes first.
    assert_eq!(
        probe(&cfx::exif_jpeg(g, 1, None, Some(2))).colour,
        ColourSpace::Srgb
    );
    // R03 comes before ColorSpace 1 (a camera writing both is in Adobe RGB).
    assert_eq!(
        probe(&cfx::exif_jpeg(g, 1, Some("R03"), None)).colour,
        ColourSpace::AdobeRgb
    );
    assert_eq!(
        probe(&cfx::exif_jpeg(g, 1, Some("R98"), None)).colour,
        ColourSpace::Srgb
    );
    // A Canon CR2 recording sRGB in its note.
    assert_eq!(probe(&cfx::canon_cr2(g, 1)).colour, ColourSpace::Srgb);
    // An sRGB profile is sRGB, without ranges to convert from.
    let p = probe(&cfx::icc_jpeg(g, &cfx::srgb_icc()));
    assert_eq!(p.colour, ColourSpace::Srgb);
    assert!(p.icc.is_empty());
    // An embedded Adobe RGB profile is named Adobe RGB, and the profile wins over EXIF.
    let both = cfx::with_icc(&cfx::exif_jpeg(g, 1, None, None), &cfx::adobe_rgb_icc());
    assert_eq!(probe(&both).colour, ColourSpace::AdobeRgb);
}

#[test]
fn malformed_profile_is_ignored_with_a_warning() {
    let mut icc = cfx::display_p3_icc();
    icc.truncate(60);
    let j = cfx::icc_jpeg(cfx::GREEN, &icc);
    let p = probe(&j);
    assert_eq!(p.colour, ColourSpace::Srgb);
    assert!(
        p.warnings.iter().any(|w| w.contains("ICC profile")),
        "{:?}",
        p.warnings
    );
    let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
    let src = source_value(&j);
    assert!(
        centre(&t.jpeg)
            .iter()
            .zip(src)
            .all(|(x, y)| x.abs_diff(y) <= 2)
    );
    // A chunk set with a missing chunk is ignored the same way.
    let mut segs = cfx::icc_segments(&cfx::display_p3_icc());
    segs[17] = 2; // "chunk 1 of 2", and no chunk 2
    let body = cfx::untagged_jpeg(cfx::GREEN);
    let mut j = body[..2].to_vec();
    j.extend_from_slice(&segs);
    j.extend_from_slice(&body[2..]);
    let p = probe(&j);
    assert_eq!(p.colour, ColourSpace::Srgb);
    assert!(!p.warnings.is_empty());
}

#[test]
fn colour_detection_reads_no_more_than_the_probe_window() {
    // The probe of a file read through a counting reader reads the same bytes with or without a profile.
    for bytes in [
        cfx::untagged_jpeg(cfx::GREEN),
        cfx::icc_jpeg(cfx::GREEN, &cfx::display_p3_icc()),
        cfx::adobe_rgb_jpeg(cfx::GREEN),
        cfx::canon_adobe_rgb_cr2(cfx::GREEN),
    ] {
        let counter = ps_media::CountingReader::new(std::io::Cursor::new(&bytes));
        let read = counter.counter();
        let p = ps_media::probe_reader(counter, bytes.len() as u64, None);
        let n = read.load(std::sync::atomic::Ordering::Relaxed);
        assert!(n <= ps_media::PROBE_LIMIT as u64);
        assert_eq!(p.bytes_examined, n);
    }
}
