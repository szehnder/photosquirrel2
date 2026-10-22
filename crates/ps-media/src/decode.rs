//! Downscaled decodes (ADR-0010, ADR-0017, ADR-0054, ADR-0055, ADR-0056): JPEG DCT scaling via mozjpeg
//! (libjpeg-turbo; jpeg-decoder for CMYK, corrupt or truncated files and as the fallback), full JPEG decodes via
//! zune-jpeg, PNG/TIFF/WebP/GIF/BMP via `image`, RAW only through embedded previews (never demosaiced),
//! HEIC via libheif behind the `heic` feature. Orientation is applied once, on the decoded buffer.

use std::io::Cursor;

use ps_model::{Dimensions, Format, Orientation, PreviewSource, ProbeRecord};
use thiserror::Error;

use crate::util::contain;

/// Long edge the analysis decode must reach (ADR-0010: smallest scale whose long edge is >= 512 px).
pub const ANALYSIS_MIN_LONG_EDGE: u32 = 512;

/// Memory cap for one decode (hostile headers can claim gigapixel images).
const MAX_DECODE_BYTES: usize = 768 * 1024 * 1024;

/// An 8-bit RGB image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn dims(&self) -> Dimensions {
        Dimensions::new(self.width, self.height)
    }

    /// Pixel at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        match self.data.get(i..i + 3) {
            Some(p) => [p[0], p[1], p[2]],
            None => [0, 0, 0],
        }
    }

    /// Apply an EXIF orientation (1-8), returning the upright image.
    pub fn oriented(self, o: Orientation) -> RgbImage {
        let o = o.0;
        if !(2..=8).contains(&o) {
            return self;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (dw, dh) = if (5..=8).contains(&o) { (h, w) } else { (w, h) };
        let mut out = vec![0u8; dw * dh * 3];
        for y in 0..h {
            let row = &self.data[y * w * 3..(y + 1) * w * 3];
            for x in 0..w {
                let (dx, dy) = match o {
                    2 => (w - 1 - x, y),
                    3 => (w - 1 - x, h - 1 - y),
                    4 => (x, h - 1 - y),
                    5 => (y, x),
                    6 => (h - 1 - y, x),
                    7 => (h - 1 - y, w - 1 - x),
                    _ => (y, w - 1 - x),
                };
                let d = (dy * dw + dx) * 3;
                out[d..d + 3].copy_from_slice(&row[x * 3..x * 3 + 3]);
            }
        }
        RgbImage {
            width: dw as u32,
            height: dh as u32,
            data: out,
        }
    }
}

/// What was decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeSource {
    /// The main image.
    Main,
    /// An embedded preview (no sensor data or full image decoded).
    Preview(PreviewSource),
}

/// A decoded, oriented image plus how it was obtained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub image: RgbImage,
    pub source: DecodeSource,
    /// DCT scale denominator used (1, 2, 4 or 8); 1 for non-JPEG decodes.
    pub scale_denom: u32,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("format not decodable: {0}")]
    Unsupported(String),
    #[error("HEIC/HEIF/AVIF decoder not built in (enable the `heic` feature)")]
    HeicUnavailable,
    #[error("no embedded preview in this RAW file")]
    NoRawPreview,
    #[error("decode failed: {0}")]
    Failed(String),
}

/// Choose the DCT denominator (8, 4, 2, 1) giving the smallest decode whose long edge is >= `min_long`.
pub fn choose_denominator(w: u32, h: u32, min_long: u32) -> u32 {
    let long = w.max(h);
    for d in [8u32, 4, 2] {
        if long.div_ceil(d) >= min_long {
            return d;
        }
    }
    1
}

/// Decode a JPEG to the smallest DCT scale whose long edge is >= `min_long` (unoriented).
pub fn decode_jpeg(bytes: &[u8], min_long: u32) -> Result<(RgbImage, u32), DecodeError> {
    contain(|| decode_jpeg_inner(bytes, min_long))
        .unwrap_or_else(|| Err(DecodeError::Failed("JPEG decoder failure".into())))
}

/// Progressive JPEG coefficient memory (ADR-0056). Every decoder (libjpeg, jpeg-decoder, zune-jpeg) buffers a
/// progressive image's full-resolution DCT coefficients, 2 bytes each, before it can output a row, whatever the
/// output scale: a 250 MP 4:4:4 progressive file needs 1.5 GB. Above [`PROGRESSIVE_SERIAL_BYTES`] a decode
/// waits for any other such decode in the process, so 15 analysis workers cannot each hold one at once; above
/// [`PROGRESSIVE_MAX_BYTES`] it is refused (hostile headers claim gigapixel frames in a few bytes).
pub const PROGRESSIVE_SERIAL_BYTES: u64 = 128 * 1024 * 1024;
/// See [`PROGRESSIVE_SERIAL_BYTES`].
pub const PROGRESSIVE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How a JPEG decode may use memory, decided from its frame header alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegMemoryPlan {
    /// Sequential, or progressive with a small coefficient buffer: decode on the calling worker.
    Parallel,
    /// A large progressive coefficient buffer: decode, but one such decode at a time per process.
    Serialized,
    /// The coefficient buffer would exceed [`PROGRESSIVE_MAX_BYTES`]: refuse (analysis failed).
    TooLarge,
}

/// Upper bound of the coefficient memory a progressive frame needs: every component at full resolution
/// (4:4:4; subsampled chroma needs less), 64 coefficients of 2 bytes per 8x8 block, blocks rounded up.
pub fn progressive_coefficient_bytes(width: u32, height: u32, components: u32) -> u64 {
    let blocks = u64::from(width.div_ceil(8)) * u64::from(height.div_ceil(8));
    blocks * 64 * 2 * u64::from(components)
}

/// The memory plan of a frame (see [`JpegMemoryPlan`]).
pub fn jpeg_memory_plan(
    width: u32,
    height: u32,
    components: u32,
    progressive: bool,
) -> JpegMemoryPlan {
    if !progressive {
        return JpegMemoryPlan::Parallel;
    }
    match progressive_coefficient_bytes(width, height, components) {
        b if b > PROGRESSIVE_MAX_BYTES => JpegMemoryPlan::TooLarge,
        b if b > PROGRESSIVE_SERIAL_BYTES => JpegMemoryPlan::Serialized,
        _ => JpegMemoryPlan::Parallel,
    }
}

/// Held for the whole of a [`JpegMemoryPlan::Serialized`] decode.
static LARGE_PROGRESSIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn decode_jpeg_inner(bytes: &[u8], min_long: u32) -> Result<(RgbImage, u32), DecodeError> {
    let mut dec = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    dec.set_max_decoding_buffer_size(MAX_DECODE_BYTES);
    dec.read_info()
        .map_err(|e| DecodeError::Failed(e.to_string()))?;
    let info = dec
        .info()
        .ok_or_else(|| DecodeError::Failed("no JPEG frame header".into()))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    if w == 0 || h == 0 {
        return Err(DecodeError::Failed("zero JPEG dimensions".into()));
    }
    let components = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 | jpeg_decoder::PixelFormat::L16 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::CMYK32 => 4,
    };
    let progressive = info.coding_process == jpeg_decoder::CodingProcess::DctProgressive;
    let _serial = match jpeg_memory_plan(w, h, components, progressive) {
        JpegMemoryPlan::Parallel => None,
        JpegMemoryPlan::Serialized => Some(
            LARGE_PROGRESSIVE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ),
        JpegMemoryPlan::TooLarge => {
            return Err(DecodeError::Failed(format!(
                "progressive JPEG too large to decode ({w}x{h}, {} MB of coefficients)",
                progressive_coefficient_bytes(w, h, components) / (1024 * 1024)
            )));
        }
    };
    let denom = choose_denominator(w, h, min_long);
    if denom == 1
        && matches!(
            info.pixel_format,
            jpeg_decoder::PixelFormat::RGB24 | jpeg_decoder::PixelFormat::L8
        )
    {
        // Full-size decode: zune-jpeg is the faster decoder (ADR-0017).
        if let Ok(img) = decode_zune(bytes) {
            return Ok((img, 1));
        }
    }
    if denom > 1
        && matches!(
            info.pixel_format,
            jpeg_decoder::PixelFormat::RGB24 | jpeg_decoder::PixelFormat::L8
        )
        && (w as usize) * (h as usize) * 3 <= MAX_DECODE_BYTES
        && let Ok(img) = decode_mozjpeg_scaled(bytes, denom)
        && (img.width, img.height) == (w.div_ceil(denom), h.div_ceil(denom))
    {
        // DCT-scaled decode: libjpeg-turbo's entropy decoder and reduced IDCTs (ADR-0054). Corrupt or
        // truncated data makes it fail (ADR-0055), and jpeg-decoder below decides, as before mozjpeg.
        return Ok((img, denom));
    }
    let (sw, sh) = if denom > 1 {
        dec.scale(w.div_ceil(denom) as u16, h.div_ceil(denom) as u16)
            .map_err(|e| DecodeError::Failed(e.to_string()))?
    } else {
        (info.width, info.height)
    };
    let px = dec
        .decode()
        .map_err(|e| DecodeError::Failed(e.to_string()))?;
    let fmt = dec
        .info()
        .map(|i| i.pixel_format)
        .unwrap_or(info.pixel_format);
    let (sw, sh) = (u32::from(sw), u32::from(sh));
    let n = sw as usize * sh as usize;
    let data = match fmt {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|v| [*v, *v, *v]).collect(),
        jpeg_decoder::PixelFormat::L16 => px
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|v| [v[0], v[0], v[0]])
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => px
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| {
                let k = u16::from(c[3]);
                [c[0], c[1], c[2]].map(|v| ((u16::from(v) * k) / 255) as u8)
            })
            .collect(),
    };
    if data.len() < n * 3 {
        return Err(DecodeError::Failed("short JPEG decode".into()));
    }
    Ok((
        RgbImage {
            width: sw,
            height: sh,
            data,
        },
        denom,
    ))
}

/// Why a strict libjpeg decode stopped.
enum LibjpegStop {
    /// `error_exit`: libjpeg cannot continue.
    Fatal,
    /// `emit_message` with a negative level: libjpeg would carry on with made-up data (grey padding after a
    /// premature end, skipped garbage before a marker, a resync after a corrupt restart interval).
    Warning,
}

/// A libjpeg error manager that unwinds on fatal errors and on every warning (ADR-0055). The mozjpeg crate's
/// default one silences warnings, so a truncated or corrupt file "decodes" with grey padding: a damaged
/// full-size copy could then group with its intact derivatives and win keeper on pixel count.
#[allow(unsafe_code)]
fn strict_error_mgr() -> mozjpeg_sys::jpeg_error_mgr {
    // SAFETY: `jpeg_error_mgr` is a plain C struct of integers, pointers and optional function pointers, for
    // which all-zero is valid; `jpeg_std_error` then fills in libjpeg's defaults (message tables are statics,
    // so the struct can move). Both callbacks below only unwind, which the `unwinding` build of mozjpeg-sys
    // supports for `error_exit` already, and `emit_message` is called from the same libjpeg frames.
    unsafe {
        let mut err: mozjpeg_sys::jpeg_error_mgr = std::mem::zeroed();
        mozjpeg_sys::jpeg_std_error(&mut err);
        err.error_exit = Some(strict_error_exit);
        err.emit_message = Some(strict_emit_message);
        err
    }
}

extern "C-unwind" fn strict_error_exit(_cinfo: &mut mozjpeg_sys::jpeg_common_struct) {
    std::panic::resume_unwind(Box::new(LibjpegStop::Fatal));
}

extern "C-unwind" fn strict_emit_message(
    _cinfo: &mut mozjpeg_sys::jpeg_common_struct,
    msg_level: std::os::raw::c_int,
) {
    if msg_level < 0 {
        std::panic::resume_unwind(Box::new(LibjpegStop::Warning));
    }
}

/// A DCT-scaled (1/`denom`) RGB decode through mozjpeg's libjpeg-turbo core, on the calling thread
/// (ADR-0054). Grayscale sources are expanded to RGB by libjpeg. Any libjpeg error or warning (truncated or
/// corrupt data) is a failure (ADR-0055); the caller then falls back to jpeg-decoder. Public for tests that
/// check intact files take this path.
#[doc(hidden)]
pub fn decode_mozjpeg_scaled(bytes: &[u8], denom: u32) -> Result<RgbImage, DecodeError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mozjpeg_strict(bytes, denom)
    })) {
        Ok(r) => r,
        Err(stop) => Err(DecodeError::Failed(
            match stop.downcast_ref::<LibjpegStop>() {
                Some(LibjpegStop::Warning) => "corrupt or truncated JPEG data",
                Some(LibjpegStop::Fatal) => "libjpeg error",
                None => "JPEG decoder failure",
            }
            .into(),
        )),
    }
}

fn mozjpeg_strict(bytes: &[u8], denom: u32) -> Result<RgbImage, DecodeError> {
    let failed = |e: std::io::Error| DecodeError::Failed(e.to_string());
    let mut d = mozjpeg::Decompress::builder()
        .with_err(strict_error_mgr())
        .from_mem(bytes)
        .map_err(failed)?;
    d.scale((8 / denom.clamp(1, 8)) as u8);
    let mut started = d.rgb().map_err(failed)?;
    let (width, height) = (started.width() as u32, started.height() as u32);
    let data = started.read_scanlines::<u8>().map_err(failed)?;
    if data.len() != width as usize * height as usize * 3 {
        return Err(DecodeError::Failed("short JPEG decode".into()));
    }
    // Reads up to EOI: a missing EOI or trailing scan data warns, and so fails.
    started.finish().map_err(failed)?;
    Ok(RgbImage {
        width,
        height,
        data,
    })
}

fn decode_zune(bytes: &[u8]) -> Result<RgbImage, DecodeError> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    // Strict: lenient zune-jpeg zero-pads truncated or corrupt scan data and returns Ok, which would hash a damaged
    // photo (ADR-0055). An error here falls back to jpeg-decoder, which decides as before ADR-0054.
    let opts = DecoderOptions::default()
        .set_strict_mode(true)
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(65_535)
        .set_max_height(65_535);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), opts);
    d.decode_headers()
        .map_err(|e| DecodeError::Failed(format!("{e:?}")))?;
    let info = d
        .info()
        .ok_or_else(|| DecodeError::Failed("no header".into()))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    if (w as usize) * (h as usize) * 3 > MAX_DECODE_BYTES {
        return Err(DecodeError::Failed("image too large".into()));
    }
    let data = d
        .decode()
        .map_err(|e| DecodeError::Failed(format!("{e:?}")))?;
    if data.len() != w as usize * h as usize * 3 {
        return Err(DecodeError::Failed("unexpected zune output size".into()));
    }
    Ok(RgbImage {
        width: w,
        height: h,
        data,
    })
}

/// Decode PNG/GIF/WebP/TIFF/BMP with `image`, alpha composited over white (unoriented).
pub fn decode_generic(bytes: &[u8]) -> Result<RgbImage, DecodeError> {
    contain(|| {
        let mut reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| DecodeError::Failed(e.to_string()))?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODE_BYTES as u64);
        reader.limits(limits);
        let img = reader
            .decode()
            .map_err(|e| DecodeError::Failed(e.to_string()))?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let data = rgba
            .pixels()
            .flat_map(|p| {
                let a = u16::from(p[3]);
                [p[0], p[1], p[2]].map(|c| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8)
            })
            .collect();
        Ok(RgbImage {
            width: w,
            height: h,
            data,
        })
    })
    .unwrap_or_else(|| Err(DecodeError::Failed("image decoder failure".into())))
}

/// Decode a RAW file's best embedded preview, never the sensor data. Uses the probe's preview location
/// first, then rawler's preview extraction. Unoriented.
pub fn decode_raw_preview(
    bytes: &[u8],
    probe: &ProbeRecord,
    min_long: u32,
) -> Result<(RgbImage, u32), DecodeError> {
    if let Some(p) = probe.preview
        && let Some(j) = p.range.slice(bytes)
        && j.starts_with(&[0xFF, 0xD8])
        && let Ok(r) = decode_jpeg(j, min_long)
    {
        return Ok(r);
    }
    let img = contain(|| -> Option<image::DynamicImage> {
        let src = rawler::rawsource::RawSource::new_from_slice(bytes);
        let dec = rawler::get_decoder(&src).ok()?;
        let params = rawler::decoders::RawDecodeParams::default();
        dec.preview_image(&src, &params)
            .ok()
            .flatten()
            .or_else(|| dec.thumbnail_image(&src, &params).ok().flatten())
    })
    .flatten()
    .ok_or(DecodeError::NoRawPreview)?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    Ok((
        RgbImage {
            width: w,
            height: h,
            data: rgb.into_raw(),
        },
        1,
    ))
}

/// Decode the main image of `bytes` for analysis: the smallest available scale whose long edge is >= 512 px,
/// orientation applied. RAW files decode their embedded preview.
pub fn decode_for_analysis(bytes: &[u8], probe: &ProbeRecord) -> Result<Decoded, DecodeError> {
    decode_scaled(bytes, probe, ANALYSIS_MIN_LONG_EDGE)
}

/// Decode the main image (or a RAW's preview) to the smallest scale with long edge >= `min_long`, oriented.
pub fn decode_scaled(
    bytes: &[u8],
    probe: &ProbeRecord,
    min_long: u32,
) -> Result<Decoded, DecodeError> {
    let format = probe
        .format
        .ok_or_else(|| DecodeError::Unsupported("unknown format".into()))?;
    let (image, denom, source, orient) = match format {
        Format::Jpeg => {
            let (i, d) = decode_jpeg(bytes, min_long)?;
            (i, d, DecodeSource::Main, true)
        }
        Format::Png | Format::Gif | Format::Webp | Format::Tiff | Format::Bmp => {
            (decode_generic(bytes)?, 1, DecodeSource::Main, true)
        }
        Format::Raw(_) => {
            let (i, d) = decode_raw_preview(bytes, probe, min_long)?;
            (i, d, DecodeSource::Preview(PreviewSource::RawPreview), true)
        }
        Format::Heic | Format::Avif => {
            // libheif applies irot/imir itself.
            (
                crate::heic::decode(bytes, None)?,
                1,
                DecodeSource::Main,
                false,
            )
        }
        other => {
            return Err(DecodeError::Unsupported(format!(
                "{} stills are not decoded",
                other.label()
            )));
        }
    };
    let image = if orient {
        image.oriented(probe.orientation)
    } else {
        image
    };
    Ok(Decoded {
        image,
        source,
        scale_denom: denom,
    })
}

/// Decode the probe's embedded preview (JPEG previews, or HEIC thumbnails with the `heic` feature),
/// oriented. `None` when there is no decodable preview.
pub fn decode_preview(bytes: &[u8], probe: &ProbeRecord, min_long: u32) -> Option<Decoded> {
    let p = probe.preview?;
    if !p.decodable {
        return None;
    }
    let (image, denom, orient) = match p.source {
        PreviewSource::HeicThumbnail => {
            (crate::heic::decode(bytes, Some(min_long)).ok()?, 1, false)
        }
        _ => {
            let j = p.range.slice(bytes)?;
            let (i, d) = decode_jpeg(j, min_long).ok()?;
            (i, d, true)
        }
    };
    let image = if orient {
        image.oriented(probe.orientation)
    } else {
        image
    };
    Some(Decoded {
        image,
        source: DecodeSource::Preview(p.source),
        scale_denom: denom,
    })
}

/// Resize to fit `max_long` on the long edge (never upscales) with Lanczos3.
pub fn resize_to_long_edge(img: &RgbImage, max_long: u32) -> RgbImage {
    let target = img.dims().fit_long_edge(max_long);
    resize_exact(
        img,
        target.width,
        target.height,
        fast_image_resize::FilterType::Lanczos3,
    )
}

/// Resize to exactly `w` x `h`.
pub fn resize_exact(
    img: &RgbImage,
    w: u32,
    h: u32,
    filter: fast_image_resize::FilterType,
) -> RgbImage {
    use fast_image_resize as fr;
    if (w, h) == (img.width, img.height) || img.width == 0 || img.height == 0 || w == 0 || h == 0 {
        return img.clone();
    }
    let resized = (|| -> Option<RgbImage> {
        let src = fr::images::ImageRef::new(img.width, img.height, &img.data, fr::PixelType::U8x3)
            .ok()?;
        let mut dst = fr::images::Image::new(w, h, fr::PixelType::U8x3);
        let opts = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(filter));
        fr::Resizer::new().resize(&src, &mut dst, &opts).ok()?;
        Some(RgbImage {
            width: w,
            height: h,
            data: dst.into_vec(),
        })
    })();
    resized.unwrap_or_else(|| img.clone())
}
