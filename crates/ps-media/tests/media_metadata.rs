//! media-metadata scenarios.

use std::path::Path;
use std::time::{Duration, SystemTime};

use ps_fixtures::media as fx;
use ps_media::{extract_metadata, probe_bytes};
use ps_model::{CaptureTime, DateProvenance, Dimensions, Format, Metadata, URational};

fn analyze(
    bytes: &[u8],
    name: &str,
    mtime: Option<SystemTime>,
) -> (ps_model::ProbeRecord, Metadata) {
    let p = probe_bytes(bytes, bytes.len() as u64, Some(Path::new(name)));
    let m = extract_metadata(bytes, &p, mtime);
    (p, m)
}

#[test]
fn scenario_media_metadata_camera_exif_extracted() {
    let j = fx::jpeg_with_exif(640, 480, 1, 90, &fx::ExifSpec::canon_5d());
    let (_, m) = analyze(&j, "c.jpg", None);
    let f = &m.fields;
    assert_eq!(f.camera_make.as_deref(), Some("Canon"));
    assert_eq!(f.camera_model.as_deref(), Some("Canon EOS 5D"));
    assert_eq!(m.exposure, Some(URational { num: 1, den: 250 }));
    assert_eq!(f.aperture, Some(2.8));
    assert_eq!(f.iso, Some(400));
    assert_eq!(f.focal_length_mm, Some(50.0));
    assert_eq!(f.lens_model.as_deref(), Some("EF50mm f/1.4 USM"));
    assert_eq!(f.camera_serial.as_deref(), Some("0123456789"));
    assert_eq!(f.flash_fired, Some(false));
    assert_eq!(f.orientation.0, 1);
    assert!(f.software.is_some());
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
}

#[test]
fn sub_seconds_and_offset_are_kept() {
    let spec = fx::ExifSpec {
        sub_sec_original: Some("25".into()),
        offset_time_original: Some("+02:00".into()),
        ..fx::ExifSpec::canon_5d()
    };
    let (_, m) = analyze(&fx::jpeg_with_exif(64, 48, 1, 80, &spec), "s.jpg", None);
    let c = m.capture_date.expect("date").value;
    assert_eq!(c.millis, 250);
    assert_eq!(c.offset_minutes, Some(120));
    assert_eq!(c.display(), "2010-05-01 10:00:00");
}

#[test]
fn scenario_media_metadata_gps_converted_to_decimal_degrees() {
    let spec = fx::ExifSpec {
        gps: Some(fx::GpsSpec::eiffel()),
        ..Default::default()
    };
    let (_, m) = analyze(&fx::jpeg_with_exif(64, 48, 1, 80, &spec), "g.jpg", None);
    let g = m.fields.gps.expect("gps");
    assert!((g.latitude - 48.858222).abs() < 1e-6, "{}", g.latitude);
    assert!((g.longitude - 2.294500).abs() < 1e-6, "{}", g.longitude);
}

#[test]
fn scenario_media_metadata_phone_video_metadata() {
    let mov = fx::movie(&fx::MovieSpec::iphone());
    let (p, m) = analyze(&mov, "IMG_0001.MOV", None);
    assert_eq!(p.format, Some(Format::Mov));
    let f = &m.fields;
    assert_eq!((f.width, f.height), (Some(1920), Some(1080)));
    assert_eq!(f.camera_make.as_deref(), Some("Apple"));
    let d = f.duration_ms.expect("duration");
    assert!(d.abs_diff(12_500) <= 50, "{d}");
    let c = m.capture_date.expect("date");
    assert_eq!(c.provenance, DateProvenance::Container);
    assert_eq!(
        c.value.local_seconds,
        CaptureTime::from_parts(2019, 7, 4, 18, 30, 0).local_seconds
    );
    assert_eq!(c.value.offset_minutes, Some(0));
    assert!(!c.undated);
    assert_eq!(f.frame_rate.map(|r| r.round()), Some(30.0));
    assert_eq!(m.rotation_deg, Some(0));
}

#[test]
fn rotated_mp4_reports_rotation() {
    let spec = fx::MovieSpec {
        brand: fx::MovieBrand::Mp4,
        rotation: 90,
        ..fx::MovieSpec::iphone()
    };
    let (p, m) = analyze(&fx::movie(&spec), "v.mp4", None);
    assert_eq!(m.rotation_deg, Some(90));
    assert_eq!(p.display, Some(Dimensions::new(1080, 1920)));
    // No mdta keys in this MP4: the mvhd creation time is the container date.
    assert_eq!(
        m.capture_date.map(|c| c.provenance),
        Some(DateProvenance::Container)
    );
}

#[test]
fn scenario_media_metadata_fallback_to_file_time_flagged() {
    let j = fx::jpeg(320, 240, 2, 85);
    let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_330_560_000); // 2012-03-01T00:00:00Z
    let (_, m) = analyze(&j, "n.jpg", Some(mtime));
    let c = m.capture_date.expect("date");
    assert_eq!(c.value.ymd(), (2012, 3, 1));
    assert_eq!(c.provenance, DateProvenance::FileMtime);
    assert_eq!(c.provenance.as_str(), "file-mtime");
    assert!(c.undated);
    assert!(m.fields.capture_time.is_none());
}

#[test]
fn scenario_media_metadata_datetimeoriginal_preferred() {
    let spec = fx::ExifSpec {
        date_time_original: Some("2010:05:01 10:00:00".into()),
        create_date: Some("2015:01:01 00:00:00".into()),
        ..Default::default()
    };
    let (_, m) = analyze(&fx::jpeg_with_exif(64, 48, 1, 80, &spec), "d.jpg", None);
    let c = m.capture_date.expect("date");
    assert_eq!(c.provenance, DateProvenance::DateTimeOriginal);
    assert_eq!(c.value.display(), "2010-05-01 10:00:00");
    let only_create = fx::ExifSpec {
        create_date: Some("2015:01:01 00:00:00".into()),
        ..Default::default()
    };
    let (_, m) = analyze(
        &fx::jpeg_with_exif(64, 48, 1, 80, &only_create),
        "d.jpg",
        None,
    );
    assert_eq!(
        m.capture_date.map(|c| c.provenance),
        Some(DateProvenance::CreateDate)
    );
}

#[test]
fn scenario_media_metadata_quality_estimate_within_tolerance() {
    let (q60, q92) = fx::quality_pair(5);
    let (_, a) = analyze(&q60, "a.jpg", None);
    let (_, b) = analyze(&q92, "b.jpg", None);
    let qa = a.fields.jpeg_quality.expect("q60 estimate");
    let qb = b.fields.jpeg_quality.expect("q92 estimate");
    assert!(qa.abs_diff(60) <= 5, "{qa}");
    assert!(qb.abs_diff(92) <= 5, "{qb}");
    for q in [10u8, 30, 50, 75, 85, 95, 100] {
        let e = ps_media::quality::estimate(&fx::jpeg(64, 64, 1, q)).expect("estimate");
        assert!(e.abs_diff(q) <= 5, "q{q} -> {e}");
    }
}

#[test]
fn scenario_media_metadata_maker_notes_mark_camera_original() {
    let (orig, edited) = fx::camera_original_and_edited(4);
    let (_, a) = analyze(&orig, "o.jpg", None);
    let (_, b) = analyze(&edited, "e.jpg", None);
    assert!(a.fields.has_maker_notes);
    assert!(a.likely_camera_original);
    assert!(!b.fields.has_maker_notes);
    assert!(!b.likely_camera_original);
}

#[test]
fn scenario_media_metadata_all_tags_listed() {
    let (j, n) = fx::jpeg_with_40_tags(3);
    assert_eq!(n, 40);
    let (_, m) = analyze(&j, "t.jpg", None);
    let tags = &m.fields.all_tags;
    assert_eq!(tags.len(), 40, "{tags:#?}");
    assert!(
        tags.iter().any(|(_, name, _)| name == "0xC4A5"),
        "{tags:#?}"
    );
    assert!(
        tags.iter()
            .any(|(g, name, v)| g == "IFD0" && name == "Make" && v == "Canon")
    );
    // Stable order: two extractions agree.
    let (_, m2) = analyze(&j, "t.jpg", None);
    assert_eq!(m2.fields.all_tags, *tags);
}

#[test]
fn scenario_media_metadata_corrupt_exif_keeps_file() {
    let j = fx::jpeg_with_corrupt_exif(7);
    let (p, m) = analyze(&j, "c.jpg", None);
    assert_eq!(p.format, Some(Format::Jpeg));
    assert_eq!(p.stored, Some(Dimensions::new(640, 480)));
    assert_eq!(m.fields.width, Some(640));
    assert!(m.fields.camera_make.is_none() && m.fields.camera_model.is_none());
    assert!(m.exposure.is_none() && m.fields.gps.is_none() && m.fields.capture_time.is_none());
    assert!(m.fields.all_tags.is_empty());
    assert_eq!(m.warnings.len(), 1, "{:?}", m.warnings);
}

#[test]
fn heic_metadata_read() {
    let h = include_bytes!("data/tiny.heic");
    let (_, m) = analyze(h, "x.heic", None);
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    assert_eq!(m.fields.camera_make.as_deref(), Some("Apple"));
    assert_eq!(
        m.capture_date.map(|c| c.provenance),
        Some(DateProvenance::DateTimeOriginal)
    );
}

#[test]
fn dng_metadata_read() {
    let d = fx::dng_with_preview(1024, 683, 96, 64, 2);
    let (_, m) = analyze(&d, "x.dng", None);
    assert_eq!(m.fields.camera_make.as_deref(), Some("FixtureCam"));
    assert_eq!(m.exposure, Some(URational { num: 1, den: 125 }));
}

/// An Apple maker note (`Apple iOS` header, big-endian IFD) holding tag 0x0011 ContentIdentifier.
fn apple_maker_note(content_identifier: &str) -> Vec<u8> {
    let mut v = content_identifier.as_bytes().to_vec();
    v.push(0);
    let mut note = b"Apple iOS\0\x00\x01MM".to_vec();
    note.extend_from_slice(&2u16.to_be_bytes());
    // 0x0001 MakerNoteVersion (SLONG 14), then 0x0011 ContentIdentifier (ASCII, stored after the IFD).
    note.extend_from_slice(&[0x00, 0x01, 0x00, 0x09, 0, 0, 0, 1, 0, 0, 0, 14]);
    let data_off = (16 + 2 * 12 + 4) as u32;
    note.extend_from_slice(&0x0011u16.to_be_bytes());
    note.extend_from_slice(&2u16.to_be_bytes());
    note.extend_from_slice(&(v.len() as u32).to_be_bytes());
    note.extend_from_slice(&data_off.to_be_bytes());
    note.extend_from_slice(&0u32.to_be_bytes());
    note.extend_from_slice(&v);
    note
}

/// Export task 1.2: a Live Photo pair (an iPhone JPEG still and its MOV) carries one Apple ContentIdentifier
/// in the still's maker note and in the movie's `com.apple.quicktime.content.identifier` key.
#[test]
fn live_photo_still_and_video_share_the_apple_content_identifier() {
    let id = "5D2E7C1B-9A3F-4E60-8B1D-2C4F6A8E0B13";
    let still = fx::jpeg_with_exif(
        320,
        240,
        7,
        85,
        &fx::ExifSpec {
            make: Some("Apple".into()),
            model: Some("iPhone 12".into()),
            date_time_original: Some("2021:07:04 18:30:00".into()),
            maker_note: Some(apple_maker_note(id)),
            ..Default::default()
        },
    );
    let (_, m) = analyze(&still, "IMG_0001.JPG", None);
    assert_eq!(m.fields.content_identifier.as_deref(), Some(id));
    assert!(
        m.fields
            .all_tags
            .iter()
            .any(|(g, n, v)| g == "Apple" && n == "ContentIdentifier" && v == id)
    );
    let movie = ps_fixtures::video::write_video(&ps_fixtures::video::VideoSpec {
        quicktime: true,
        width: 1920,
        height: 1440,
        duration_ms: 2_800,
        created_utc: 1_625_423_400,
        make: Some("Apple".into()),
        model: Some("iPhone 12".into()),
        location: None,
        content_identifier: Some(id.into()),
        payload: 4_096,
        seed: 7,
    });
    let (p, m) = analyze(&movie, "IMG_0001.MOV", None);
    assert_eq!(p.format, Some(Format::Mov));
    assert_eq!(m.fields.content_identifier.as_deref(), Some(id));
    assert_eq!(m.fields.camera_model.as_deref(), Some("iPhone 12"));

    // Other cameras and plain movies carry none.
    let (_, canon) = analyze(
        &fx::jpeg_with_exif(64, 48, 1, 80, &fx::ExifSpec::canon_5d()),
        "c.jpg",
        None,
    );
    assert_eq!(canon.fields.content_identifier, None);
    let plain = ps_fixtures::video::write_video(&ps_fixtures::video::VideoSpec {
        quicktime: true,
        width: 640,
        height: 480,
        duration_ms: 1_000,
        created_utc: 1_625_423_400,
        make: None,
        model: None,
        location: None,
        content_identifier: None,
        payload: 1_024,
        seed: 1,
    });
    assert_eq!(
        analyze(&plain, "v.mov", None).1.fields.content_identifier,
        None
    );
}

/// A still whose ContentIdentifier is only in XMP (converters that drop the maker note keep it there).
#[test]
fn content_identifier_from_xmp() {
    let id = "0F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9";
    let xmp = format!(
        "http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description \
         xmlns:apple-fi=\"http://ns.apple.com/faceinfo/1.0/\" apple-fi:ContentIdentifier=\"{id}\"/></rdf:RDF>\
         </x:xmpmeta>"
    );
    let j = fx::insert_segment(&fx::jpeg(64, 48, 3, 80), 0xE1, xmp.as_bytes());
    let (_, m) = analyze(&j, "x.jpg", None);
    assert_eq!(m.fields.content_identifier.as_deref(), Some(id));
}
