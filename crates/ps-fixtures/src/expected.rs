//! The class the ADR-0015 classifier should assign with default settings, in the rule order of the
//! media-analysis design: cache path -> camera EXIF -> video rules -> graphic -> size -> likely_photo.

use ps_model::PhotoClass;

/// Default thresholds (ADR-0015).
pub const MIN_SIDE: u32 = 200;
pub const MIN_PIXELS: u64 = 120_000;
pub const MIN_VIDEO_MS: u64 = 2_000;
pub const MIN_VIDEO_SIDE: u32 = 320;

/// Known cache-folder path fragments.
pub fn is_cache_path(path: &str) -> bool {
    const PARTS: &[&str] = &[
        "/.thumbnails/",
        "/AppData/",
        "/node_modules/",
        "/.cache/",
        "/resources/proxies/",
        "Previews.lrdata/",
    ];
    PARTS.iter().any(|p| path.contains(p))
}

/// What the generator knows about a file, for classification.
pub struct Facts<'a> {
    pub path: &'a str,
    pub camera_exif: bool,
    /// Palette or alpha channel (PNG/GIF/ICO).
    pub palette_or_alpha: bool,
    pub graphic_format: bool,
    pub width: u32,
    pub height: u32,
    /// Duration for videos.
    pub video_ms: Option<u64>,
    /// Video carries make/model or a creation date.
    pub video_meta: bool,
}

pub fn expected_class(f: &Facts<'_>) -> PhotoClass {
    if is_cache_path(f.path) {
        return PhotoClass::ThumbnailCache;
    }
    if let Some(ms) = f.video_ms {
        if ms < MIN_VIDEO_MS || f.width.min(f.height) < MIN_VIDEO_SIDE {
            return PhotoClass::Junk;
        }
        return if f.video_meta {
            PhotoClass::Photo
        } else {
            PhotoClass::LikelyPhoto
        };
    }
    if f.camera_exif {
        return PhotoClass::Photo;
    }
    if f.graphic_format && f.palette_or_alpha {
        return PhotoClass::Graphic;
    }
    if f.width.min(f.height) < MIN_SIDE || u64::from(f.width) * u64::from(f.height) < MIN_PIXELS {
        return PhotoClass::Junk;
    }
    PhotoClass::LikelyPhoto
}
