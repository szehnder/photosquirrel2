use serde::{Deserialize, Serialize};

/// Image or video.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
}

/// Camera RAW families (ADR-0017).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawFormat {
    Cr2,
    Cr3,
    Crw,
    Nef,
    Nrw,
    Arw,
    Srf,
    Sr2,
    Dng,
    Raf,
    Orf,
    Rw2,
    Pef,
    Srw,
    X3f,
    Iiq,
    ThreeFr,
    Other,
}

/// File formats PhotoSquirrel recognises by content (magic numbers), not by extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    Webp,
    Tiff,
    Bmp,
    Heic,
    Avif,
    Raw(RawFormat),
    Mov,
    Mp4,
    M4v,
    Avi,
    Mkv,
    ThreeGp,
    Mts,
    Svg,
    Ico,
    Psd,
}

impl Format {
    pub fn kind(self) -> MediaKind {
        match self {
            Format::Mov
            | Format::Mp4
            | Format::M4v
            | Format::Avi
            | Format::Mkv
            | Format::ThreeGp
            | Format::Mts => MediaKind::Video,
            _ => MediaKind::Image,
        }
    }

    pub fn is_raw(self) -> bool {
        matches!(self, Format::Raw(_))
    }

    /// Lossy still formats where recompression loses quality.
    pub fn is_lossy_still(self) -> bool {
        matches!(
            self,
            Format::Jpeg | Format::Heic | Format::Avif | Format::Webp
        )
    }

    /// Lossless still formats (lossless WebP is treated as lossy; we cannot tell cheaply).
    pub fn is_lossless_still(self) -> bool {
        matches!(self, Format::Png | Format::Tiff | Format::Bmp)
    }

    /// Short upper-case label for the UI (`JPEG`, `CR2`, `MOV`, ...).
    pub fn label(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Gif => "GIF",
            Format::Webp => "WEBP",
            Format::Tiff => "TIFF",
            Format::Bmp => "BMP",
            Format::Heic => "HEIC",
            Format::Avif => "AVIF",
            Format::Raw(r) => match r {
                RawFormat::Cr2 => "CR2",
                RawFormat::Cr3 => "CR3",
                RawFormat::Crw => "CRW",
                RawFormat::Nef => "NEF",
                RawFormat::Nrw => "NRW",
                RawFormat::Arw => "ARW",
                RawFormat::Srf => "SRF",
                RawFormat::Sr2 => "SR2",
                RawFormat::Dng => "DNG",
                RawFormat::Raf => "RAF",
                RawFormat::Orf => "ORF",
                RawFormat::Rw2 => "RW2",
                RawFormat::Pef => "PEF",
                RawFormat::Srw => "SRW",
                RawFormat::X3f => "X3F",
                RawFormat::Iiq => "IIQ",
                RawFormat::ThreeFr => "3FR",
                RawFormat::Other => "RAW",
            },
            Format::Mov => "MOV",
            Format::Mp4 => "MP4",
            Format::M4v => "M4V",
            Format::Avi => "AVI",
            Format::Mkv => "MKV",
            Format::ThreeGp => "3GP",
            Format::Mts => "MTS",
            Format::Svg => "SVG",
            Format::Ico => "ICO",
            Format::Psd => "PSD",
        }
    }

    /// Extensions that are consistent with this format (lower case, without the dot). The probe's extension check
    /// and the exclusion rule's "known photo or video extension" ([`crate::is_media_extension`]) read this table.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Jpeg => &["jpg", "jpeg", "jpe", "jfif", "jif"],
            Format::Png => &["png", "apng"],
            Format::Gif => &["gif"],
            Format::Webp => &["webp"],
            Format::Tiff => &["tif", "tiff"],
            Format::Bmp => &["bmp", "dib"],
            Format::Heic => &["heic", "heif", "hif", "heics"],
            Format::Avif => &["avif"],
            Format::Raw(r) => match r {
                RawFormat::Cr2 => &["cr2"],
                RawFormat::Cr3 => &["cr3"],
                RawFormat::Crw => &["crw"],
                RawFormat::Nef | RawFormat::Nrw => &["nef", "nrw"],
                RawFormat::Arw | RawFormat::Srf | RawFormat::Sr2 => &["arw", "srf", "sr2"],
                RawFormat::Dng => &["dng"],
                RawFormat::Raf => &["raf"],
                RawFormat::Orf => &["orf"],
                RawFormat::Rw2 => &["rw2", "raw", "rwl"],
                RawFormat::Pef => &["pef", "dng"],
                RawFormat::Srw => &["srw"],
                RawFormat::X3f => &["x3f"],
                RawFormat::Iiq => &["iiq"],
                RawFormat::ThreeFr => &["3fr", "fff"],
                RawFormat::Other => &[],
            },
            Format::Mov => &["mov", "qt"],
            Format::Mp4 => &["mp4", "m4v", "mov", "m4a"],
            Format::M4v => &["m4v", "mp4"],
            Format::Avi => &["avi"],
            Format::Mkv => &["mkv", "webm", "mka"],
            Format::ThreeGp => &["3gp", "3g2", "mp4"],
            Format::Mts => &["mts", "m2ts", "ts", "m2t"],
            Format::Svg => &["svg"],
            Format::Ico => &["ico", "cur"],
            Format::Psd => &["psd", "psb"],
        }
    }

    /// Stable string for storage (`jpeg`, `raw:cr2`, ...). Round-trips through [`Format::from_storage`].
    pub fn storage_key(self) -> String {
        match self {
            Format::Raw(_) => format!("raw:{}", self.label().to_ascii_lowercase()),
            _ => self.label().to_ascii_lowercase(),
        }
    }

    pub fn from_storage(key: &str) -> Option<Format> {
        ALL_FORMATS.iter().copied().find(|f| f.storage_key() == key)
    }
}

/// Every format, for lookups and exhaustive tests.
pub const ALL_FORMATS: &[Format] = &[
    Format::Jpeg,
    Format::Png,
    Format::Gif,
    Format::Webp,
    Format::Tiff,
    Format::Bmp,
    Format::Heic,
    Format::Avif,
    Format::Raw(RawFormat::Cr2),
    Format::Raw(RawFormat::Cr3),
    Format::Raw(RawFormat::Crw),
    Format::Raw(RawFormat::Nef),
    Format::Raw(RawFormat::Nrw),
    Format::Raw(RawFormat::Arw),
    Format::Raw(RawFormat::Srf),
    Format::Raw(RawFormat::Sr2),
    Format::Raw(RawFormat::Dng),
    Format::Raw(RawFormat::Raf),
    Format::Raw(RawFormat::Orf),
    Format::Raw(RawFormat::Rw2),
    Format::Raw(RawFormat::Pef),
    Format::Raw(RawFormat::Srw),
    Format::Raw(RawFormat::X3f),
    Format::Raw(RawFormat::Iiq),
    Format::Raw(RawFormat::ThreeFr),
    Format::Raw(RawFormat::Other),
    Format::Mov,
    Format::Mp4,
    Format::M4v,
    Format::Avi,
    Format::Mkv,
    Format::ThreeGp,
    Format::Mts,
    Format::Svg,
    Format::Ico,
    Format::Psd,
];

/// What the probe-time classifier decided (ADR-0015).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoClass {
    /// Camera metadata present: certainly a photo or camera video.
    Photo,
    /// Looks like a photo; no camera metadata.
    LikelyPhoto,
    /// A cached/derived thumbnail found in a known cache location. Analyzed (can join groups) but never
    /// preferred as keeper over a better copy.
    ThumbnailCache,
    /// Illustration, UI asset, screenshot-like graphic.
    Graphic,
    /// Icons, tiny images, very short or tiny videos: not worth a full read.
    Junk,
}

impl PhotoClass {
    /// Classes that are fully analyzed (hashed, decoded, thumbnailed).
    pub fn is_candidate(self) -> bool {
        matches!(
            self,
            PhotoClass::Photo | PhotoClass::LikelyPhoto | PhotoClass::ThumbnailCache
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PhotoClass::Photo => "photo",
            PhotoClass::LikelyPhoto => "likely_photo",
            PhotoClass::ThumbnailCache => "thumbnail_cache",
            PhotoClass::Graphic => "graphic",
            PhotoClass::Junk => "junk",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "photo" => PhotoClass::Photo,
            "likely_photo" => PhotoClass::LikelyPhoto,
            "thumbnail_cache" => PhotoClass::ThumbnailCache,
            "graphic" => PhotoClass::Graphic,
            "junk" => PhotoClass::Junk,
            _ => return None,
        })
    }
}

/// The classifier's decision with its human-readable reasons.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    pub class: PhotoClass,
    pub reasons: Vec<String>,
}

/// Per-file pipeline stage (ADR-0008).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Discovered,
    Probed,
    Analyzed,
    Thumbed,
    /// Classified as not worth analyzing (reason stored with the classification).
    Skipped,
    /// Failed; error stored, retried on later runs (at most twice).
    Failed,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Stage; 6] = [
        Stage::Discovered,
        Stage::Probed,
        Stage::Analyzed,
        Stage::Thumbed,
        Stage::Skipped,
        Stage::Failed,
    ];

    /// No more scan work is needed for this file.
    pub fn is_terminal(self) -> bool {
        matches!(self, Stage::Thumbed | Stage::Skipped)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Discovered => "discovered",
            Stage::Probed => "probed",
            Stage::Analyzed => "analyzed",
            Stage::Thumbed => "thumbed",
            Stage::Skipped => "skipped",
            Stage::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "discovered" => Stage::Discovered,
            "probed" => Stage::Probed,
            "analyzed" => Stage::Analyzed,
            "thumbed" => Stage::Thumbed,
            "skipped" => Stage::Skipped,
            "failed" => Stage::Failed,
            _ => return None,
        })
    }
}

/// Two 64-bit perceptual hashes computed on the oriented image (ADR-0012).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PerceptualHash {
    pub phash: u64,
    pub dhash: u64,
    /// Algorithm version; a change triggers re-hashing from stored thumbnails.
    pub version: u8,
}

impl PerceptualHash {
    pub fn phash_distance(&self, other: &PerceptualHash) -> u32 {
        (self.phash ^ other.phash).count_ones()
    }

    pub fn dhash_distance(&self, other: &PerceptualHash) -> u32 {
        (self.dhash ^ other.dhash).count_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_keys_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for f in ALL_FORMATS {
            let k = f.storage_key();
            assert!(seen.insert(k.clone()), "duplicate key {k}");
            assert_eq!(Format::from_storage(&k), Some(*f));
        }
    }

    #[test]
    fn stage_and_class_strings_round_trip() {
        for s in [
            Stage::Discovered,
            Stage::Probed,
            Stage::Analyzed,
            Stage::Thumbed,
            Stage::Skipped,
            Stage::Failed,
        ] {
            assert_eq!(Stage::parse(s.as_str()), Some(s));
        }
        for c in [
            PhotoClass::Photo,
            PhotoClass::LikelyPhoto,
            PhotoClass::ThumbnailCache,
            PhotoClass::Graphic,
            PhotoClass::Junk,
        ] {
            assert_eq!(PhotoClass::parse(c.as_str()), Some(c));
        }
    }

    #[test]
    fn video_formats_are_videos() {
        assert_eq!(Format::Mov.kind(), MediaKind::Video);
        assert_eq!(Format::Jpeg.kind(), MediaKind::Image);
        assert!(Format::Raw(RawFormat::Cr2).is_raw());
    }
}
