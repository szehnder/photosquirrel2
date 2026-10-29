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

#[test]
fn scenario_thumbnails_srgb_thumbnails_are_unchanged() {
    let write = std::env::var_os("PHOTOSQUIRREL_WRITE_COLOUR_GOLDENS").is_some();
    for (file, jpeg) in thumbnails() {
        let path = std::path::Path::new(DIR).join(&file);
        if write {
            std::fs::create_dir_all(DIR).unwrap();
            std::fs::write(&path, &jpeg).unwrap();
            continue;
        }
        let golden = std::fs::read(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(
            golden == jpeg,
            "{file}: the thumbnail differs from the build before the change"
        );
    }
}
