use serde::{Deserialize, Serialize};

use crate::Format;

/// A capture time as the camera recorded it: local wall-clock time plus the UTC offset when known.
///
/// Cameras usually record local time without a zone; comparing two capture times is only meaningful as
/// wall-clock values, so `local_seconds` is "seconds since 1970-01-01T00:00:00 as if the wall clock were UTC".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CaptureTime {
    pub local_seconds: i64,
    /// Sub-second milliseconds (`SubSecTimeOriginal`), 0 when unknown.
    pub millis: u16,
    /// UTC offset in minutes (`OffsetTimeOriginal`), when recorded.
    pub offset_minutes: Option<i16>,
}

impl CaptureTime {
    /// Absolute difference in milliseconds between two wall-clock capture times.
    pub fn diff_millis(&self, other: &CaptureTime) -> i64 {
        let a = self.local_seconds * 1000 + i64::from(self.millis);
        let b = other.local_seconds * 1000 + i64::from(other.millis);
        (a - b).abs()
    }

    /// (year, month, day) of the wall-clock date, proleptic Gregorian.
    pub fn ymd(&self) -> (i32, u8, u8) {
        civil_from_days(self.local_seconds.div_euclid(86_400))
    }

    /// `YYYY-MM-DD HH:MM:SS`.
    pub fn display(&self) -> String {
        let (y, m, d) = self.ymd();
        let secs = self.local_seconds.rem_euclid(86_400);
        format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60
        )
    }

    /// Build from calendar parts (wall-clock).
    pub fn from_parts(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> Self {
        let days = days_from_civil(y, mo, d);
        Self {
            local_seconds: days * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s),
            millis: 0,
            offset_minutes: None,
        }
    }
}

/// Howard Hinnant's days-from-civil algorithm.
pub fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// GPS position in decimal degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpsPosition {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
}

/// EXIF orientation 1-8 (1 = upright). Values 5-8 swap width and height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Orientation(pub u8);

impl Orientation {
    pub const UPRIGHT: Orientation = Orientation(1);

    pub fn swaps_dimensions(self) -> bool {
        (5..=8).contains(&self.0)
    }
}

impl Default for Orientation {
    fn default() -> Self {
        Orientation::UPRIGHT
    }
}

/// Everything PhotoSquirrel extracts about a file's content. Fields are optional because sources vary wildly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// Stored pixel dimensions (before orientation).
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Orientation,
    pub capture_time: Option<CaptureTime>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub camera_serial: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length_mm: Option<f32>,
    pub focal_length_35mm: Option<f32>,
    pub aperture: Option<f32>,
    /// Exposure time in seconds.
    pub exposure_s: Option<f32>,
    pub iso: Option<u32>,
    pub flash_fired: Option<bool>,
    pub gps: Option<GpsPosition>,
    pub software: Option<String>,
    /// Estimated JPEG quality 1-100 from quantization tables.
    pub jpeg_quality: Option<u8>,
    /// Maker notes present (a sign of an untouched camera original), also when kept in a DNG's
    /// `DNGPrivateData` (ADR-0105).
    pub has_maker_notes: bool,
    /// The frame the file records (`OriginalRawFileName`, maker-note file number; ADR-0105).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<crate::FrameId>,
    /// Video duration in milliseconds.
    pub duration_ms: Option<u64>,
    pub video_codec: Option<String>,
    pub frame_rate: Option<f32>,
    /// Apple Live Photo / content identifier pairing a HEIC with its MOV.
    pub content_identifier: Option<String>,
    /// All tags as (group, name, value) strings, for the inspector.
    pub all_tags: Vec<(String, String, String)>,
}

impl MediaMetadata {
    /// Width and height after applying orientation.
    pub fn oriented_dimensions(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.width?, self.height?);
        Some(if self.orientation.swaps_dimensions() {
            (h, w)
        } else {
            (w, h)
        })
    }

    pub fn pixel_count(&self) -> u64 {
        match (self.width, self.height) {
            (Some(w), Some(h)) => u64::from(w) * u64::from(h),
            _ => 0,
        }
    }

    /// Camera EXIF present (make/model, capture time or exposure data).
    pub fn has_camera_exif(&self) -> bool {
        self.camera_make.is_some()
            || self.camera_model.is_some()
            || (self.capture_time.is_some() && (self.exposure_s.is_some() || self.iso.is_some()))
    }

    /// 0-4: how many of capture time, GPS, make/model, exposure are present (best-copy ranking, ADR-0014).
    pub fn completeness(&self) -> u8 {
        u8::from(self.capture_time.is_some())
            + u8::from(self.gps.is_some())
            + u8::from(self.camera_make.is_some() || self.camera_model.is_some())
            + u8::from(self.exposure_s.is_some() || self.aperture.is_some() || self.iso.is_some())
    }
}

/// The result of probing a file (ADR-0010): format and whatever the first bytes revealed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeInfo {
    pub format: Format,
    pub metadata: MediaMetadata,
    /// An embedded preview (EXIF thumbnail, RAW preview, HEIC thumbnail) and its size, if found in the probe
    /// window. Bytes are not kept here; the analyzer extracts them from the full read.
    pub embedded_preview: Option<(u32, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-1_000_000i64, -1, 0, 1, 10_957, 19_000, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn capture_time_display_and_diff() {
        let a = CaptureTime::from_parts(2014, 7, 4, 18, 30, 5);
        assert_eq!(a.display(), "2014-07-04 18:30:05");
        assert_eq!(a.ymd(), (2014, 7, 4));
        let b = CaptureTime {
            millis: 500,
            ..CaptureTime::from_parts(2014, 7, 4, 18, 30, 7)
        };
        assert_eq!(a.diff_millis(&b), 2500);
    }

    #[test]
    fn orientation_swaps() {
        let m = MediaMetadata {
            width: Some(4000),
            height: Some(3000),
            orientation: Orientation(6),
            ..Default::default()
        };
        assert_eq!(m.oriented_dimensions(), Some((3000, 4000)));
        assert_eq!(m.pixel_count(), 12_000_000);
    }
}
