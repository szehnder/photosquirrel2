//! QuickTime/MP4 metadata: nom-exif track info (ADR-0017) plus rotation, frame rate and codec read from the
//! `moov` boxes directly.

use nom_exif::{EntryValue, MediaParser, MediaSource, TrackInfoTag};
use ps_model::{CaptureTime, GpsPosition};

use crate::bmff::{self, BoxRef};
use crate::probe::mac_time;
use crate::util::contain;

/// Everything we read from a movie's `moov`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoInfo {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub creation: Option<CaptureTime>,
    /// `creation` is the keyed `com.apple.quicktime.creationdate` (a wall clock with the offset the device
    /// recorded, even +00:00), not the `mvhd` time (a UTC instant; ADR-0081).
    pub creation_keyed: bool,
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub rotation: u16,
    pub frame_rate: Option<f32>,
    pub codec: Option<String>,
    pub gps: Option<GpsPosition>,
    /// Apple ContentIdentifier (`com.apple.quicktime.content.identifier`): pairs a Live Photo video with its
    /// still.
    pub content_identifier: Option<String>,
    /// nom-exif could not parse the container (fields then come from our own box walk only).
    pub track_error: Option<String>,
}

/// Read `moov` (which must lie inside `buf`).
pub fn movie_info(buf: &[u8], moov: &BoxRef) -> VideoInfo {
    let boxes = bmff::parse_movie(buf, moov);
    let mut v = VideoInfo {
        width: boxes.width.filter(|w| *w > 0),
        height: boxes.height.filter(|h| *h > 0),
        duration_ms: boxes.duration_ms,
        creation: mac_time(boxes.creation_mac),
        rotation: boxes.rotation,
        frame_rate: boxes.frame_rate,
        codec: boxes.codec,
        content_identifier: crate::apple::quicktime_content_identifier(buf, moov),
        ..Default::default()
    };

    // nom-exif needs the header bytes to recognise the container; give it ftyp + moov only.
    let mut mini = Vec::with_capacity(64 + moov.declared_len as usize);
    let top = bmff::top_level(buf);
    match bmff::find(&top, b"ftyp") {
        Some(f) if f.complete(buf) => {
            mini.extend_from_slice(&buf[f.start..f.end() as usize]);
        }
        _ => mini.extend_from_slice(b"\x00\x00\x00\x14ftypqt  \x00\x00\x02\x00qt  "),
    }
    mini.extend_from_slice(&buf[moov.start..moov.end() as usize]);

    let parsed = contain(|| -> Result<nom_exif::TrackInfo, String> {
        let ms = MediaSource::from_memory(mini).map_err(|e| e.to_string())?;
        MediaParser::new()
            .parse_track(ms)
            .map_err(|e| e.to_string())
    });
    match parsed {
        Some(Ok(info)) => {
            let text = |t| {
                info.get(t)
                    .and_then(EntryValue::as_str)
                    .map(|s| s.trim().to_owned())
            };
            v.make = text(TrackInfoTag::Make).filter(|s| !s.is_empty());
            v.model = text(TrackInfoTag::Model).filter(|s| !s.is_empty());
            v.software = text(TrackInfoTag::Software).filter(|s| !s.is_empty());
            if let Some(d) = info
                .get(TrackInfoTag::DurationMs)
                .and_then(EntryValue::as_u64)
                && d > 0
            {
                v.duration_ms = Some(d);
            }
            if v.width.is_none() {
                v.width = info.get(TrackInfoTag::Width).and_then(EntryValue::as_u32);
                v.height = info.get(TrackInfoTag::Height).and_then(EntryValue::as_u32);
            }
            if let Some(EntryValue::DateTime(dt)) = info.get(TrackInfoTag::CreateDate) {
                let offset = dt.offset().local_minus_utc() / 60;
                let local = dt.naive_local().and_utc();
                // mvhd creation 0 (1904-01-01) means "unset".
                if dt.timestamp() > -MAC_EPOCH_UNIX {
                    v.creation = Some(CaptureTime {
                        local_seconds: local.timestamp(),
                        millis: 0,
                        offset_minutes: i16::try_from(offset).ok(),
                    });
                    // nom-exif prefers the keyed date and falls back to `mvhd`: the date is keyed when the key
                    // holds this very wall clock.
                    v.creation_keyed = crate::apple::quicktime_creation_date(buf, moov)
                        .and_then(|k| crate::date::parse_date_text(&k))
                        .is_some_and(|k| k.local_seconds == local.timestamp());
                }
            }
            if let Some(g) = info.gps_info()
                && let (Some(lat), Some(lon)) = (g.latitude_decimal(), g.longitude_decimal())
            {
                {
                    v.gps = Some(GpsPosition {
                        latitude: lat,
                        longitude: lon,
                        altitude_m: g.altitude_meters(),
                    });
                }
            }
        }
        Some(Err(e)) => v.track_error = Some(e),
        None => v.track_error = Some("track parser failure".into()),
    }
    v
}

const MAC_EPOCH_UNIX: i64 = crate::util::MAC_EPOCH_OFFSET;

/// Largest `moov` box [`duration_of_file`] reads (16 MB): longer clips' sample tables stay well below.
pub const MOOV_READ_MAX: u64 = 16 * 1024 * 1024;

/// A movie's duration from its `moov` box, wherever it lies in the file: the top-level boxes are walked by their
/// headers (seeks, no data read), so a `moov` after the media data (cameras without fast-start) is found too.
/// The file is opened read-only (ADR-0004). `None` when there is no readable `moov` or no duration.
pub fn duration_of_file(path: &std::path::Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = crate::fs::open_read_only(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut head = [0u8; 16];
        let n = f.read(&mut head).ok()?;
        if n < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let size = match size32 {
            0 => len - pos,
            1 if n >= 16 => u64::from_be_bytes(head[8..16].try_into().ok()?),
            1 => return None,
            s => s,
        };
        if size < 8 {
            return None;
        }
        if &head[4..8] == b"moov" {
            if size > MOOV_READ_MAX || pos + size > len {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            f.seek(SeekFrom::Start(pos)).ok()?;
            f.read_exact(&mut buf).ok()?;
            let moov = bmff::moov_box(&buf)?;
            return movie_info(&buf, &moov).duration_ms;
        }
        pos = pos.checked_add(size)?;
    }
    None
}
