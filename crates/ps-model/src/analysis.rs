//! Media-analysis types (media-analysis change): probe records, capture-date provenance, classifier
//! settings and reasons, and thumbnail records. Additive to [`crate::Format`], [`crate::MediaMetadata`] and
//! friends.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CaptureTime, Format, MediaKind, MediaMetadata, Orientation};

/// The design documents call the content-detected format `MediaFormat`; it is [`Format`].
pub type MediaFormat = Format;

/// Pixel dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

impl Dimensions {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn long_edge(self) -> u32 {
        self.width.max(self.height)
    }

    pub fn short_edge(self) -> u32 {
        self.width.min(self.height)
    }

    pub fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Dimensions after applying an EXIF orientation (5-8 swap width and height).
    pub fn oriented(self, o: Orientation) -> Self {
        if o.swaps_dimensions() {
            Self::new(self.height, self.width)
        } else {
            self
        }
    }

    /// Scale so the long edge is at most `max_long_edge` (never upscales); both sides stay >= 1.
    pub fn fit_long_edge(self, max_long_edge: u32) -> Self {
        let long = self.long_edge();
        if long <= max_long_edge || long == 0 {
            return self;
        }
        let scale = |v: u32| -> u32 {
            let s =
                (u64::from(v) * u64::from(max_long_edge) + u64::from(long) / 2) / u64::from(long);
            (s as u32).max(1)
        };
        if self.width >= self.height {
            Self::new(max_long_edge, scale(self.height))
        } else {
            Self::new(scale(self.width), max_long_edge)
        }
    }
}

impl fmt::Display for Dimensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// A byte range inside a file (absolute offset).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u64,
}

impl ByteRange {
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }

    /// The range as a slice of `bytes` when it lies entirely inside.
    pub fn slice<'a>(&self, bytes: &'a [u8]) -> Option<&'a [u8]> {
        let start = usize::try_from(self.offset).ok()?;
        let end = usize::try_from(self.end()).ok()?;
        bytes.get(start..end)
    }
}

/// Where an embedded preview image comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    /// The EXIF IFD1 thumbnail of a JPEG/TIFF/HEIC.
    ExifThumbnail,
    /// A JPEG preview stored by a camera inside a RAW file.
    RawPreview,
    /// A HEIF `thmb` item (HEVC coded; needs the HEIC decoder).
    HeicThumbnail,
}

/// An embedded preview the probe located (ADR-0010, ADR-0016).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedPreview {
    pub source: PreviewSource,
    /// Absolute byte range of the preview's coded data.
    pub range: ByteRange,
    /// Pixel dimensions when known (from the preview's own header or the referencing tags).
    pub dims: Option<Dimensions>,
    /// False when this build cannot decode the preview (HEIC thumbnail without the `heic` feature).
    pub decodable: bool,
}

/// Camera EXIF signals found in the probe window; what the classifier needs (ADR-0015).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraSignals {
    pub make: Option<String>,
    pub model: Option<String>,
    pub date_time_original: Option<CaptureTime>,
    /// ExposureTime, FNumber or ISO present.
    pub has_exposure: bool,
    pub has_maker_notes: bool,
    pub software: Option<String>,
}

impl CameraSignals {
    /// Camera EXIF in the ADR-0015 sense: make, model, DateTimeOriginal or exposure data.
    pub fn is_camera_exif(&self) -> bool {
        self.make.is_some()
            || self.model.is_some()
            || self.date_time_original.is_some()
            || self.has_exposure
    }
}

/// QuickTime/MP4 facts available at probe time (when `moov` lies inside the probe window).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoSummary {
    pub duration_ms: Option<u64>,
    pub creation: Option<CaptureTime>,
    pub make: Option<String>,
    pub model: Option<String>,
}

/// Why a probe failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    /// The file ends before its structure does.
    Truncated,
    /// The structure is invalid.
    Corrupt,
    /// The file could not be read.
    Io,
}

/// A probe error record: the probe never fails the caller, it reports (ADR-0010).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeError {
    pub kind: ProbeErrorKind,
    pub message: String,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let k = match self.kind {
            ProbeErrorKind::Truncated => "truncated",
            ProbeErrorKind::Corrupt => "corrupt",
            ProbeErrorKind::Io => "io",
        };
        write!(f, "{k}: {}", self.message)
    }
}

/// Everything the bounded probe (first 256 KB) learnt about one file (ADR-0010).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeRecord {
    /// Content-detected format; `None` means unsupported (see `unsupported_reason`).
    pub format: Option<Format>,
    pub unsupported_reason: Option<String>,
    /// Size of the whole file in bytes.
    pub file_size: u64,
    /// The file name's extension does not match the detected format.
    pub extension_mismatch: bool,
    /// Stored pixel dimensions (before orientation).
    pub stored: Option<Dimensions>,
    /// Display dimensions (after orientation).
    pub display: Option<Dimensions>,
    pub orientation: Orientation,
    /// Absolute byte range of the EXIF (TIFF) block, when found.
    pub exif_range: Option<ByteRange>,
    pub camera: CameraSignals,
    /// Indexed-colour (palette) image.
    pub has_palette: bool,
    /// Alpha channel or transparency.
    pub has_alpha: bool,
    pub preview: Option<EmbeddedPreview>,
    pub video: Option<VideoSummary>,
    pub error: Option<ProbeError>,
    /// Non-fatal observations ("moov atom not in the probe window", ...).
    pub warnings: Vec<String>,
    /// Bytes of the file the probe examined (<= 262,144).
    pub bytes_examined: u64,
}

impl ProbeRecord {
    /// An empty record for a file of `file_size` bytes.
    pub fn empty(file_size: u64) -> Self {
        Self {
            format: None,
            unsupported_reason: None,
            file_size,
            extension_mismatch: false,
            stored: None,
            display: None,
            orientation: Orientation::UPRIGHT,
            exif_range: None,
            camera: CameraSignals::default(),
            has_palette: false,
            has_alpha: false,
            preview: None,
            video: None,
            error: None,
            warnings: Vec::new(),
            bytes_examined: 0,
        }
    }

    pub fn kind(&self) -> Option<MediaKind> {
        self.format.map(Format::kind)
    }

    pub fn is_supported(&self) -> bool {
        self.format.is_some()
    }
}

/// Where a capture date came from, in resolution order (media-metadata spec).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateProvenance {
    DateTimeOriginal,
    CreateDate,
    /// QuickTime/MP4 (or HEIF) container creation date.
    Container,
    FileMtime,
    /// A date in the name of a folder holding the file (ADR-0069). Not embedded, not undated.
    FolderName,
}

impl DateProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            DateProvenance::DateTimeOriginal => "DateTimeOriginal",
            DateProvenance::CreateDate => "CreateDate",
            DateProvenance::Container => "container",
            DateProvenance::FileMtime => "file-mtime",
            DateProvenance::FolderName => "folder-name",
        }
    }

    /// Inverse of [`DateProvenance::as_str`] (the catalog's `capture_provenance` column).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "DateTimeOriginal" => DateProvenance::DateTimeOriginal,
            "CreateDate" => DateProvenance::CreateDate,
            "container" => DateProvenance::Container,
            "file-mtime" => DateProvenance::FileMtime,
            "folder-name" => DateProvenance::FolderName,
            _ => return None,
        })
    }

    /// DateTimeOriginal, CreateDate or a container date: recorded in the file itself.
    pub fn is_embedded(self) -> bool {
        matches!(
            self,
            DateProvenance::DateTimeOriginal
                | DateProvenance::CreateDate
                | DateProvenance::Container
        )
    }

    /// For people: "DateTimeOriginal", "CreateDate", "container", "file modification time", "folder name".
    pub fn label(self) -> &'static str {
        match self {
            DateProvenance::FileMtime => "file modification time",
            DateProvenance::FolderName => "folder name",
            other => other.as_str(),
        }
    }
}

/// How much of a capture date is known (ADR-0069): a month folder says nothing about the day.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DatePrecision {
    /// Date and time (embedded dates, file times).
    #[default]
    Second,
    /// A calendar day (`2019-07-04` folders); the time is 00:00.
    Day,
    /// A month (`2019-07 Italy`); stored as the 1st at 00:00, never shown or filed as a day.
    Month,
}

impl DatePrecision {
    pub fn as_str(self) -> &'static str {
        match self {
            DatePrecision::Second => "second",
            DatePrecision::Day => "day",
            DatePrecision::Month => "month",
        }
    }

    /// The catalog's `capture_precision` code (migration 17: 0 second, 1 day, 2 month).
    pub fn code(self) -> u8 {
        match self {
            DatePrecision::Second => 0,
            DatePrecision::Day => 1,
            DatePrecision::Month => 2,
        }
    }

    /// `t` to this precision: `YYYY-MM`, `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS` (machine output).
    pub fn iso(self, t: &CaptureTime) -> String {
        let (y, m, d) = t.ymd();
        match self {
            DatePrecision::Month => format!("{y:04}-{m:02}"),
            DatePrecision::Day => format!("{y:04}-{m:02}-{d:02}"),
            DatePrecision::Second => t.display(),
        }
    }

    pub fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            0 => DatePrecision::Second,
            1 => DatePrecision::Day,
            2 => DatePrecision::Month,
            _ => return None,
        })
    }
}

/// The version of the capture-date rules (ADR-0071): 1 before folder-name dates and local days of instants
/// (ADR-0069, ADR-0070), 2 with them. Rows dated by an older version are re-dated from stored facts on the next
/// rescan of their source.
pub const DATE_LOGIC_VERSION: u8 = 2;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The one capture date of a file, with provenance and precision. `undated` is set when it is only the file
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CaptureDate {
    pub value: CaptureTime,
    pub provenance: DateProvenance,
    pub undated: bool,
    /// Absent in metadata written before ADR-0069: those dates are all to the second.
    #[serde(default)]
    pub precision: DatePrecision,
}

impl CaptureDate {
    /// Recorded in the file (DateTimeOriginal, CreateDate, container): what grouping and ranking may use.
    pub fn is_embedded(&self) -> bool {
        self.provenance.is_embedded()
    }

    /// "July 2019" (month), "4 July 2019" (day) or `YYYY-MM-DD HH:MM:SS` (second).
    pub fn display(&self) -> String {
        let (y, m, d) = self.value.ymd();
        let month = MONTHS[usize::from(m.clamp(1, 12) - 1)];
        match self.precision {
            DatePrecision::Month => format!("{month} {y}"),
            DatePrecision::Day => format!("{d} {month} {y}"),
            DatePrecision::Second => self.value.display(),
        }
    }

    /// `YYYY-MM` (month), `YYYY-MM-DD` (day) or `YYYY-MM-DD HH:MM:SS` (second): machine output.
    pub fn iso(&self) -> String {
        self.precision.iso(&self.value)
    }
}

/// An unsigned EXIF rational (exposure 1/250 stays exact).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct URational {
    pub num: u32,
    pub den: u32,
}

impl URational {
    pub fn to_f64(self) -> Option<f64> {
        (self.den != 0).then(|| f64::from(self.num) / f64::from(self.den))
    }
}

impl fmt::Display for URational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

/// Full metadata of one file (media-metadata spec): the shared [`MediaMetadata`] fields plus the
/// analysis-only extras.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub fields: MediaMetadata,
    /// Exposure time as recorded (e.g. 1/250).
    pub exposure: Option<URational>,
    /// Resolved capture date with provenance; `None` only when not even a file time is known.
    pub capture_date: Option<CaptureDate>,
    /// Maker notes or full camera EXIF and no editing-software signature.
    pub likely_camera_original: bool,
    /// Video rotation in degrees clockwise (0, 90, 180, 270).
    pub rotation_deg: Option<u16>,
    /// Non-fatal metadata problems (a corrupt EXIF block, ...); the file is still catalogued.
    pub warnings: Vec<String>,
}

/// Classifier thresholds (ADR-0015 defaults), from settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassifierSettings {
    /// Still images without camera EXIF whose shorter side is below this are junk.
    pub min_side_px: u32,
    /// Still images without camera EXIF with fewer pixels than this are junk.
    pub min_pixels: u64,
    /// Videos shorter than this are junk.
    pub min_video_duration_ms: u64,
    /// Videos whose shorter side is below this are junk.
    pub min_video_side_px: u32,
    /// Extra path fragments (matched case-insensitively against path components) treated as cache folders.
    pub extra_cache_patterns: Vec<String>,
}

impl Default for ClassifierSettings {
    fn default() -> Self {
        Self {
            min_side_px: 200,
            min_pixels: 120_000,
            min_video_duration_ms: 2_000,
            min_video_side_px: 320,
            extra_cache_patterns: Vec::new(),
        }
    }
}

/// The probe facts the classifier reads (ADR-0015), kept per file in the catalog so a classifier settings
/// change re-classifies an unchanged file without reading it again (ADR-0051). Short serde names: one row per
/// file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassFacts {
    /// Content-detected format; `None`: unsupported.
    #[serde(rename = "f", skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,
    /// Why the format is unsupported (when it is).
    #[serde(rename = "u", skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
    /// The probe's error message, if it reported one.
    #[serde(rename = "e", skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Stored pixel dimensions.
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    pub stored: Option<Dimensions>,
    /// Display dimensions, when they differ from the stored ones.
    #[serde(rename = "d", skip_serializing_if = "Option::is_none")]
    pub display: Option<Dimensions>,
    /// Camera EXIF in the ADR-0015 sense.
    #[serde(rename = "c", skip_serializing_if = "is_false")]
    pub camera_exif: bool,
    #[serde(rename = "p", skip_serializing_if = "is_false")]
    pub has_palette: bool,
    #[serde(rename = "a", skip_serializing_if = "is_false")]
    pub has_alpha: bool,
    /// Video duration from the probe window.
    #[serde(rename = "vd", skip_serializing_if = "Option::is_none")]
    pub video_duration_ms: Option<u64>,
    /// The video carries make, model or a creation date.
    #[serde(rename = "vm", skip_serializing_if = "is_false")]
    pub video_camera_meta: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One human-readable reason behind a classification (ADR-0015).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ClassReason {
    /// Make, model, DateTimeOriginal or exposure data.
    CameraExif,
    /// Inside a known cache folder (the name of the rule, e.g. "Lightroom previews").
    CacheFolder {
        rule: String,
    },
    /// Phone/camera video container (make/model or creation date).
    CameraVideo,
    /// Shorter side below the setting.
    ShortSideBelow {
        side: u32,
        min: u32,
    },
    /// Pixel count below the setting.
    PixelsBelow {
        pixels: u64,
        min: u64,
    },
    VideoTooShort {
        duration_ms: u64,
        min_ms: u64,
    },
    VideoTooSmall {
        side: u32,
        min: u32,
    },
    /// Graphic format with palette, alpha, or an icon/vector format.
    GraphicFormat {
        detail: String,
    },
    /// None of the camera rules matched.
    NoCameraMetadata,
    /// Video without device make/model or creation date.
    VideoWithoutCameraMetadata,
    Unsupported {
        reason: String,
    },
    ProbeFailed {
        message: String,
    },
    /// Dimensions could not be determined.
    UnknownDimensions,
}

impl fmt::Display for ClassReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassReason::CameraExif => f.write_str("camera EXIF"),
            ClassReason::CacheFolder { rule } => write!(f, "inside a cache folder ({rule})"),
            ClassReason::CameraVideo => f.write_str("camera video metadata"),
            ClassReason::ShortSideBelow { side, min } => {
                write!(
                    f,
                    "shorter side {side} px is below the {min} px minimum side"
                )
            }
            ClassReason::PixelsBelow { pixels, min } => {
                write!(f, "{pixels} pixels is below the {min} pixel minimum")
            }
            ClassReason::VideoTooShort {
                duration_ms,
                min_ms,
            } => write!(
                f,
                "duration {:.1} s is below the {} s minimum duration",
                *duration_ms as f64 / 1000.0,
                *min_ms as f64 / 1000.0
            ),
            ClassReason::VideoTooSmall { side, min } => {
                write!(
                    f,
                    "video shorter side {side} px is below the {min} px minimum"
                )
            }
            ClassReason::GraphicFormat { detail } => write!(f, "graphic ({detail})"),
            ClassReason::NoCameraMetadata => f.write_str("no camera metadata"),
            ClassReason::VideoWithoutCameraMetadata => f.write_str("video without camera metadata"),
            ClassReason::Unsupported { reason } => write!(f, "unsupported format: {reason}"),
            ClassReason::ProbeFailed { message } => write!(f, "probe failed: {message}"),
            ClassReason::UnknownDimensions => f.write_str("dimensions unknown"),
        }
    }
}

/// What a stored thumbnail row holds (ADR-0016).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbKind {
    /// Made from the image or its embedded preview.
    Image,
    /// A video frame from the configured poster-frame tool.
    PosterFrame,
    /// No pixels: "preview unavailable" (HEIC without decoder) or a video placeholder.
    Placeholder,
}

impl ThumbKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ThumbKind::Image => "image",
            ThumbKind::PosterFrame => "poster_frame",
            ThumbKind::Placeholder => "placeholder",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "image" => ThumbKind::Image,
            "poster_frame" => ThumbKind::PosterFrame,
            "placeholder" => ThumbKind::Placeholder,
            _ => return None,
        })
    }
}

/// What a placeholder shows instead of pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaceholderInfo {
    pub media_kind: MediaKind,
    pub format: Option<Format>,
    pub duration_ms: Option<u64>,
    pub capture_date: Option<CaptureTime>,
    /// e.g. "No HEIC decoder on this system".
    pub note: String,
    /// Why there are no pixels (absent in records stored before this field existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<crate::PlaceholderReason>,
}

/// A grid thumbnail (384 px long edge JPEG at q82) or placeholder, ready for the thumbnail store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThumbnailRecord {
    pub kind: ThumbKind,
    pub width: u32,
    pub height: u32,
    /// Dominant colour (sRGB).
    pub dominant_rgb: [u8; 3],
    /// JPEG bytes; empty for placeholders.
    pub jpeg: Vec<u8>,
    pub placeholder: Option<PlaceholderInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + fmt::Debug>(v: &T) {
        let json = serde_json::to_string(v).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, v, "{json}");
    }

    #[test]
    fn probe_record_round_trips() {
        let mut p = ProbeRecord::empty(1234);
        p.format = Some(Format::Jpeg);
        p.stored = Some(Dimensions::new(4000, 3000));
        p.orientation = Orientation(6);
        p.display = p.stored.map(|d| d.oriented(p.orientation));
        p.preview = Some(EmbeddedPreview {
            source: PreviewSource::ExifThumbnail,
            range: ByteRange {
                offset: 10,
                len: 20,
            },
            dims: Some(Dimensions::new(160, 120)),
            decodable: true,
        });
        p.camera.make = Some("Canon".into());
        p.error = Some(ProbeError {
            kind: ProbeErrorKind::Truncated,
            message: "x".into(),
        });
        p.video = Some(VideoSummary::default());
        round_trip(&p);
        assert_eq!(p.display, Some(Dimensions::new(3000, 4000)));
    }

    #[test]
    fn metadata_and_settings_round_trip() {
        let m = Metadata {
            exposure: Some(URational { num: 1, den: 250 }),
            capture_date: Some(CaptureDate {
                value: CaptureTime::from_parts(2010, 5, 1, 10, 0, 0),
                provenance: DateProvenance::DateTimeOriginal,
                undated: false,
                precision: DatePrecision::Second,
            }),
            warnings: vec!["w".into()],
            ..Default::default()
        };
        round_trip(&m);
        round_trip(&ClassifierSettings::default());
        let s: ClassifierSettings = serde_json::from_str(r#"{"min_side_px":100}"#).expect("parse");
        assert_eq!(s.min_side_px, 100);
        assert_eq!(s.min_pixels, 120_000);
        for r in [
            ClassReason::CameraExif,
            ClassReason::CacheFolder { rule: "x".into() },
            ClassReason::VideoTooShort {
                duration_ms: 1500,
                min_ms: 2000,
            },
        ] {
            round_trip(&r);
        }
        assert_eq!(ClassReason::CameraExif.to_string(), "camera EXIF");
        assert!(
            ClassReason::ShortSideBelow {
                side: 180,
                min: 200
            }
            .to_string()
            .contains("200 px minimum side")
        );
    }

    /// Metadata JSON written before ADR-0069 has no `precision`: it parses as `second`.
    #[test]
    fn capture_date_without_precision_is_to_the_second() {
        let old = r#"{"value":{"local_seconds":1272708000,"millis":0,"offset_minutes":null},
                      "provenance":"file_mtime","undated":true}"#;
        let d: CaptureDate = serde_json::from_str(old).expect("old capture date");
        assert_eq!(d.precision, DatePrecision::Second);
        assert_eq!(d.provenance, DateProvenance::FileMtime);
        let mut stored = serde_json::to_value(Metadata::default()).expect("json");
        stored["capture_date"] = serde_json::from_str(old).expect("value");
        let m: Metadata = serde_json::from_value(stored).expect("old metadata");
        assert_eq!(m.capture_date, Some(d));
        let month = CaptureDate {
            value: CaptureTime::from_parts(2019, 7, 1, 0, 0, 0),
            provenance: DateProvenance::FolderName,
            undated: false,
            precision: DatePrecision::Month,
        };
        round_trip(&month);
        assert_eq!(month.display(), "July 2019");
        assert_eq!(month.iso(), "2019-07");
        assert!(!month.is_embedded());
        let day = CaptureDate {
            value: CaptureTime::from_parts(2019, 7, 4, 0, 0, 0),
            precision: DatePrecision::Day,
            ..month
        };
        assert_eq!(day.display(), "4 July 2019");
        assert_eq!(day.iso(), "2019-07-04");
        for p in [
            DateProvenance::DateTimeOriginal,
            DateProvenance::CreateDate,
            DateProvenance::Container,
            DateProvenance::FileMtime,
            DateProvenance::FolderName,
        ] {
            assert_eq!(DateProvenance::parse(p.as_str()), Some(p));
        }
        for p in [
            DatePrecision::Second,
            DatePrecision::Day,
            DatePrecision::Month,
        ] {
            assert_eq!(DatePrecision::from_code(p.code()), Some(p));
        }
    }

    #[test]
    fn thumbnail_record_round_trips() {
        let t = ThumbnailRecord {
            kind: ThumbKind::Placeholder,
            width: 0,
            height: 0,
            dominant_rgb: [1, 2, 3],
            jpeg: vec![],
            placeholder: Some(PlaceholderInfo {
                media_kind: MediaKind::Video,
                format: Some(Format::Mov),
                duration_ms: Some(12_500),
                capture_date: None,
                note: "n".into(),
                reason: None,
            }),
        };
        round_trip(&t);
        for k in [
            ThumbKind::Image,
            ThumbKind::PosterFrame,
            ThumbKind::Placeholder,
        ] {
            assert_eq!(ThumbKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn dimensions_fit_and_orient() {
        let d = Dimensions::new(4000, 3000);
        assert_eq!(d.fit_long_edge(384), Dimensions::new(384, 288));
        assert_eq!(
            Dimensions::new(3000, 4000).fit_long_edge(384),
            Dimensions::new(288, 384)
        );
        assert_eq!(
            Dimensions::new(100, 50).fit_long_edge(384),
            Dimensions::new(100, 50)
        );
        assert_eq!(d.oriented(Orientation(8)), Dimensions::new(3000, 4000));
        assert_eq!(d.short_edge(), 3000);
    }
}
