//! TIFF stills decoded one strip or tile at a time and box-downsampled while decoding (ADR-0085).
//!
//! The `tiff` crate does the decompression (none, LZW, Deflate, PackBits, JPEG), predictors, byte order and
//! WhiteIsZero inversion of one chunk; this module chooses the image directory (the first image, or a
//! reduced-resolution level of it that is large enough), visits its chunks in file order, converts every sample
//! to 8 bits, sums it into a downsampling accumulator and converts the colour model once at the end. It holds at
//! most one chunk plus the accumulator, so it works over any `Read + Seek`: the analysis buffer, a file, or the
//! scanner's one-pass reader for TIFFs above 64 MB (ADR-0086), which needs the file-order visit.

use std::io::{Read, Seek, SeekFrom};

use tiff::TiffError;
use tiff::decoder::{ChunkType, Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::decode::{DecodeError, RgbImage};
use crate::util::contain;

/// Largest strip or tile the decoder holds, decoded or compressed (32 MB). Larger chunks are refused with
/// [`DecodeError::TooLarge`]: the caller falls back to a whole-image decode when the file is in memory anyway,
/// and shows a placeholder when it is not.
pub const TIFF_CHUNK_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Largest downsampling accumulator (sums of 8-bit samples, 4 bytes each). An analysis decode needs at most
/// about 1023 x 1023 x 5 x 4 bytes (20 MB); a 2048 px preview of an image just below 4096 px needs more.
pub const TIFF_ACCUMULATOR_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Image directories of the main chain considered for a reduced-resolution level (the first image included).
/// Equal to the probe walker's chain bound, so the scanner's directory reads cover every level looked at.
pub const TIFF_MAX_LEVELS: usize = crate::tiff::MAX_CHAIN;

/// Accumulator channels: CMYK plus alpha. Further extra samples are ignored.
const MAX_CHANNELS: usize = 5;

/// How a TIFF decode went: the directory used, the box factor and the memory it held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TiffDecodeStats {
    /// Index of the decoded directory in the main chain (0 = the first image).
    pub level: usize,
    /// Pixel dimensions of that directory.
    pub level_dims: (u32, u32),
    /// Box factor: every output pixel averages `factor x factor` source pixels.
    pub factor: u32,
    /// Strips or tiles decoded.
    pub chunks: u32,
    /// Largest compressed plus decoded size of one chunk.
    pub peak_chunk_bytes: usize,
    /// Size of the downsampling accumulator.
    pub accumulator_bytes: usize,
}

impl TiffDecodeStats {
    /// Upper bound of the working memory of the decode: one chunk plus the accumulator.
    pub fn peak_bytes(&self) -> usize {
        self.peak_chunk_bytes + self.accumulator_bytes
    }
}

/// The colour model of the decoded directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Model {
    Gray,
    Rgb,
    Cmyk,
    YCbCr,
}

/// Decode the first image of a TIFF (or its smallest reduced-resolution level whose long edge is at least
/// `min_long`) to the smallest integer box scale whose long edge is still at least `min_long`; unoriented.
pub fn decode<R: Read + Seek>(
    mut reader: R,
    min_long: u32,
) -> Result<(RgbImage, TiffDecodeStats), DecodeError> {
    contain(|| decode_inner(&mut reader, min_long))
        .unwrap_or_else(|| Err(DecodeError::Failed("TIFF decoder failure".into())))
}

fn rewind<R: Seek>(reader: &mut R) -> Result<(), DecodeError> {
    reader
        .seek(SeekFrom::Start(0))
        .map(|_| ())
        .map_err(|e| DecodeError::Failed(format!("TIFF: {e}")))
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.decoding_buffer_size = TIFF_CHUNK_MAX_BYTES;
    l.intermediate_buffer_size = TIFF_CHUNK_MAX_BYTES;
    l.ifd_value_size = 1024 * 1024;
    l
}

fn err(e: TiffError) -> DecodeError {
    match e {
        TiffError::UnsupportedError(u) => DecodeError::Unsupported(format!("TIFF: {u}")),
        TiffError::LimitsExceeded => DecodeError::TooLarge(format!(
            "a TIFF strip or tile exceeds {} MB",
            TIFF_CHUNK_MAX_BYTES / (1024 * 1024)
        )),
        other => DecodeError::Failed(format!("TIFF: {other}")),
    }
}

/// The directory to decode: the first image, or the smallest reduced-resolution level (NewSubfileType bit 0)
/// of the main chain with the first image's aspect ratio (within 1%) whose long edge reaches `min_long`.
fn choose_level<R: Read + Seek>(reader: &mut R, min_long: u32) -> Result<usize, DecodeError> {
    rewind(reader)?;
    let mut dec = Decoder::new(&mut *reader)
        .map_err(err)?
        .with_limits(limits());
    let (w0, h0) = dec.dimensions().map_err(err)?;
    if w0 == 0 || h0 == 0 {
        return Err(DecodeError::Failed("TIFF: zero dimensions".into()));
    }
    let aspect0 = f64::from(w0) / f64::from(h0);
    let mut best = (0usize, u64::from(w0) * u64::from(h0));
    let mut i = 0;
    while i + 1 < TIFF_MAX_LEVELS && dec.more_images() {
        if dec.next_image().is_err() {
            break;
        }
        i += 1;
        let Ok((w, h)) = dec.dimensions() else { break };
        let subfile = dec
            .find_tag_unsigned::<u32>(Tag::NewSubfileType)
            .ok()
            .flatten()
            .unwrap_or(0);
        if subfile & 1 == 0 || w == 0 || h == 0 || w.max(h) < min_long {
            continue;
        }
        let aspect = f64::from(w) / f64::from(h);
        if ((aspect - aspect0) / aspect0).abs() > 0.01 {
            continue;
        }
        let px = u64::from(w) * u64::from(h);
        if px < best.1 {
            best = (i, px);
        }
    }
    Ok(best.0)
}

fn decode_inner<R: Read + Seek>(
    reader: &mut R,
    min_long: u32,
) -> Result<(RgbImage, TiffDecodeStats), DecodeError> {
    let level = choose_level(reader, min_long)?;
    rewind(reader)?;
    let mut dec = Decoder::new(&mut *reader)
        .map_err(err)?
        .with_limits(limits());
    if level > 0 {
        dec.seek_to_image(level).map_err(err)?;
    }
    let (w, h) = dec.dimensions().map_err(err)?;
    if w == 0 || h == 0 {
        return Err(DecodeError::Failed("TIFF: zero dimensions".into()));
    }
    let tag_u16 = |dec: &mut Decoder<&mut R>, tag: Tag, default: u16| -> Result<u16, DecodeError> {
        Ok(dec
            .find_tag_unsigned::<u16>(tag)
            .map_err(err)?
            .unwrap_or(default))
    };
    let photometric = tag_u16(&mut dec, Tag::PhotometricInterpretation, u16::MAX)?;
    let spp = usize::from(tag_u16(&mut dec, Tag::SamplesPerPixel, 1)?.max(1));
    let planar = tag_u16(&mut dec, Tag::PlanarConfiguration, 1)? == 2;
    let bits = dec
        .find_tag_unsigned_vec::<u16>(Tag::BitsPerSample)
        .map_err(err)?
        .and_then(|v| v.first().copied())
        .unwrap_or(1);
    let extra = dec
        .find_tag_unsigned_vec::<u16>(Tag::ExtraSamples)
        .map_err(err)?
        .unwrap_or_default();
    let model = match (photometric, spp) {
        (0 | 1, _) => Model::Gray,
        (2, s) if s >= 3 => Model::Rgb,
        (5, s) if s >= 4 => Model::Cmyk,
        (6, s) if s >= 3 => Model::YCbCr,
        (p, s) => {
            return Err(DecodeError::Unsupported(format!(
                "TIFF photometric interpretation {p} with {s} samples per pixel"
            )));
        }
    };
    let colour = match model {
        Model::Gray => 1,
        Model::Rgb | Model::YCbCr => 3,
        Model::Cmyk => 4,
    };
    // The first extra sample is alpha when ExtraSamples says so (1 associated, 2 unassociated).
    let alpha = match extra.first() {
        Some(1) if spp > colour => Some(true),
        Some(2) if spp > colour => Some(false),
        _ => None,
    };
    let channels = spp.min(MAX_CHANNELS);

    let factor = (w.max(h) / min_long.max(1)).max(1);
    let (ow, oh) = ((w / factor).max(1), (h / factor).max(1));
    let acc_len = (ow as usize)
        .checked_mul(oh as usize)
        .and_then(|n| n.checked_mul(channels))
        .filter(|n| n.saturating_mul(4) <= TIFF_ACCUMULATOR_MAX_BYTES)
        .ok_or_else(|| {
            DecodeError::TooLarge(format!(
                "TIFF downsampled to {ow}x{oh} needs too much memory"
            ))
        })?;
    let mut acc = vec![0u32; acc_len];

    let tiles = dec.get_chunk_type() == ChunkType::Tile;
    let (cw, ch) = dec.chunk_dimensions();
    if cw == 0 || ch == 0 {
        return Err(DecodeError::Failed("TIFF: zero strip or tile size".into()));
    }
    let (offsets_tag, counts_tag) = if tiles {
        (Tag::TileOffsets, Tag::TileByteCounts)
    } else {
        (Tag::StripOffsets, Tag::StripByteCounts)
    };
    let offsets = dec.get_tag_u64_vec(offsets_tag).map_err(err)?;
    let counts = dec.get_tag_u64_vec(counts_tag).unwrap_or_default();
    let across = w.div_ceil(cw);
    let down = h.div_ceil(ch);
    let per_plane = if tiles { across * down } else { down };
    let planes = if planar { spp as u32 } else { 1 };
    let total = per_plane
        .checked_mul(planes)
        .ok_or_else(|| DecodeError::Failed("TIFF: chunk count overflows".into()))?;
    if (offsets.len() as u64) < u64::from(total) {
        return Err(DecodeError::Failed(format!(
            "TIFF: {} chunk offsets for {total} strips or tiles",
            offsets.len()
        )));
    }
    // File order, so a one-pass reader only ever moves forward.
    let mut order: Vec<u32> = (0..total).collect();
    order.sort_by_key(|&i| offsets[i as usize]);

    let chunk_samples = if planar { 1 } else { spp };
    let bytes_per_sample = usize::from(bits.div_ceil(8)).max(1);
    let mut stats = TiffDecodeStats {
        level,
        level_dims: (w, h),
        factor,
        accumulator_bytes: acc_len * 4,
        ..Default::default()
    };
    for index in order {
        let (dw, dh) = dec.chunk_data_dimensions(index);
        let decoded = (dw as usize)
            .saturating_mul(dh as usize)
            .saturating_mul(chunk_samples)
            .saturating_mul(bytes_per_sample);
        let compressed = counts
            .get(index as usize)
            .copied()
            .map_or(0, |c| usize::try_from(c).unwrap_or(usize::MAX));
        if decoded > TIFF_CHUNK_MAX_BYTES || compressed > TIFF_CHUNK_MAX_BYTES {
            return Err(DecodeError::TooLarge(format!(
                "a TIFF strip or tile of {dw}x{dh} exceeds {} MB",
                TIFF_CHUNK_MAX_BYTES / (1024 * 1024)
            )));
        }
        stats.peak_chunk_bytes = stats.peak_chunk_bytes.max(decoded + compressed);
        let data = dec.read_chunk(index).map_err(err)?;
        let (plane, x0, y0) = if tiles {
            let t = index % per_plane;
            (index / per_plane, (t % across) * cw, (t / across) * ch)
        } else {
            (index / per_plane, 0, (index % per_plane) * ch)
        };
        let region = Region {
            x0,
            y0,
            w: dw,
            h: dh,
            samples: chunk_samples,
            first_channel: if planar { plane as usize } else { 0 },
        };
        let out = Out {
            acc: &mut acc,
            w: ow,
            h: oh,
            channels,
            factor,
        };
        accumulate(&data, bits, region, out)?;
        stats.chunks += 1;
    }

    let image = finish(&acc, ow, oh, channels, factor, model, alpha);
    Ok((image, stats))
}

/// Where a decoded chunk lies and how its samples are laid out.
#[derive(Clone, Copy)]
struct Region {
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    /// Samples per pixel in the chunk (1 for planar chunks).
    samples: usize,
    /// Accumulator channel of the chunk's first sample.
    first_channel: usize,
}

struct Out<'a> {
    acc: &'a mut [u32],
    w: u32,
    h: u32,
    channels: usize,
    factor: u32,
}

fn accumulate(
    data: &DecodingResult,
    bits: u16,
    r: Region,
    out: Out<'_>,
) -> Result<(), DecodeError> {
    let bits = u32::from(bits);
    let scale = |max: f64| move |v: f64| ((v / max) * 255.0 + 0.5).clamp(0.0, 255.0) as u32;
    match data {
        DecodingResult::U8(v) if bits < 8 => {
            let unpacked = unpack_bits(v, bits, r.w as usize * r.samples, r.h as usize);
            let max = f64::from((1u32 << bits) - 1);
            sum(&unpacked, r, out, |x| scale(max)(f64::from(x)))
        }
        DecodingResult::U8(v) => sum(v, r, out, u32::from),
        DecodingResult::U16(v) => {
            let max = f64::from((1u32 << bits.clamp(9, 16)) - 1);
            sum(v, r, out, |x| scale(max)(f64::from(x)))
        }
        DecodingResult::U32(v) => {
            let shift = bits.clamp(17, 32) - 8;
            sum(v, r, out, |x| (x >> shift).min(255))
        }
        DecodingResult::U64(v) => sum(v, r, out, |x| (x >> 56) as u32),
        DecodingResult::I8(v) => sum(v, r, out, |x| (i32::from(x) + 128) as u32),
        DecodingResult::I16(v) => sum(v, r, out, |x| ((i32::from(x) + 32768) >> 8) as u32),
        DecodingResult::I32(v) => sum(v, r, out, |x| ((i64::from(x) + (1 << 31)) >> 24) as u32),
        DecodingResult::I64(v) => sum(v, r, out, |x| ((x >> 56) + 128) as u32),
        DecodingResult::F16(v) => sum(v, r, out, |x| unit(f64::from(x.to_f32()))),
        DecodingResult::F32(v) => sum(v, r, out, |x| unit(f64::from(x))),
        DecodingResult::F64(v) => sum(v, r, out, unit),
    }
}

/// A floating-point sample, 0.0 black to 1.0 white, to 8 bits (out-of-range values clip).
fn unit(v: f64) -> u32 {
    if v.is_nan() {
        return 0;
    }
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u32
}

/// Expand rows of 1, 2 or 4-bit samples (each row padded to a byte) to one byte per sample.
fn unpack_bits(v: &[u8], bits: u32, row_samples: usize, rows: usize) -> Vec<u8> {
    let row_bytes = (row_samples * bits as usize).div_ceil(8);
    let mask = (1u16 << bits) - 1;
    let mut out = Vec::with_capacity(row_samples * rows);
    for row in v.chunks(row_bytes.max(1)).take(rows) {
        for i in 0..row_samples {
            let bit = i * bits as usize;
            let byte = row.get(bit / 8).copied().unwrap_or(0);
            let shift = 8 - bits as usize - bit % 8;
            out.push(((u16::from(byte) >> shift) & mask) as u8);
        }
    }
    out
}

fn sum<T: Copy>(
    v: &[T],
    r: Region,
    out: Out<'_>,
    norm: impl Fn(T) -> u32,
) -> Result<(), DecodeError> {
    let row_len = r.w as usize * r.samples;
    if v.len() < row_len * r.h as usize {
        return Err(DecodeError::Failed("TIFF: short strip or tile".into()));
    }
    let Out {
        acc,
        w: ow,
        h: oh,
        channels,
        factor,
    } = out;
    let take = r.samples.min(channels.saturating_sub(r.first_channel));
    if take == 0 {
        return Ok(());
    }
    for y in 0..r.h {
        let oy = (r.y0 + y) / factor;
        if oy >= oh {
            break;
        }
        let row = &v[y as usize * row_len..(y as usize + 1) * row_len];
        let out_row = oy as usize * ow as usize;
        for x in 0..r.w {
            let ox = (r.x0 + x) / factor;
            if ox >= ow {
                break;
            }
            let o = (out_row + ox as usize) * channels + r.first_channel;
            let s = x as usize * r.samples;
            for c in 0..take {
                acc[o + c] += norm(row[s + c]);
            }
        }
    }
    Ok(())
}

/// Averages to 8-bit RGB: colour model conversion and alpha over white, per output pixel.
fn finish(
    acc: &[u32],
    w: u32,
    h: u32,
    channels: usize,
    factor: u32,
    model: Model,
    alpha: Option<bool>,
) -> RgbImage {
    let n = factor * factor;
    let avg = |s: u32| ((s + n / 2) / n).min(255);
    let colour = match model {
        Model::Gray => 1,
        Model::Rgb | Model::YCbCr => 3,
        Model::Cmyk => 4,
    };
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for px in acc.chunks_exact(channels) {
        let c = |i: usize| px.get(i).map_or(0, |s| avg(*s));
        let rgb = match model {
            Model::Gray => [c(0); 3],
            Model::Rgb => [c(0), c(1), c(2)],
            Model::Cmyk => {
                let k = 255 - c(3);
                [c(0), c(1), c(2)].map(|v| (255 - v) * k / 255)
            }
            Model::YCbCr => ycbcr_to_rgb(c(0), c(1), c(2)),
        };
        let rgb = match alpha {
            Some(associated) if channels > colour => {
                let a = c(colour);
                rgb.map(|v| {
                    let over = 255 * (255 - a);
                    if associated {
                        (v * 255 + over) / 255
                    } else {
                        (v * a + over) / 255
                    }
                    .min(255)
                })
            }
            _ => rgb,
        };
        data.extend(rgb.map(|v| v as u8));
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// Full-range YCbCr (JPEG/JFIF, and TIFF's default ReferenceBlackWhite) to RGB.
fn ycbcr_to_rgb(y: u32, cb: u32, cr: u32) -> [u32; 3] {
    let (y, cb, cr) = (f64::from(y), f64::from(cb) - 128.0, f64::from(cr) - 128.0);
    let clamp = |v: f64| (v + 0.5).clamp(0.0, 255.0) as u32;
    [
        clamp(y + 1.402 * cr),
        clamp(y - 0.344_136 * cb - 0.714_136 * cr),
        clamp(y + 1.772 * cb),
    ]
}
