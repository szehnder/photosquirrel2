//! media-metadata scenarios.

use std::path::Path;
use std::time::{Duration, SystemTime};

use ps_fixtures::media as fx;
use ps_media::{DateContext, FixedZone, LocalZone, RuleZone, extract_metadata, probe_bytes};
use ps_model::{
    CaptureDate, CaptureTime, DatePrecision, DateProvenance, Dimensions, Format, Metadata, RelPath,
    URational,
};

/// Analyse `bytes` as a file `name` at the source root, with instants read in UTC (`FixedZone(0)`): these
/// scenarios give their dates in UTC, so they hold in every machine's zone.
fn analyze(
    bytes: &[u8],
    name: &str,
    mtime: Option<SystemTime>,
) -> (ps_model::ProbeRecord, Metadata) {
    analyze_at(bytes, name, mtime, &FixedZone(0))
}

/// Analyse `bytes` as the file at `rel` (inside its source) with the local zone `zone`.
fn analyze_at(
    bytes: &[u8],
    rel: &str,
    mtime: Option<SystemTime>,
    zone: &dyn LocalZone,
) -> (ps_model::ProbeRecord, Metadata) {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let p = probe_bytes(bytes, bytes.len() as u64, Some(Path::new(name)));
    let rel_path = RelPath::from_bytes(rel.as_bytes().to_vec()).expect("rel path");
    let m = extract_metadata(
        bytes,
        &p,
        mtime,
        &DateContext {
            rel_path: Some(&rel_path),
            zone,
        },
    );
    (p, m)
}

/// A UTC instant as a `SystemTime`.
fn utc(y: i32, mo: u8, d: u8, h: u8, mi: u8) -> SystemTime {
    let s = CaptureTime::from_parts(y, mo, d, h, mi, 0).local_seconds;
    if s >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(s as u64)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(s.unsigned_abs())
    }
}

fn date(m: &Metadata) -> CaptureDate {
    m.capture_date.expect("a capture date")
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
    // The file time is an instant given in UTC: read it in UTC so the day holds in every zone (ADR-0070).
    let (_, m) = analyze_at(&j, "n.jpg", Some(mtime), &FixedZone(0));
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
    let (_, m) = analyze_at(
        &fx::jpeg_with_exif(64, 48, 1, 80, &spec),
        "d.jpg",
        None,
        &FixedZone(0),
    );
    let c = m.capture_date.expect("date");
    assert_eq!(c.provenance, DateProvenance::DateTimeOriginal);
    assert_eq!(c.precision, DatePrecision::Second);
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

// ---- capture-dates: folder-name dates (ADR-0069) and local days of instants (ADR-0070) ---------------------

/// An EXIF-less JPEG.
fn stripped() -> Vec<u8> {
    fx::jpeg(320, 240, 9, 85)
}

#[test]
fn scenario_media_metadata_folder_date_below_exif_and_above_file_time() {
    let mtime = utc(2024, 3, 1, 12, 0);
    let (_, plain) = analyze(&stripped(), "2019-07-04 Beach/IMG_0001.JPG", Some(mtime));
    let d = date(&plain);
    assert_eq!(d.value, CaptureTime::from_parts(2019, 7, 4, 0, 0, 0));
    assert_eq!(d.provenance, DateProvenance::FolderName);
    assert_eq!(d.provenance.as_str(), "folder-name");
    assert_eq!(d.precision, DatePrecision::Day);
    assert!(!d.undated);
    // Never an embedded date: grouping and ranking do not see it (ADR-0069).
    assert!(plain.fields.capture_time.is_none());
    assert!(!d.is_embedded());

    let spec = fx::ExifSpec {
        date_time_original: Some("2018:01:02 10:00:00".into()),
        ..Default::default()
    };
    let (_, exif) = analyze(
        &fx::jpeg_with_exif(64, 48, 1, 80, &spec),
        "2019-07-04 Beach/IMG_0002.JPG",
        Some(mtime),
    );
    let d = date(&exif);
    assert_eq!(d.provenance, DateProvenance::DateTimeOriginal);
    assert_eq!(d.value.display(), "2018-01-02 10:00:00");
    assert_eq!(d.precision, DatePrecision::Second);
}

#[test]
fn scenario_media_metadata_folder_date_patterns() {
    let mtime = utc(2024, 3, 1, 12, 0);
    let day = ["2019-07-04", "2019_07_04 Beach", "20190704", "2019/07/04"];
    for folder in day {
        let (_, m) = analyze(&stripped(), &format!("{folder}/a.jpg"), Some(mtime));
        let d = date(&m);
        assert_eq!(
            (d.value.ymd(), d.provenance, d.precision),
            ((2019, 7, 4), DateProvenance::FolderName, DatePrecision::Day),
            "{folder}"
        );
    }
    for folder in ["2019-07 Italy", "2019/07"] {
        let (_, m) = analyze(&stripped(), &format!("{folder}/a.jpg"), Some(mtime));
        let d = date(&m);
        assert_eq!(
            (d.value.ymd(), d.provenance, d.precision),
            (
                (2019, 7, 1),
                DateProvenance::FolderName,
                DatePrecision::Month
            ),
            "{folder}"
        );
        assert_eq!(d.display(), "July 2019");
    }
    for folder in ["2019", "Backup 2019-07-04", "DSC_19871234"] {
        let (_, m) = analyze(&stripped(), &format!("{folder}/a.jpg"), Some(mtime));
        let d = date(&m);
        assert_eq!(
            (d.value.ymd(), d.provenance, d.undated),
            ((2024, 3, 1), DateProvenance::FileMtime, true),
            "{folder}"
        );
    }
}

#[test]
fn scenario_media_metadata_school_year_folder_is_not_a_month() {
    let mtime = utc(2024, 3, 1, 12, 0);
    for folder in [
        "2011-12 School Year",
        "2008_09 Season",
        "2011-2012",
        "2011-2012 School Year",
        "1999.00 Winter",
    ] {
        let (_, m) = analyze(&stripped(), &format!("{folder}/a.jpg"), Some(mtime));
        let d = date(&m);
        assert_eq!(
            (d.value.ymd(), d.provenance, d.undated),
            ((2024, 3, 1), DateProvenance::FileMtime, true),
            "{folder}"
        );
    }
    for (folder, ymd) in [
        ("2011-11 Thanksgiving", (2011, 11, 1)),
        ("2011/12", (2011, 12, 1)),
    ] {
        let (_, m) = analyze(&stripped(), &format!("{folder}/a.jpg"), Some(mtime));
        let d = date(&m);
        assert_eq!(
            (d.value.ymd(), d.provenance, d.precision),
            (ymd, DateProvenance::FolderName, DatePrecision::Month),
            "{folder}"
        );
    }
    // A day after the range's month is still a day.
    let (_, m) = analyze(&stripped(), "2011-12-25 Christmas/a.jpg", Some(mtime));
    assert_eq!(date(&m).value.ymd(), (2011, 12, 25));
}

#[test]
fn scenario_media_metadata_nearest_folder_wins() {
    let (_, m) = analyze(
        &stripped(),
        "2018/2019-07-04 Beach/raw/IMG_0001.JPG",
        Some(utc(2024, 3, 1, 12, 0)),
    );
    let d = date(&m);
    assert_eq!(d.value.ymd(), (2019, 7, 4));
    assert_eq!(d.provenance, DateProvenance::FolderName);
    assert_eq!(d.precision, DatePrecision::Day);
}

#[test]
fn scenario_media_metadata_source_root_name_not_used() {
    // A source whose own folder is named like a date: only folders inside the source count.
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("2019-07-04");
    std::fs::create_dir_all(&root).expect("mkdir");
    let file = root.join("IMG_0001.JPG");
    std::fs::write(&file, stripped()).expect("write");
    let rel = RelPath::between(&root, &file).expect("inside the source");
    let bytes = std::fs::read(&file).expect("read");
    let p = probe_bytes(&bytes, bytes.len() as u64, Some(&file));
    let m = extract_metadata(
        &bytes,
        &p,
        Some(utc(2024, 3, 1, 12, 0)),
        &DateContext {
            rel_path: Some(&rel),
            zone: &FixedZone(0),
        },
    );
    let d = date(&m);
    assert_eq!(d.provenance, DateProvenance::FileMtime);
    assert_eq!(d.value.ymd(), (2024, 3, 1));
    assert!(d.undated);
}

#[test]
fn scenario_media_metadata_backup_folder_date_ignored() {
    let (_, m) = analyze(
        &stripped(),
        "2019-07-04/IMG_0001.JPG",
        Some(utc(2012, 3, 1, 12, 0)),
    );
    let d = date(&m);
    assert_eq!(d.value.ymd(), (2012, 3, 1));
    assert_eq!(d.provenance, DateProvenance::FileMtime);
    assert!(d.undated);
    // One day of slack: a file saved the evening before the folder's day still takes the folder date.
    let (_, m) = analyze(
        &stripped(),
        "2019-07-04/IMG_0002.JPG",
        Some(utc(2019, 7, 3, 1, 0)),
    );
    assert_eq!(date(&m).provenance, DateProvenance::FolderName);
    // A month folder compares against the start of its month minus one day.
    let (_, m) = analyze(
        &stripped(),
        "2019-07 Italy/IMG_0003.JPG",
        Some(utc(2019, 6, 29, 12, 0)),
    );
    assert_eq!(date(&m).provenance, DateProvenance::FileMtime);
}

#[test]
fn scenario_media_metadata_unset_camera_clock_does_not_block_folder_date() {
    let (_, m) = analyze(
        &stripped(),
        "2019-07-04/IMG_0001.JPG",
        Some(utc(1980, 1, 1, 0, 0)),
    );
    let d = date(&m);
    assert_eq!(d.value.ymd(), (2019, 7, 4));
    assert_eq!(d.provenance, DateProvenance::FolderName);
    assert!(!d.undated);
}

/// The resolver half; the CLI JSON half is in `photosquirrel-cli`'s tests.
#[test]
fn scenario_media_metadata_month_folder_stays_a_month() {
    let (_, m) = analyze(
        &stripped(),
        "2019-07 Italy/IMG_0001.JPG",
        Some(utc(2024, 3, 1, 12, 0)),
    );
    let d = date(&m);
    assert_eq!(d.precision, DatePrecision::Month);
    assert_eq!(d.value, CaptureTime::from_parts(2019, 7, 1, 0, 0, 0));
    assert_eq!(d.iso(), "2019-07");
    assert_eq!(d.display(), "July 2019");
    // Stored in the metadata JSON with its precision.
    let json = serde_json::to_value(d).expect("json");
    assert_eq!(json["precision"], "month");
}

#[test]
fn scenario_media_metadata_file_time_in_the_local_zone() {
    let (_, m) = analyze_at(
        &stripped(),
        "IMG_0001.JPG",
        Some(utc(2019, 7, 5, 3, 30)),
        &RuleZone::LOS_ANGELES,
    );
    let d = date(&m);
    assert_eq!(d.value.display(), "2019-07-04 20:30:00");
    assert_eq!(d.value.offset_minutes, Some(-420));
    assert_eq!(d.provenance, DateProvenance::FileMtime);
    assert!(d.undated);
}

#[test]
fn scenario_media_metadata_daylight_saving_of_the_file_s_date_applies() {
    // The rules at the file's instant apply (January: standard time), whatever today's offset is.
    let (_, m) = analyze_at(
        &stripped(),
        "IMG_0001.JPG",
        Some(utc(2019, 1, 15, 7, 30)),
        &RuleZone::LOS_ANGELES,
    );
    let d = date(&m);
    assert_eq!(d.value.display(), "2019-01-14 23:30:00");
    assert_eq!(d.value.offset_minutes, Some(-480));
}

#[test]
fn scenario_media_metadata_utc_video_creation_time_made_local() {
    let spec = fx::MovieSpec {
        creation_unix: Some(CaptureTime::from_parts(2019, 7, 5, 2, 0, 0).local_seconds),
        // Only the mvhd time: a UTC instant.
        keyed_offset_min: None,
        make: None,
        model: None,
        ..fx::MovieSpec::iphone()
    };
    let (p, m) = analyze_at(
        &fx::movie(&spec),
        "IMG_0001.MOV",
        Some(utc(2024, 3, 1, 12, 0)),
        &RuleZone::NEW_YORK,
    );
    assert_eq!(p.format, Some(Format::Mov));
    let d = date(&m);
    assert_eq!(d.provenance, DateProvenance::Container);
    assert_eq!(d.value.display(), "2019-07-04 22:00:00");
    assert_eq!(d.value.offset_minutes, Some(-240));
    assert!(!d.undated);
    // The embedded capture time grouping reads stays as recorded.
    assert_eq!(
        m.fields.capture_time.map(|t| t.display()),
        Some("2019-07-05 02:00:00".into())
    );
}

#[test]
fn scenario_media_metadata_keyed_quicktime_date_never_shifted() {
    // 2019-07-04 00:30 at +00:00 from the keyed creation date (and the same instant in mvhd).
    let spec = fx::MovieSpec {
        creation_unix: Some(CaptureTime::from_parts(2019, 7, 4, 0, 30, 0).local_seconds),
        keyed_offset_min: Some(0),
        ..fx::MovieSpec::iphone()
    };
    let (_, m) = analyze_at(
        &fx::movie(&spec),
        "IMG_0002.MOV",
        Some(utc(2024, 3, 1, 12, 0)),
        &RuleZone::LOS_ANGELES,
    );
    let d = date(&m);
    assert_eq!(d.provenance, DateProvenance::Container);
    assert_eq!(d.value.display(), "2019-07-04 00:30:00", "stays on its day");
    assert_eq!(d.value.offset_minutes, Some(0));
    // A keyed date with another offset keeps its wall clock too.
    let spec = fx::MovieSpec {
        keyed_offset_min: Some(-420),
        ..spec
    };
    let (_, m) = analyze_at(
        &fx::movie(&spec),
        "IMG_0003.MOV",
        Some(utc(2024, 3, 1, 12, 0)),
        &RuleZone::NEW_YORK,
    );
    let d = date(&m);
    assert_eq!(d.value.display(), "2019-07-03 17:30:00");
    assert_eq!(d.value.offset_minutes, Some(-420));
    // Re-dating from stored facts agrees (ADR-0071).
    assert_eq!(
        ps_media::resolve_capture_date(
            &ps_media::EmbeddedDates::from_stored(&m),
            None,
            None,
            &RuleZone::NEW_YORK
        ),
        m.capture_date
    );
}

#[test]
fn scenario_media_metadata_exif_wall_clock_never_shifted() {
    let plain = fx::ExifSpec {
        date_time_original: Some("2019:07:04 23:30:00".into()),
        ..Default::default()
    };
    let with_offset = fx::ExifSpec {
        offset_time_original: Some("+09:00".into()),
        ..plain.clone()
    };
    for (spec, offset) in [(plain, None), (with_offset, Some(540))] {
        let (_, m) = analyze_at(
            &fx::jpeg_with_exif(64, 48, 1, 80, &spec),
            "2020-01-01/IMG_0001.JPG",
            Some(utc(2024, 3, 1, 12, 0)),
            &RuleZone::BERLIN,
        );
        let d = date(&m);
        assert_eq!(d.provenance, DateProvenance::DateTimeOriginal);
        assert_eq!(d.value.display(), "2019-07-04 23:30:00");
        assert_eq!(d.value.offset_minutes, offset);
    }
}

/// An offset the file records without a date (EXIF `OffsetTime`) makes its file time local before the zone.
#[test]
fn recorded_offset_beats_the_zone_for_the_file_time() {
    let spec = fx::ExifSpec {
        offset_time_original: Some("+09:00".into()),
        ..Default::default()
    };
    let (_, m) = analyze_at(
        &fx::jpeg_with_exif(64, 48, 1, 80, &spec),
        "IMG_0001.JPG",
        Some(utc(2019, 7, 4, 20, 0)),
        &RuleZone::LOS_ANGELES,
    );
    let d = date(&m);
    assert_eq!(d.provenance, DateProvenance::FileMtime);
    assert_eq!(d.value.display(), "2019-07-05 05:00:00");
    assert_eq!(d.value.offset_minutes, Some(540));
}
