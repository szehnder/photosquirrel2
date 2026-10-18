//! The per-file input of grouping and ranking: everything the rules of ADR-0011, ADR-0013, ADR-0014, ADR-0015
//! and ADR-0035 look at, and nothing else. The scanner/catalog builds one [`DedupInput`] per analyzed file.

use ps_model::{
    CaptureTime, ContentId, ContentIdKind, Dimensions, FileId, Format, MediaKind, MediaMetadata,
    Orientation, PerceptualHash, PhotoClass,
};
use serde::{Deserialize, Serialize};

/// The metadata subset the rules use.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DedupMeta {
    /// Stored pixel dimensions (before orientation).
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Orientation,
    /// Capture time (DateTimeOriginal, else CreateDate, else the container date). Never the file time.
    pub capture_time: Option<CaptureTime>,
    /// `capture_time.millis` was really recorded (SubSecTimeOriginal present), so `millis == 0` means 0 ms.
    pub subsec_present: bool,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub camera_serial: Option<String>,
    /// Maker notes present (ADR-0035 "looks like a camera original").
    pub has_maker_notes: bool,
    /// Untouched camera original (maker notes or full camera EXIF, no editing software): ADR-0014 tier signal.
    pub camera_original: bool,
    pub has_gps: bool,
    /// ExposureTime, FNumber or ISO present.
    pub has_exposure: bool,
    /// Estimated JPEG quality 1-100.
    pub jpeg_quality: Option<u8>,
    /// Video duration (sampled-identity check, ADR-0011).
    pub duration_ms: Option<u64>,
}

impl DedupMeta {
    /// Build from extracted metadata. `camera_original` is `ps_model::Metadata::likely_camera_original`.
    /// Sub-second presence is read from the tag list (`SubSecTimeOriginal`), since `CaptureTime::millis` uses
    /// 0 for "unknown"; a non-zero `millis` also counts as present.
    pub fn from_media(m: &MediaMetadata, camera_original: bool) -> Self {
        let subsec_tag = m.all_tags.iter().any(|(_, name, _)| {
            let n = name.to_ascii_lowercase();
            n == "subsectimeoriginal" || n == "0x9291"
        });
        Self {
            width: m.width,
            height: m.height,
            orientation: m.orientation,
            capture_time: m.capture_time,
            subsec_present: m.capture_time.is_some()
                && (subsec_tag || m.capture_time.is_some_and(|c| c.millis != 0)),
            camera_make: m.camera_make.clone(),
            camera_model: m.camera_model.clone(),
            camera_serial: m.camera_serial.clone(),
            has_maker_notes: m.has_maker_notes,
            camera_original,
            has_gps: m.gps.is_some(),
            has_exposure: m.exposure_s.is_some() || m.aperture.is_some() || m.iso.is_some(),
            jpeg_quality: m.jpeg_quality,
            duration_ms: m.duration_ms,
        }
    }

    /// Stored dimensions when both are known.
    pub fn stored(&self) -> Option<Dimensions> {
        Some(Dimensions::new(self.width?, self.height?))
    }

    /// Display dimensions (orientation applied).
    pub fn display(&self) -> Option<Dimensions> {
        self.stored().map(|d| d.oriented(self.orientation))
    }

    pub fn pixel_count(&self) -> u64 {
        self.stored().map_or(0, Dimensions::pixels)
    }

    /// 0-4: capture time, GPS, make/model, exposure (ADR-0014 "metadata completeness").
    pub fn completeness(&self) -> u8 {
        u8::from(self.capture_time.is_some())
            + u8::from(self.has_gps)
            + u8::from(self.camera_make.is_some() || self.camera_model.is_some())
            + u8::from(self.has_exposure)
    }
}

/// One analyzed file, as grouping and ranking see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DedupInput {
    pub file: FileId,
    /// Content identity (ADR-0011); `None` when the file was not read fully (it then joins no exact edge).
    pub content: Option<ContentId>,
    /// Perceptual hashes (ADR-0012); `None` for videos and undecodable images.
    pub hash: Option<PerceptualHash>,
    pub format: Format,
    pub class: PhotoClass,
    /// The file's path with `/` separators (absolute or source-qualified). Used for the RAW+JPEG folder/stem
    /// rule and the path-depth tie-break.
    pub path: String,
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    pub meta: DedupMeta,
}

impl DedupInput {
    /// The parent folder (`a/b` of `a/b/c.jpg`).
    pub fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// The file name without its last extension.
    pub fn stem(&self) -> &str {
        let name = self
            .path
            .rsplit_once('/')
            .map_or(self.path.as_str(), |(_, n)| n);
        name.rsplit_once('.').map_or(name, |(s, _)| s)
    }

    /// Number of folders above the file (ranking tie-break).
    pub fn depth(&self) -> usize {
        self.path.trim_start_matches('/').matches('/').count()
    }

    pub fn is_image(&self) -> bool {
        self.format.kind() == MediaKind::Image
    }

    /// The key under which byte-identical copies meet: the content identity, plus size, duration and
    /// dimensions for sampled identities (ADR-0011).
    pub fn exact_key(&self) -> Option<ExactKey> {
        let content = self.content?;
        Some(match content.kind {
            ContentIdKind::Full => ExactKey {
                content,
                sampled_facts: None,
            },
            ContentIdKind::Sampled => ExactKey {
                content,
                sampled_facts: Some((
                    self.size,
                    self.meta.duration_ms,
                    self.meta.width,
                    self.meta.height,
                )),
            },
        })
    }
}

/// See [`DedupInput::exact_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExactKey {
    pub content: ContentId,
    pub sampled_facts: Option<(u64, Option<u64>, Option<u32>, Option<u32>)>,
}
