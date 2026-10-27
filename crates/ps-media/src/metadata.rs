//! Full metadata extraction (media-metadata spec): EXIF via nom-exif, QuickTime/MP4 via [`crate::video`],
//! RAW fallback via rawler, capture-date resolution, JPEG quality and the camera-original signal.

use std::io::Cursor;
use std::time::SystemTime;

use nom_exif::{EntryValue, ExifIter, IfdKind, MediaParser, MediaSource, TagOrCode};
use ps_model::{
    CaptureTime, Format, GpsPosition, MediaKind, MediaMetadata, Metadata, ProbeRecord, URational,
};

use crate::date::{DateContext, EmbeddedDates, resolve_capture_date};
use crate::util::{contain, parse_exif_datetime, parse_offset, parse_subsec};

/// Software names that mark an image as edited (case-insensitive substring match).
const EDITORS: &[&str] = &[
    "photoshop",
    "lightroom",
    "gimp",
    "paint.net",
    "picasa",
    "acdsee",
    "affinity",
    "pixelmator",
    "snapseed",
    "luminar",
    "capture one",
    "darktable",
    "rawtherapee",
    "photoscape",
    "irfanview",
    "xnview",
    "imagemagick",
    "graphicsmagick",
    "photo editor",
    "fotor",
    "befunky",
    "picsart",
    "vsco",
    "facetune",
    "skitch",
    "preview",
    "photos ",
    "digikam",
    "shotwell",
    "faststone",
    "corel",
    "paintshop",
    "dxo",
    "on1",
    "polarr",
];

/// The software tag names an image editor.
pub fn is_editing_software(software: &str) -> bool {
    let s = software.to_ascii_lowercase();
    EDITORS.iter().any(|e| s.contains(e))
}

/// Extract all metadata from the full file `bytes`. `probe` is the probe record for the same file;
/// `mtime` is the file modification time (the last capture-date fallback); `dates` gives the file's path
/// inside its source (folder-name dates) and the zone that makes instants local ([`crate::date`]). Never
/// fails: problems become warnings and the fields stay empty.
pub fn extract_metadata(
    bytes: &[u8],
    probe: &ProbeRecord,
    mtime: Option<SystemTime>,
    dates: &DateContext<'_>,
) -> Metadata {
    let mut m = Metadata::default();
    m.fields.width = probe.stored.map(|d| d.width);
    m.fields.height = probe.stored.map(|d| d.height);
    m.fields.orientation = probe.orientation;
    let mut exif_dates = ExifDates::default();
    let mut container_date = None;
    let mut container_keyed = false;

    match probe.format {
        Some(f) if f.kind() == MediaKind::Video => {
            if let Some(moov) = crate::bmff::moov_box(bytes).filter(|b| b.complete(bytes)) {
                let v = crate::video::movie_info(bytes, &moov);
                m.fields.width = v.width.or(m.fields.width);
                m.fields.height = v.height.or(m.fields.height);
                m.fields.duration_ms = v.duration_ms;
                m.fields.frame_rate = v.frame_rate;
                m.fields.video_codec = v.codec.clone();
                m.fields.camera_make = v.make.clone();
                m.fields.camera_model = v.model.clone();
                m.fields.software = v.software.clone();
                m.fields.gps = v.gps;
                m.fields.content_identifier = v.content_identifier.clone();
                m.rotation_deg = Some(v.rotation);
                container_date = v.creation;
                container_keyed = v.creation_keyed;
                push_tag(
                    &mut m.fields,
                    "QuickTime",
                    "Duration",
                    v.duration_ms.map(|d| format!("{d} ms")),
                );
                push_tag(&mut m.fields, "QuickTime", "Make", v.make);
                push_tag(&mut m.fields, "QuickTime", "Model", v.model);
                push_tag(&mut m.fields, "QuickTime", "Software", v.software);
                push_tag(
                    &mut m.fields,
                    "QuickTime",
                    "CreationDate",
                    v.creation.map(|c| c.display()),
                );
                // Where the creation date came from, so re-dating from stored facts treats it alike (ADR-0081).
                push_tag(
                    &mut m.fields,
                    "QuickTime",
                    crate::date::CREATION_DATE_SOURCE_TAG,
                    v.creation
                        .map(|_| if v.creation_keyed { "keyed" } else { "mvhd" }.to_owned()),
                );
                push_tag(
                    &mut m.fields,
                    "QuickTime",
                    "Rotation",
                    Some(v.rotation.to_string()),
                );
                push_tag(
                    &mut m.fields,
                    "QuickTime",
                    "FrameRate",
                    v.frame_rate.map(|f| format!("{f:.3}")),
                );
                push_tag(&mut m.fields, "QuickTime", "Codec", v.codec);
                push_tag(
                    &mut m.fields,
                    "QuickTime",
                    "ContentIdentifier",
                    v.content_identifier,
                );
                if let Some(e) = v.track_error {
                    m.warnings
                        .push(format!("video metadata partly unreadable: {e}"));
                }
            } else {
                m.warnings
                    .push("video metadata unreadable: no complete moov atom".into());
            }
        }
        Some(f) => {
            let mut res = contain(|| read_exif(bytes)).unwrap_or(Err("EXIF parser failure".into()));
            // The probe located the TIFF block itself (e.g. HEIF Exif items nom-exif misses): parse it alone.
            if res.is_err()
                && let Some(block) = probe.exif_range.and_then(|r| r.slice(bytes))
                && block.starts_with(b"II") | block.starts_with(b"MM")
                && let Some(Ok(it)) = contain(|| read_exif(block))
            {
                res = Ok(it);
            }
            match res {
                Ok(iter) => {
                    let n = apply_exif(iter, &mut m, &mut exif_dates);
                    if n == 0 && probe.exif_range.is_some() {
                        m.warnings
                            .push("EXIF block present but no tag could be decoded".into());
                    }
                }
                Err(e)
                    if probe.exif_range.is_some() || (f.is_raw() && !e.contains("not found")) =>
                {
                    // Try rawler for RAW files nom-exif does not understand.
                    if f.is_raw() && raw_fallback(bytes, &mut m, &mut exif_dates) {
                    } else {
                        m.warnings.push(format!("EXIF block unreadable: {e}"));
                    }
                }
                Err(_) if f.is_raw() => {
                    raw_fallback(bytes, &mut m, &mut exif_dates);
                }
                Err(_) => {}
            }
            if f == Format::Jpeg {
                m.fields.jpeg_quality = crate::quality::estimate(bytes);
            }
            // Frame identity and maker notes kept in DNGPrivateData (ADR-0105), from the same bytes.
            let frame = crate::frame::read_frame_facts(bytes, probe);
            m.fields.has_maker_notes |= frame.maker_notes;
            m.fields.set_frame_id(frame.frame_id);
            // Live Photo still: the maker note's ContentIdentifier (set by apply_exif), else XMP's.
            if m.fields.content_identifier.is_none()
                && let Some(id) = crate::apple::xmp_content_identifier(bytes)
            {
                push_tag(&mut m.fields, "XMP", "ContentIdentifier", Some(id.clone()));
                m.fields.content_identifier = Some(id);
            }
        }
        None => {}
    }

    // Capture date resolution (crate::date): DateTimeOriginal, CreateDate, container, folder name, file mtime.
    let embedded = EmbeddedDates {
        original: exif_dates.original,
        create: exif_dates.create,
        container: container_date,
        container_keyed,
        recorded_offset: crate::date::recorded_offset(&m.fields.all_tags),
    };
    m.fields.capture_time = embedded.capture_time();
    m.capture_date = resolve_capture_date(
        &embedded,
        dates.rel_path,
        mtime.and_then(unix_secs),
        dates.zone,
    );

    let f = &m.fields;
    let full_camera =
        f.camera_make.is_some() && f.camera_model.is_some() && exif_dates.original.is_some();
    let edited = f.software.as_deref().is_some_and(is_editing_software);
    m.likely_camera_original =
        probe.kind() == Some(MediaKind::Image) && (f.has_maker_notes || full_camera) && !edited;
    m
}

/// Seconds since the epoch, rounded down (so a file time before 1970 lands on the second it falls in, like the
/// catalog's `mtime_ns`).
pub fn unix_secs(t: SystemTime) -> Option<i64> {
    let ns: i128 = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i128::try_from(d.as_nanos()).ok()?,
        Err(e) => -i128::try_from(e.duration().as_nanos()).ok()?,
    };
    i64::try_from(ns.div_euclid(1_000_000_000)).ok()
}

fn push_tag(m: &mut MediaMetadata, group: &str, name: &str, value: Option<String>) {
    if let Some(v) = value {
        m.all_tags.push((group.to_owned(), name.to_owned(), v));
    }
}

fn read_exif(bytes: &[u8]) -> Result<ExifIter, String> {
    let ms = MediaSource::seekable(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    MediaParser::new().parse_exif(ms).map_err(|e| e.to_string())
}

#[derive(Default)]
struct ExifDates {
    original: Option<CaptureTime>,
    create: Option<CaptureTime>,
}

fn entry_time(v: &EntryValue) -> Option<CaptureTime> {
    match v {
        EntryValue::DateTime(dt) => Some(CaptureTime {
            local_seconds: dt.naive_local().and_utc().timestamp(),
            millis: 0,
            offset_minutes: i16::try_from(dt.offset().local_minus_utc() / 60).ok(),
        }),
        EntryValue::NaiveDateTime(n) => Some(CaptureTime {
            local_seconds: n.and_utc().timestamp(),
            millis: 0,
            offset_minutes: None,
        }),
        EntryValue::Text(s) => parse_exif_datetime(s),
        _ => None,
    }
}

fn group_name(ifd: usize, kind: IfdKind) -> String {
    match kind {
        IfdKind::Exif => "Exif".into(),
        IfdKind::Gps => "GPS".into(),
        IfdKind::Interop => "Interop".into(),
        _ => format!("IFD{ifd}"),
    }
}

fn display_value(v: &EntryValue) -> String {
    match v {
        EntryValue::Undefined(b) if b.len() > 64 => format!("[{} bytes]", b.len()),
        EntryValue::U8Array(b) if b.len() > 64 => format!("[{} bytes]", b.len()),
        EntryValue::URational(r) => format!("{}/{}", r.numerator(), r.denominator()),
        EntryValue::IRational(r) => format!("{}/{}", r.numerator(), r.denominator()),
        other => {
            let s = other.to_string();
            if s.len() > 256 {
                format!("{}…", &s[..s.floor_char_boundary(256)])
            } else {
                s
            }
        }
    }
}

fn urational(v: &EntryValue) -> Option<URational> {
    let r = match v {
        EntryValue::URational(r) => *r,
        EntryValue::URationalArray(a) => *a.first()?,
        _ => return None,
    };
    Some(URational {
        num: r.numerator(),
        den: r.denominator(),
    })
}

fn as_f32(v: &EntryValue) -> Option<f32> {
    urational(v)
        .and_then(URational::to_f64)
        .or_else(|| v.try_as_float())
        .map(|f| f as f32)
}

/// Apply nom-exif entries in file order; returns the number of tags listed.
fn apply_exif(mut iter: ExifIter, m: &mut Metadata, dates: &mut ExifDates) -> usize {
    let gps = iter.parse_gps().ok().flatten();
    let mut errors = 0usize;
    let mut subsec = None;
    let mut offset = None;
    let mut count = 0;
    for entry in iter.by_ref() {
        let kind = entry.ifd_kind();
        let ifd = entry.ifd().as_usize();
        let code = entry.tag().code();
        let Some(v) = entry.value() else {
            errors += 1;
            continue;
        };
        // Structural pointers are not tags a user cares about.
        if kind == IfdKind::Tiff && matches!(code, 0x8769 | 0x8825 | 0xA005 | 0x014A) {
            continue;
        }
        if kind == IfdKind::Exif && code == 0xA005 {
            continue;
        }
        let name = match entry.tag() {
            TagOrCode::Tag(t) => t.to_string(),
            TagOrCode::Unknown(c) => format!("0x{c:04X}"),
        };
        m.fields
            .all_tags
            .push((group_name(ifd, kind), name, display_value(v)));
        count += 1;
        let f = &mut m.fields;
        let text = || {
            v.as_str()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        match (kind, ifd, code) {
            (IfdKind::Tiff, 0, 0x010F) => f.camera_make = text(),
            (IfdKind::Tiff, 0, 0x0110) => f.camera_model = text(),
            (IfdKind::Tiff, 0, 0x0131) => f.software = text(),
            (IfdKind::Exif, _, 0x829A) => {
                m.exposure = urational(v);
                f.exposure_s = as_f32(v);
            }
            (IfdKind::Exif, _, 0x829D) => f.aperture = as_f32(v),
            (IfdKind::Exif, _, 0x8827) => {
                f.iso = v
                    .try_as_integer()
                    .and_then(|i| u32::try_from(i).ok())
                    .or_else(|| {
                        v.as_u16_slice()
                            .and_then(|s| s.first())
                            .map(|x| u32::from(*x))
                    });
            }
            (IfdKind::Exif, _, 0x920A) => f.focal_length_mm = as_f32(v),
            (IfdKind::Exif, _, 0xA405) => f.focal_length_35mm = as_f32(v),
            (IfdKind::Exif, _, 0x9209) => {
                f.flash_fired = v.try_as_integer().map(|x| x & 1 == 1);
            }
            (IfdKind::Exif, _, 0xA434) => f.lens_model = text(),
            (IfdKind::Exif, _, 0xA431) => f.camera_serial = text(),
            (IfdKind::Exif, _, 0x927C) => {
                f.has_maker_notes = v.as_undefined().is_none_or(|b| !b.is_empty());
                if let Some(id) = v
                    .as_undefined()
                    .and_then(crate::apple::maker_note_content_identifier)
                {
                    f.all_tags
                        .push(("Apple".into(), "ContentIdentifier".into(), id.clone()));
                    f.content_identifier = Some(id);
                }
            }
            (IfdKind::Exif, _, 0x9003) => dates.original = entry_time(v),
            (IfdKind::Exif, _, 0x9004) => dates.create = entry_time(v),
            (IfdKind::Exif, _, 0x9291) => subsec = v.as_str().and_then(parse_subsec),
            (IfdKind::Exif, _, 0x9011) => offset = v.as_str().and_then(parse_offset),
            _ => {}
        }
    }
    if let Some(o) = dates.original.as_mut() {
        o.millis = subsec.unwrap_or(0);
        if o.offset_minutes.is_none() {
            o.offset_minutes = offset;
        }
    }
    if let Some(g) = gps
        && let (Some(lat), Some(lon)) = (g.latitude_decimal(), g.longitude_decimal())
    {
        m.fields.gps = Some(GpsPosition {
            latitude: lat,
            longitude: lon,
            altitude_m: g.altitude_meters(),
        });
    }
    if errors > 0 {
        m.warnings
            .push(format!("{errors} EXIF tag(s) could not be decoded"));
    }
    count
}

/// RAW metadata through rawler when nom-exif cannot read the container. Returns true on success.
fn raw_fallback(bytes: &[u8], m: &mut Metadata, dates: &mut ExifDates) -> bool {
    let res = contain(|| -> Option<rawler::decoders::RawMetadata> {
        let src = rawler::rawsource::RawSource::new_from_slice(bytes);
        let dec = rawler::get_decoder(&src).ok()?;
        dec.raw_metadata(&src, &rawler::decoders::RawDecodeParams::default())
            .ok()
    })
    .flatten();
    let Some(raw) = res else { return false };
    let f = &mut m.fields;
    f.camera_make = Some(raw.make.clone()).filter(|s| !s.is_empty());
    f.camera_model = Some(raw.model.clone()).filter(|s| !s.is_empty());
    let e = &raw.exif;
    let rat = |r: &rawler::formats::tiff::Rational| URational { num: r.n, den: r.d };
    m.exposure = e.exposure_time.as_ref().map(rat);
    f.exposure_s = m.exposure.and_then(URational::to_f64).map(|x| x as f32);
    f.aperture = e
        .fnumber
        .as_ref()
        .and_then(|r| rat(r).to_f64())
        .map(|x| x as f32);
    f.iso = e.iso_speed.or(e.iso_speed_ratings.map(u32::from));
    f.focal_length_mm = e
        .focal_length
        .as_ref()
        .and_then(|r| rat(r).to_f64())
        .map(|x| x as f32);
    f.lens_model = e.lens_model.clone();
    f.camera_serial = e.serial_number.clone();
    dates.original = e
        .date_time_original
        .as_deref()
        .and_then(parse_exif_datetime);
    dates.create = e.create_date.as_deref().and_then(parse_exif_datetime);
    if let Some(t) = crate::tiff::Tiff::parse(bytes) {
        // nom-exif cannot read this container; list the TIFF/EXIF tags with our own walker. The RAW's own
        // IFD0 Make/Model win over rawler's normalized names ("EOS 70D" for "Canon EOS 70D") so that the RAW
        // and its JPEG twin agree, and SubSecTimeOriginal/OffsetTimeOriginal apply as they do for JPEGs.
        let (ifds, _) = t.walk();
        let mut subsec = None;
        let mut offset = None;
        for ifd in &ifds {
            let text = |en: &crate::tiff::Entry| {
                t.ascii(en)
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
            };
            for en in &ifd.entries {
                match (ifd.kind, en.tag) {
                    (crate::tiff::IfdKind::Chain(0), 0x010F) => {
                        if let Some(v) = text(en) {
                            f.camera_make = Some(v);
                        }
                    }
                    (crate::tiff::IfdKind::Chain(0), 0x0110) => {
                        if let Some(v) = text(en) {
                            f.camera_model = Some(v);
                        }
                    }
                    (crate::tiff::IfdKind::Exif, 0x9003) if dates.original.is_none() => {
                        dates.original = text(en).as_deref().and_then(parse_exif_datetime);
                    }
                    (crate::tiff::IfdKind::Exif, 0x9291) => {
                        subsec = text(en).as_deref().and_then(parse_subsec);
                    }
                    (crate::tiff::IfdKind::Exif, 0x9011) => {
                        offset = text(en).as_deref().and_then(parse_offset);
                    }
                    (crate::tiff::IfdKind::Exif, 0xA431) if f.camera_serial.is_none() => {
                        f.camera_serial = text(en);
                    }
                    _ => {}
                }
            }
        }
        if let Some(o) = dates.original.as_mut() {
            o.millis = subsec.unwrap_or(0);
            if o.offset_minutes.is_none() {
                o.offset_minutes = offset;
            }
        }
        for ifd in &ifds {
            let group = match ifd.kind {
                crate::tiff::IfdKind::Chain(i) => format!("IFD{i}"),
                crate::tiff::IfdKind::Exif => "Exif".into(),
                crate::tiff::IfdKind::Gps => "GPS".into(),
                crate::tiff::IfdKind::Interop => "Interop".into(),
                crate::tiff::IfdKind::Sub => "SubIFD".into(),
            };
            for en in &ifd.entries {
                if matches!(en.tag, 0x8769 | 0x8825 | 0xA005 | 0x014A) {
                    continue;
                }
                if en.tag == 0x927C && ifd.kind == crate::tiff::IfdKind::Exif {
                    f.has_maker_notes = en.count > 0;
                }
                let value = t
                    .ascii(en)
                    .filter(|_| en.typ == 2)
                    .or_else(|| {
                        let r = t.rationals(en);
                        (!r.is_empty()).then(|| {
                            r.iter()
                                .map(|(n, d)| format!("{n}/{d}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                    })
                    .or_else(|| {
                        let u = t.uints(en);
                        (!u.is_empty() && en.count <= 16)
                            .then(|| u.iter().map(u32::to_string).collect::<Vec<_>>().join(" "))
                    })
                    .unwrap_or_else(|| format!("[{} values]", en.count));
                f.all_tags
                    .push((group.clone(), format!("0x{:04X}", en.tag), value));
            }
        }
    } else {
        push_tag(f, "RAW", "Make", f.camera_make.clone());
        push_tag(f, "RAW", "Model", f.camera_model.clone());
        push_tag(f, "RAW", "DateTimeOriginal", e.date_time_original.clone());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps_fixtures::media::{ExifSpec, exif_tiff};

    /// A Canon CR2 that nom-exif cannot read: the RAW path must take Make/Model from the RAW's own IFD0
    /// (rawler normalizes "Canon EOS 70D" to "EOS 70D") and apply SubSecTimeOriginal, like the JPEG twin.
    #[test]
    fn raw_fallback_reads_make_model_and_subsec_from_raw_ifds() {
        let spec = ExifSpec {
            make: Some("Canon".into()),
            model: Some("Canon EOS 70D".into()),
            date_time_original: Some("2000:01:01 00:01:11".into()),
            sub_sec_original: Some("99".into()),
            body_serial: Some("021021000941".into()),
            ..Default::default()
        };
        let bytes = exif_tiff(&spec);
        let mut m = Metadata::default();
        let mut dates = ExifDates::default();
        assert!(raw_fallback(&bytes, &mut m, &mut dates));
        assert_eq!(m.fields.camera_make.as_deref(), Some("Canon"));
        assert_eq!(m.fields.camera_model.as_deref(), Some("Canon EOS 70D"));
        assert_eq!(m.fields.camera_serial.as_deref(), Some("021021000941"));
        let t = dates.original.expect("DateTimeOriginal");
        assert_eq!(t.millis, 990);
    }
}
