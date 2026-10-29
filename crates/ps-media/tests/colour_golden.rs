//! sRGB thumbnails are byte-identical to those of the build before `adobe-rgb-color` (ADR-0113): the goldens under
//! `tests/data/colour` were written by that build (`PHOTOSQUIRREL_WRITE_COLOUR_GOLDENS=1`).

use ps_fixtures::colour as cfx;
use ps_fixtures::media as fx;
use ps_media::thumb::{analyze_still, make_thumbnail};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/colour");

/// (name, file): an untagged camera-like JPEG and the same with an embedded sRGB profile, and the green patch both
/// ways.
fn cases() -> Vec<(&'static str, Vec<u8>)> {
    let photo = fx::jpeg(1000, 750, 3, 85);
    vec![
        ("untagged", photo.clone()),
        ("srgb-profile", cfx::with_icc(&photo, &cfx::srgb_icc())),
        ("untagged-patch", cfx::untagged_jpeg(cfx::GREEN)),
        (
            "srgb-profile-patch",
            cfx::icc_jpeg(cfx::GREEN, &cfx::srgb_icc()),
        ),
    ]
}

/// (golden file name, thumbnail JPEG) for every case, from the thumbnail path and the analysis path.
fn thumbnails() -> Vec<(String, Vec<u8>)> {
    let decoders = ps_media::Decoders::none();
    let mut out = Vec::new();
    for (name, bytes) in cases() {
        let probe = ps_media::probe_bytes(&bytes, bytes.len() as u64, None);
        let (t, _) = make_thumbnail(&bytes, &probe, &decoders).expect("thumbnail");
        out.push((format!("{name}.thumb.jpg"), t.jpeg));
        let (_, t, _) = analyze_still(&bytes, &probe, &decoders).expect("analysis");
        out.push((format!("{name}.analysis.jpg"), t.jpeg));
    }
    out
}

/// Whether this target is the one the goldens were written on (x86_64 Linux). Elsewhere SIMD paths of the decoder
/// and the resizer may differ by a code value from it, so the goldens are compared within 2 there; the comparison
/// with the conversion off below is exact on every target.
const GOLDEN_TARGET: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

#[test]
fn scenario_thumbnails_srgb_thumbnails_are_unchanged() {
    let write = std::env::var_os("PHOTOSQUIRREL_WRITE_COLOUR_GOLDENS").is_some();
    let now = thumbnails();
    // This binary is the only user of the process-wide hook: the build before the change, on this target.
    ps_model::set_colour_conversion_enabled(false);
    let before = thumbnails();
    ps_model::set_colour_conversion_enabled(true);
    assert_eq!(now, before, "sRGB thumbnails are not converted");
    for (file, jpeg) in now {
        let path = std::path::Path::new(DIR).join(&file);
        if write {
            std::fs::create_dir_all(DIR).unwrap();
            std::fs::write(&path, &jpeg).unwrap();
            continue;
        }
        let golden = std::fs::read(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        if GOLDEN_TARGET {
            assert!(
                golden == jpeg,
                "{file}: the thumbnail differs from the build before the change"
            );
        } else {
            let (a, _) = ps_media::decode::decode_jpeg(&golden, 1).unwrap();
            let (b, _) = ps_media::decode::decode_jpeg(&jpeg, 1).unwrap();
            assert_eq!((a.width, a.height), (b.width, b.height), "{file}");
            let worst = a
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            assert!(worst <= 2, "{file}: differs from the golden by {worst}");
        }
    }
}
