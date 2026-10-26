//! photo-classification scenarios.

use std::path::Path;

use ps_fixtures::media as fx;
use ps_media::{FolderHints, classify, classify_with, probe_bytes};
use ps_model::{ClassReason, ClassifierSettings, MediaKind, PhotoClass, ProbeRecord};

fn probe(bytes: &[u8], path: &str) -> ProbeRecord {
    probe_bytes(bytes, bytes.len() as u64, Some(Path::new(path)))
}

fn class_of(bytes: &[u8], path: &str) -> (PhotoClass, Vec<ClassReason>) {
    classify(
        &probe(bytes, path),
        Path::new(path),
        &ClassifierSettings::default(),
    )
}

#[test]
fn scenario_photo_classification_class_and_reason_assigned() {
    let settings = ClassifierSettings::default();
    for f in fx::media_set(11) {
        let p = probe(&f.bytes, &f.path);
        let (_, reasons) = classify(&p, Path::new(&f.path), &settings);
        assert!(!reasons.is_empty(), "{} has no reason", f.path);
        let c = ps_media::classify::classification(
            &p,
            Path::new(&f.path),
            &settings,
            FolderHints::default(),
        );
        assert!(c.reasons.iter().all(|r| !r.is_empty()));
    }
}

#[test]
fn scenario_photo_classification_classification_is_deterministic() {
    let settings = ClassifierSettings::default();
    for f in fx::media_set(12) {
        let p = probe(&f.bytes, &f.path);
        let a = classify(&p, Path::new(&f.path), &settings);
        let b = classify(&p.clone(), Path::new(&f.path), &settings.clone());
        assert_eq!(a, b, "{}", f.path);
    }
}

#[test]
fn scenario_photo_classification_camera_exif_wins_over_size() {
    let spec = fx::ExifSpec {
        make: Some("NIKON".into()),
        date_time_original: Some("2009:08:01 12:00:00".into()),
        ..Default::default()
    };
    let (c, r) = class_of(&fx::jpeg_with_exif(150, 150, 1, 85, &spec), "a/tiny.jpg");
    assert_eq!(c, PhotoClass::Photo);
    assert_eq!(r[0].to_string(), "camera EXIF");
}

#[test]
fn scenario_photo_classification_tiny_image_is_junk() {
    let (c, r) = class_of(&fx::rgb_png(180, 500, 1), "a/t.png");
    assert_eq!(c, PhotoClass::Junk);
    assert!(
        r.iter()
            .any(|x| x.to_string().contains("200 px minimum side")),
        "{r:?}"
    );
}

#[test]
fn scenario_photo_classification_boundary_size_is_kept() {
    let (c, _) = class_of(&fx::jpeg(200, 600, 1, 85), "a/b.jpg");
    assert_eq!(c, PhotoClass::LikelyPhoto);
}

#[test]
fn scenario_photo_classification_custom_threshold_applied() {
    let j = fx::jpeg(180, 800, 1, 85);
    let p = probe(&j, "a/c.jpg");
    assert_eq!(
        classify(&p, Path::new("a/c.jpg"), &ClassifierSettings::default()).0,
        PhotoClass::Junk
    );
    let s = ClassifierSettings {
        min_side_px: 100,
        ..Default::default()
    };
    assert_eq!(
        classify(&p, Path::new("a/c.jpg"), &s).0,
        PhotoClass::LikelyPhoto
    );
}

#[test]
fn scenario_photo_classification_lightroom_previews_are_cache() {
    let path = "Lightroom/Catalog Previews.lrdata/0/A1/abc.jpg";
    let (c, r) = class_of(
        &fx::jpeg_with_exif(1024, 683, 1, 80, &fx::ExifSpec::canon_5d()),
        path,
    );
    assert_eq!(c, PhotoClass::ThumbnailCache);
    assert!(r[0].to_string().contains("Lightroom previews"), "{r:?}");
}

#[test]
fn other_cache_folders_are_cache() {
    let j = fx::jpeg(800, 600, 1, 80);
    for path in [
        "home/u/.thumbnails/large/x.jpg",
        "Users/u/AppData/Local/x.jpg",
        "proj/node_modules/pkg/x.jpg",
        "home/u/.cache/app/x.jpg",
        "Pictures/Photos Library.photoslibrary/resources/proxies/x.jpg",
    ] {
        assert_eq!(class_of(&j, path).0, PhotoClass::ThumbnailCache, "{path}");
    }
    let s = ClassifierSettings {
        extra_cache_patterns: vec!["CacheStuff".into()],
        ..Default::default()
    };
    let p = probe(&j, "a/cachestuff/x.jpg");
    assert_eq!(
        classify(&p, Path::new("a/cachestuff/x.jpg"), &s).0,
        PhotoClass::ThumbnailCache
    );
}

#[test]
fn scenario_photo_classification_thumbs_db_neighbour_is_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("Thumbs.db"), b"x").expect("write");
    let jp = dir.path().join("photo.jpg");
    let j = fx::jpeg(1200, 900, 1, 85);
    std::fs::write(&jp, &j).expect("write");
    let p = ps_media::probe_path(&jp);
    let hints = FolderHints::from_dir(dir.path());
    assert!(hints.has_thumbs_db);
    // The scanner classifies by the path relative to the source root (the temp dir's own location, e.g.
    // `AppData\Local\Temp` on Windows, is not part of it).
    let rel = Path::new("photo.jpg");
    let (c, _) = classify_with(&p, rel, &ClassifierSettings::default(), hints);
    assert_eq!(c, PhotoClass::ThumbnailCache);
    assert_eq!(
        classify(&p, rel, &ClassifierSettings::default()).0,
        PhotoClass::LikelyPhoto
    );
}

#[test]
fn scenario_photo_classification_palette_png_is_graphic() {
    let (c, _) = class_of(&fx::palette_png(1024, 768, 3), "a/p.png");
    assert_eq!(c, PhotoClass::Graphic);
}

#[test]
fn scenario_photo_classification_ico_is_graphic() {
    let (c, _) = class_of(&fx::ico(256), "a/i.ico");
    assert_eq!(c, PhotoClass::Graphic);
    assert_eq!(class_of(&fx::svg(), "a/s.svg").0, PhotoClass::Graphic);
}

#[test]
fn scenario_photo_classification_phone_video_is_photo() {
    let spec = fx::MovieSpec {
        duration_ms: 12_000,
        ..fx::MovieSpec::iphone()
    };
    let p = probe(&fx::movie(&spec), "v/IMG.MOV");
    let (c, r) = classify(&p, Path::new("v/IMG.MOV"), &ClassifierSettings::default());
    assert_eq!(c, PhotoClass::Photo, "{r:?}");
    assert_eq!(p.kind(), Some(MediaKind::Video));
}

#[test]
fn scenario_photo_classification_short_clip_is_junk() {
    let spec = fx::MovieSpec {
        brand: fx::MovieBrand::Mp4,
        width: 1280,
        height: 720,
        duration_ms: 1_500,
        make: None,
        model: None,
        creation_unix: None,
        ..fx::MovieSpec::iphone()
    };
    let (c, r) = class_of(&fx::movie(&spec), "v/clip.mp4");
    assert_eq!(c, PhotoClass::Junk);
    assert!(
        r.iter()
            .any(|x| x.to_string().contains("2 s minimum duration")),
        "{r:?}"
    );
}

#[test]
fn scenario_photo_classification_plain_large_jpeg_is_likely_photo() {
    let (c, r) = class_of(&fx::jpeg(3000, 2000, 1, 85), "Pictures/2010/x.jpg");
    assert_eq!(c, PhotoClass::LikelyPhoto);
    assert_eq!(r[0].to_string(), "no camera metadata");
}

#[test]
fn unsupported_and_broken_files_are_junk_with_reason() {
    let (c, r) = class_of(&fx::random_bytes(4096, 1), "a/r.jpg");
    assert_eq!(c, PhotoClass::Junk);
    assert!(matches!(r[0], ClassReason::Unsupported { .. }));
}

/// ADR-0104: a file whose content is not recognised is filed "could not be read" (shown by default), not "not a
/// supported file", when it has a photo or video extension (a damaged JPEG) or could not be read at all (the
/// probe's I/O error record).
#[test]
fn damaged_photos_and_unreadable_files_are_not_unsupported() {
    use ps_model::ExclusionCategory;
    let category = |probe: &ProbeRecord, name: &str| {
        let (class, reasons) = classify(probe, Path::new(name), &ClassifierSettings::default());
        let reasons: Vec<String> = reasons.iter().map(ToString::to_string).collect();
        ExclusionCategory::of(
            Some(class),
            &reasons,
            probe.format.is_some(),
            name.as_bytes(),
        )
    };
    let garbage = fx::random_bytes(4096, 1);
    assert_eq!(
        category(&probe(&garbage, "DCIM/IMG_9999.JPG"), "IMG_9999.JPG"),
        Some(ExclusionCategory::Unreadable)
    );
    assert_eq!(
        category(&probe(&garbage, "Downloads/data.bin"), "data.bin"),
        Some(ExclusionCategory::Unsupported)
    );
    // A read that fails (a disk error, a permission problem): the probe's I/O error record.
    struct Failing;
    impl std::io::Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("input/output error"))
        }
    }
    let io = ps_media::probe_reader(Failing, 4096, Some(Path::new("notes.txt")));
    assert!(io.error.is_some() && io.format.is_none());
    assert_eq!(
        category(&io, "notes.txt"),
        Some(ExclusionCategory::Unreadable)
    );
}
