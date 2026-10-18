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
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub rotation: u16,
    pub frame_rate: Option<f32>,
    pub codec: Option<String>,
    pub gps: Option<GpsPosition>,
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
