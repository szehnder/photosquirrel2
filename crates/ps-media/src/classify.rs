//! The probe-time photo classifier (ADR-0015). Pure: depends only on the probe record, the path, settings
//! and (optionally) folder hints the scanner already has from its directory listing.

use std::path::{Component, Path};

use ps_model::{
    ClassReason, Classification, ClassifierSettings, Format, MediaKind, PhotoClass, ProbeRecord,
};

/// Facts about the file's folder that the scanner knows from listing it (no I/O here).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderHints {
    /// The folder also contains a `Thumbs.db` (Windows thumbnail cache).
    pub has_thumbs_db: bool,
}

impl FolderHints {
    /// Hints from a list of sibling file names.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            has_thumbs_db: names
                .into_iter()
                .any(|n| n.eq_ignore_ascii_case("thumbs.db")),
        }
    }

    /// Hints by listing `dir` (read-only). Prefer [`FolderHints::from_names`] inside the scanner.
    pub fn from_dir(dir: &Path) -> Self {
        let names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        Self::from_names(names.iter().map(String::as_str))
    }
}

/// Known cache locations: (rule name, predicate over lower-cased path components).
fn cache_rule(components: &[String], settings: &ClassifierSettings) -> Option<String> {
    for (i, c) in components.iter().enumerate() {
        let c = c.as_str();
        let next = components.get(i + 1).map(String::as_str);
        let rule = match c {
            ".thumbnails" => Some(".thumbnails"),
            ".cache" => Some(".cache"),
            "appdata" => Some("AppData"),
            "node_modules" => Some("node_modules"),
            "resources" if matches!(next, Some("proxies" | "derivatives")) => {
                Some("Apple Photos/iPhoto resources/proxies")
            }
            _ if c.ends_with("previews.lrdata") => Some("Lightroom previews"),
            _ => None,
        };
        if let Some(r) = rule {
            return Some(r.to_owned());
        }
        for p in &settings.extra_cache_patterns {
            if !p.is_empty() && c == p.to_ascii_lowercase() {
                return Some(p.clone());
            }
        }
    }
    None
}

fn folder_components(path: &Path) -> Vec<String> {
    let parent = path.parent().unwrap_or(Path::new(""));
    parent
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// Classify without folder hints.
pub fn classify(
    probe: &ProbeRecord,
    path: &Path,
    settings: &ClassifierSettings,
) -> (PhotoClass, Vec<ClassReason>) {
    classify_with(probe, path, settings, FolderHints::default())
}

/// Classify in the fixed ADR-0015 order: cache path, camera EXIF, video rules, graphic, size, likely photo.
pub fn classify_with(
    probe: &ProbeRecord,
    path: &Path,
    settings: &ClassifierSettings,
    hints: FolderHints,
) -> (PhotoClass, Vec<ClassReason>) {
    let Some(format) = probe.format else {
        let reason = probe
            .unsupported_reason
            .clone()
            .unwrap_or_else(|| "unrecognised content".into());
        let mut r = vec![ClassReason::Unsupported { reason }];
        if let Some(e) = &probe.error {
            r.push(ClassReason::ProbeFailed {
                message: e.to_string(),
            });
        }
        return (PhotoClass::Junk, r);
    };
    let mut extra = Vec::new();
    if let Some(e) = &probe.error {
        extra.push(ClassReason::ProbeFailed {
            message: e.to_string(),
        });
        if probe.stored.is_none() {
            return (PhotoClass::Junk, extra);
        }
    }
    let with = |class, mut r: Vec<ClassReason>| {
        r.extend(extra.iter().cloned());
        (class, r)
    };

    // 1. Cache folders.
    let comps = folder_components(path);
    if let Some(rule) = cache_rule(&comps, settings) {
        return with(
            PhotoClass::ThumbnailCache,
            vec![ClassReason::CacheFolder { rule }],
        );
    }
    if hints.has_thumbs_db && format.kind() == MediaKind::Image {
        return with(
            PhotoClass::ThumbnailCache,
            vec![ClassReason::CacheFolder {
                rule: "folder with Thumbs.db".into(),
            }],
        );
    }

    let kind = format.kind();
    // 2. Camera EXIF (stills).
    if kind == MediaKind::Image && probe.camera.is_camera_exif() {
        return with(PhotoClass::Photo, vec![ClassReason::CameraExif]);
    }

    // 3. Videos.
    if kind == MediaKind::Video {
        let v = probe.video.clone().unwrap_or_default();
        if let Some(d) = v.duration_ms
            && d < settings.min_video_duration_ms
        {
            return with(
                PhotoClass::Junk,
                vec![ClassReason::VideoTooShort {
                    duration_ms: d,
                    min_ms: settings.min_video_duration_ms,
                }],
            );
        }
        if let Some(dims) = probe.stored
            && dims.short_edge() < settings.min_video_side_px
        {
            return with(
                PhotoClass::Junk,
                vec![ClassReason::VideoTooSmall {
                    side: dims.short_edge(),
                    min: settings.min_video_side_px,
                }],
            );
        }
        if v.make.is_some() || v.model.is_some() || v.creation.is_some() {
            return with(PhotoClass::Photo, vec![ClassReason::CameraVideo]);
        }
        return with(
            PhotoClass::LikelyPhoto,
            vec![ClassReason::VideoWithoutCameraMetadata],
        );
    }

    // 4. Graphics.
    let graphic_detail = match format {
        Format::Svg => Some("vector image".to_owned()),
        Format::Ico => Some("icon file".to_owned()),
        Format::Png | Format::Gif if probe.has_palette && probe.has_alpha => {
            Some("palette with transparency".to_owned())
        }
        Format::Png | Format::Gif if probe.has_palette => Some("palette image".to_owned()),
        Format::Png if probe.has_alpha => Some("alpha channel".to_owned()),
        _ => None,
    };
    if let Some(detail) = graphic_detail {
        return with(
            PhotoClass::Graphic,
            vec![ClassReason::GraphicFormat { detail }],
        );
    }

    // 5. Size.
    let Some(dims) = probe.display.or(probe.stored) else {
        return with(
            PhotoClass::LikelyPhoto,
            vec![
                ClassReason::UnknownDimensions,
                ClassReason::NoCameraMetadata,
            ],
        );
    };
    let mut small = Vec::new();
    if dims.short_edge() < settings.min_side_px {
        small.push(ClassReason::ShortSideBelow {
            side: dims.short_edge(),
            min: settings.min_side_px,
        });
    }
    if dims.pixels() < settings.min_pixels {
        small.push(ClassReason::PixelsBelow {
            pixels: dims.pixels(),
            min: settings.min_pixels,
        });
    }
    if !small.is_empty() {
        return with(PhotoClass::Junk, small);
    }

    // 6. Everything else.
    with(PhotoClass::LikelyPhoto, vec![ClassReason::NoCameraMetadata])
}

/// The classification as the shared model type (reasons as display strings).
pub fn classification(
    probe: &ProbeRecord,
    path: &Path,
    settings: &ClassifierSettings,
    hints: FolderHints,
) -> Classification {
    let (class, reasons) = classify_with(probe, path, settings, hints);
    Classification {
        class,
        reasons: reasons.iter().map(ToString::to_string).collect(),
    }
}
