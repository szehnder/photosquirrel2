//! Grid thumbnails (ADR-0016): 384 px long edge JPEG at quality 82 with orientation applied, dominant colour
//! in Oklab, embedded previews preferred when their long edge is >= 320 px, placeholders when no decoder.

use image::ImageEncoder;
use ps_model::{
    CaptureTime, Format, MediaKind, PlaceholderInfo, PlaceholderReason, ProbeRecord, ThumbKind,
    ThumbnailRecord,
};

use crate::decode::{self, DecodeError, DecodeSource, Decoded, RgbImage};
use crate::decoders::Decoders;

/// The placeholder for a decode that cannot produce pixels on this machine, with its reason (ADR-0109): a missing
/// decoder, an AVIF sequence, a format variant this version does not decode (`DecodeError::Unsupported`, a RAW
/// without an embedded preview, a decode too large for bounded memory) or data the decoder rejected
/// (`Unreadable`, with the error). `None` when the decode did not complete for a reason that says nothing about
/// the file (cancelled, interrupted, refused): those are retried, not recorded.
pub fn placeholder_for_error(
    probe: &ProbeRecord,
    capture: Option<CaptureTime>,
    e: &DecodeError,
) -> Option<ThumbnailRecord> {
    let reason = reason_for_error(e)?;
    // The photo's own capture date when the caller has none (the grid shows it on the placeholder).
    let capture = capture.or(probe.camera.date_time_original);
    let note = match &reason {
        PlaceholderReason::DecoderMissing { key } => crate::decoders::missing_note(*key),
        PlaceholderReason::Sequence => "preview unavailable (AVIF sequence)".to_owned(),
        PlaceholderReason::UnsupportedVariant { detail } => {
            format!("preview unavailable (not supported: {detail})")
        }
        PlaceholderReason::Unreadable { message } => {
            format!("preview unavailable (could not be decoded: {message})")
        }
        PlaceholderReason::NoPosterTool | PlaceholderReason::NeedsNewerVersion => {
            "preview unavailable".to_owned()
        }
    };
    Some(placeholder_for(probe, capture, &note, Some(reason)))
}

/// The placeholder reason of a decode error ([`placeholder_for_error`]); `None` for errors that say nothing
/// about the file.
pub fn reason_for_error(e: &DecodeError) -> Option<PlaceholderReason> {
    Some(match e {
        DecodeError::DecoderMissing(key) => PlaceholderReason::DecoderMissing { key: *key },
        DecodeError::Sequence => PlaceholderReason::Sequence,
        DecodeError::Unsupported(detail) => PlaceholderReason::UnsupportedVariant {
            detail: detail.clone(),
        },
        DecodeError::NoRawPreview => PlaceholderReason::UnsupportedVariant {
            detail: "RAW files without an embedded preview".into(),
        },
        DecodeError::TooLarge(detail) => PlaceholderReason::UnsupportedVariant {
            detail: format!("files this large ({detail})"),
        },
        DecodeError::Failed(message) => PlaceholderReason::Unreadable {
            message: message.clone(),
        },
        DecodeError::Interrupted(_) | DecodeError::Refused(_) | DecodeError::Cancelled => {
            return None;
        }
    })
}

/// Grid thumbnail long edge.
pub const THUMB_LONG_EDGE: u32 = 384;
/// Grid thumbnail JPEG quality.
pub const THUMB_QUALITY: u8 = 82;
/// Embedded previews at least this large (long edge) are used instead of decoding the image.
pub const MIN_PREVIEW_LONG_EDGE: u32 = 320;
/// On-demand large preview long edge.
pub const PREVIEW_LONG_EDGE: u32 = 2048;

/// Encode RGB pixels as JPEG.
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::with_capacity(img.data.len() / 8);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            &img.data,
            img.width,
            img.height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| DecodeError::Failed(e.to_string()))?;
    Ok(out)
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// Dominant colour: the mean, in Oklab, of the 8x8 downsample, converted back to sRGB.
pub fn dominant_colour(img: &RgbImage) -> [u8; 3] {
    if img.width == 0 || img.height == 0 {
        return [128, 128, 128];
    }
    let small = decode::resize_exact(
        img,
        8.min(img.width),
        8.min(img.height),
        fast_image_resize::FilterType::Box,
    );
    let mut acc = [0f32; 3];
    let mut n = 0f32;
    for p in small.data.as_chunks::<3>().0 {
        let lab = to_oklab([
            srgb_to_linear(p[0]),
            srgb_to_linear(p[1]),
            srgb_to_linear(p[2]),
        ]);
        for i in 0..3 {
            acc[i] += lab[i];
        }
        n += 1.0;
    }
    if n == 0.0 {
        return [128, 128, 128];
    }
    let rgb = from_oklab(acc.map(|v| v / n));
    rgb.map(linear_to_srgb)
}

/// A thumbnail record from an already-oriented image (resized to 384 px long edge, never upscaled).
pub fn thumbnail_from_image(
    img: &RgbImage,
    kind: ThumbKind,
) -> Result<ThumbnailRecord, DecodeError> {
    let t = decode::resize_to_long_edge(img, THUMB_LONG_EDGE);
    let jpeg = encode_jpeg(&t, THUMB_QUALITY)?;
    Ok(ThumbnailRecord {
        kind,
        width: t.width,
        height: t.height,
        dominant_rgb: dominant_colour(&t),
        jpeg,
        placeholder: None,
    })
}

/// How a thumbnail was produced (for callers and tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbOrigin {
    EmbeddedPreview,
    Analysis,
    MainImage,
    Placeholder,
}

/// An embedded preview usable for the grid thumbnail (long edge >= 320 px and decodable).
pub fn usable_preview(probe: &ProbeRecord) -> bool {
    probe.preview.is_some_and(|p| {
        p.decodable
            && p.dims
                .is_some_and(|d| d.long_edge() >= MIN_PREVIEW_LONG_EDGE)
    })
}

/// A placeholder record (no pixels).
pub fn placeholder(
    probe: &ProbeRecord,
    capture: Option<CaptureTime>,
    note: &str,
) -> ThumbnailRecord {
    placeholder_for(probe, capture, note, None)
}

/// [`placeholder`] with the reason it stands in for pixels (stored with it).
pub fn placeholder_for(
    probe: &ProbeRecord,
    capture: Option<CaptureTime>,
    note: &str,
    reason: Option<PlaceholderReason>,
) -> ThumbnailRecord {
    let kind = probe.kind().unwrap_or(MediaKind::Image);
    let (w, h) = probe
        .display
        .map(|d| d.fit_long_edge(THUMB_LONG_EDGE))
        .map_or((0, 0), |d| (d.width, d.height));
    ThumbnailRecord {
        kind: ThumbKind::Placeholder,
        width: w,
        height: h,
        dominant_rgb: [96, 96, 104],
        jpeg: Vec::new(),
        placeholder: Some(PlaceholderInfo {
            media_kind: kind,
            format: probe.format,
            duration_ms: probe.video.as_ref().and_then(|v| v.duration_ms),
            capture_date: capture.or_else(|| probe.video.as_ref().and_then(|v| v.creation)),
            note: note.to_owned(),
            reason,
            // This build's generation of the format's family (ADR-0109); a scan run with other generations (a
            // test's override) stamps its own.
            generation: crate::decoders::GENERATIONS.of_format(probe.format),
        }),
    }
}

/// Make the grid thumbnail of a still. Prefers an embedded preview >= 320 px; otherwise decodes the main
/// image at the smallest DCT scale reaching 384 px. HEIC without a decoder yields a placeholder naming the
/// missing decoder ("No HEIC decoder on this system").
pub fn make_thumbnail(
    bytes: &[u8],
    probe: &ProbeRecord,
    decoders: &Decoders,
) -> Result<(ThumbnailRecord, ThumbOrigin), DecodeError> {
    if usable_preview(probe)
        && let Some(d) = decode::decode_preview(bytes, probe, THUMB_LONG_EDGE, decoders)
    {
        return Ok((
            thumbnail_from_image(&d.image, ThumbKind::Image)?,
            ThumbOrigin::EmbeddedPreview,
        ));
    }
    match decode::decode_scaled(bytes, probe, THUMB_LONG_EDGE, decoders) {
        Ok(d) => Ok((
            thumbnail_from_image(&d.image, ThumbKind::Image)?,
            origin_of(&d),
        )),
        Err(e) => match placeholder_for_error(probe, None, &e) {
            Some(p) => Ok((p, ThumbOrigin::Placeholder)),
            None => Err(e),
        },
    }
}

fn origin_of(d: &Decoded) -> ThumbOrigin {
    match d.source {
        DecodeSource::Main => ThumbOrigin::MainImage,
        DecodeSource::Preview(_) => ThumbOrigin::EmbeddedPreview,
    }
}

/// The single analysis decode (>= 512 px, oriented) and the grid thumbnail derived from the same read
/// (ADR-0010). The thumbnail comes from an embedded preview >= 320 px when there is one, else from the
/// analysis decode.
pub fn analyze_still(
    bytes: &[u8],
    probe: &ProbeRecord,
    decoders: &Decoders,
) -> Result<(Decoded, ThumbnailRecord, ThumbOrigin), DecodeError> {
    analyze_still_with(bytes, probe, decoders, &|| false)
}

/// [`analyze_still`] with a stop check, called before each strip or tile of a TIFF decode (the scanner's
/// pause and cancel checkpoint): `Err(DecodeError::Cancelled)` once it returns true.
pub fn analyze_still_with(
    bytes: &[u8],
    probe: &ProbeRecord,
    decoders: &Decoders,
    stop: &dyn Fn() -> bool,
) -> Result<(Decoded, ThumbnailRecord, ThumbOrigin), DecodeError> {
    analyze_still_held(bytes, probe, decoders, stop, None)
}

/// [`analyze_still_with`] whose AVIF decode thread keeps `hold` (the scanner's decode-budget permit) until it ends
/// and checks `stop` while it waits (ADR-0097).
pub fn analyze_still_held(
    bytes: &[u8],
    probe: &ProbeRecord,
    decoders: &Decoders,
    stop: &dyn Fn() -> bool,
    hold: Option<std::sync::Arc<dyn crate::av1::DecodeHold>>,
) -> Result<(Decoded, ThumbnailRecord, ThumbOrigin), DecodeError> {
    let analysis = decode::decode_scaled_held(
        bytes,
        probe,
        decode::ANALYSIS_MIN_LONG_EDGE,
        decoders,
        stop,
        hold,
    )?;
    if usable_preview(probe)
        && probe.format != Some(Format::Heic)
        && let Some(p) = decode::decode_preview(bytes, probe, THUMB_LONG_EDGE, decoders)
    {
        let t = thumbnail_from_image(&p.image, ThumbKind::Image)?;
        return Ok((analysis, t, ThumbOrigin::EmbeddedPreview));
    }
    let t = thumbnail_from_image(&analysis.image, ThumbKind::Image)?;
    Ok((analysis, t, ThumbOrigin::Analysis))
}

/// A 2048 px (long edge) JPEG preview, generated on demand (ADR-0016). Returns JPEG bytes and dimensions.
pub fn make_large_preview(
    bytes: &[u8],
    probe: &ProbeRecord,
    long_edge: u32,
    decoders: &Decoders,
) -> Result<(Vec<u8>, u32, u32), DecodeError> {
    let d = decode::decode_scaled(bytes, probe, long_edge, decoders)?;
    let r = decode::resize_to_long_edge(&d.image, long_edge);
    Ok((encode_jpeg(&r, 85)?, r.width, r.height))
}

/// [`make_large_preview`] of a TIFF read from a seekable reader (a file not held in memory), in bounded memory
/// (ADR-0085, ADR-0086).
pub fn make_large_tiff_preview<R: std::io::Read + std::io::Seek>(
    reader: R,
    probe: &ProbeRecord,
    long_edge: u32,
) -> Result<(Vec<u8>, u32, u32), DecodeError> {
    let (d, _) = decode::decode_tiff_reader(reader, probe, long_edge)?;
    let r = decode::resize_to_long_edge(&d.image, long_edge);
    Ok((encode_jpeg(&r, 85)?, r.width, r.height))
}

/// The thumbnail of a video: a poster frame from the configured tool when available, else a placeholder
/// carrying duration and capture date.
pub fn video_thumbnail(
    source_path: &std::path::Path,
    probe: &ProbeRecord,
    capture: Option<CaptureTime>,
    tool: Option<&std::path::Path>,
) -> ThumbnailRecord {
    if let Some(tool) = tool
        && let Ok(jpeg) =
            crate::poster::poster_frame(tool, source_path, crate::poster::DEFAULT_TIMEOUT)
        && let Ok((img, _)) = decode::decode_jpeg(&jpeg, THUMB_LONG_EDGE)
        && let Ok(t) = thumbnail_from_image(&img, ThumbKind::PosterFrame)
    {
        return t;
    }
    placeholder_for(
        probe,
        capture,
        "video (no poster-frame tool)",
        Some(PlaceholderReason::NoPosterTool),
    )
}
