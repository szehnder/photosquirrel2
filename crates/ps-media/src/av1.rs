//! AVIF stills through the bundled AV1 decoder, `rav1d` (ADR-0090): the container is read with [`crate::bmff`]
//! (primary `av01` item or a `grid` of them, `av1C`, `colr nclx`, `pixi`), each coded image is decoded by one
//! single-threaded rav1d decoder, and YUV is converted to 8-bit RGB here (nclx matrix and range, else the
//! sequence header's, else BT.601; bilinear chroma; 4:0:0 as grey; the identity matrix passed through).
//! Orientation (`irot`/`imir`) is applied by the caller from the probe, once.

use ps_model::{DecoderEntry, DecoderFormat};

use crate::bmff::{self, HeifInfo, HeifItem};
use crate::decode::{DecodeError, RgbImage};

/// Pixel cap of one AVIF (the same as other decoders: `MAX_DECODE_BYTES / 3`).
pub const MAX_PIXELS: u64 = 768 * 1024 * 1024 / 3;
/// Largest coded item read into memory.
const MAX_ITEM_BYTES: u64 = 512 * 1024 * 1024;
/// Most tiles of a grid.
const MAX_TILES: usize = 1024;

/// Built with rav1d's assembly (`avif-asm`, x86_64 only: rav1d 1.1's published aarch64 assembly does not build).
pub const ASSEMBLY: bool = cfg!(all(feature = "avif-asm", target_arch = "x86_64"));
/// rav1d's assembly exists for this architecture in this build (x86_64).
pub const ASSEMBLY_ARCH: bool = cfg!(target_arch = "x86_64");

/// The AVIF entry of a decoder report: always available.
pub fn report_entry() -> DecoderEntry {
    if ASSEMBLY_ARCH {
        report_entry_for(ASSEMBLY)
    } else {
        DecoderEntry {
            assembly: None,
            reason: None,
            ..report_entry_for(false)
        }
    }
}

/// [`report_entry`] of a build with or without rav1d's assembly.
pub fn report_entry_for(assembly: bool) -> DecoderEntry {
    DecoderEntry {
        format: DecoderFormat::Avif,
        available: true,
        backend: Some("bundled AV1 decoder (rav1d)".into()),
        version: Some("1.1".into()),
        reason: (!assembly).then(|| {
            "AVIF assembly is off (built without nasm): AVIF decoding is slower".to_owned()
        }),
        fix: None,
        assembly: Some(assembly),
    }
}

/// Decode an AVIF still (the whole file in `bytes`) to RGB, unoriented.
pub fn decode(bytes: &[u8]) -> Result<RgbImage, DecodeError> {
    crate::util::contain(|| decode_inner(bytes))
        .unwrap_or_else(|| Err(DecodeError::Failed("AV1 decoder failure".into())))
}

fn failed(m: impl Into<String>) -> DecodeError {
    DecodeError::Failed(m.into())
}

fn decode_inner(bytes: &[u8]) -> Result<RgbImage, DecodeError> {
    let Some(info) = bmff::parse_heif(bytes) else {
        return Err(if is_sequence(bytes) {
            DecodeError::Sequence
        } else {
            failed("AVIF meta box missing or unreadable")
        });
    };
    let Some(primary) = info.primary.and_then(|id| info.item(id)) else {
        return Err(if is_sequence(bytes) {
            DecodeError::Sequence
        } else {
            failed("AVIF without a primary item")
        });
    };
    match &primary.kind {
        b"av01" => {
            if let Some((w, h)) = primary.dims {
                check_size(w, h)?;
            }
            decode_item(bytes, &info, primary)
        }
        b"grid" => decode_grid(bytes, &info, primary),
        k => Err(DecodeError::Unsupported(format!(
            "AVIF primary item of type {}",
            String::from_utf8_lossy(k)
        ))),
    }
}

/// An `avis` file (an image sequence) without a still to show.
fn is_sequence(bytes: &[u8]) -> bool {
    bmff::ftyp_brands(bytes).is_some_and(|b| b.first() == Some(b"avis"))
}

fn check_size(w: u32, h: u32) -> Result<(), DecodeError> {
    if w == 0 || h == 0 {
        return Err(failed("zero AVIF dimensions"));
    }
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(DecodeError::TooLarge(format!("AVIF of {w}x{h}")));
    }
    Ok(())
}

fn decode_grid(bytes: &[u8], info: &HeifInfo, grid: &HeifItem) -> Result<RgbImage, DecodeError> {
    let body = info
        .item_data(bytes, grid, 64)
        .ok_or_else(|| failed("AVIF grid item data unreadable"))?;
    let g = bmff::parse_grid(&body).ok_or_else(|| failed("malformed AVIF grid"))?;
    check_size(g.width, g.height)?;
    let tiles = info.inputs(grid.id).unwrap_or(&[]);
    let n = (g.rows * g.columns) as usize;
    if tiles.len() != n || n > MAX_TILES {
        return Err(failed(format!(
            "AVIF grid of {}x{} with {} tiles",
            g.columns,
            g.rows,
            tiles.len()
        )));
    }
    let (w, h) = (g.width as usize, g.height as usize);
    let mut out = vec![0u8; w * h * 3];
    let (mut tw, mut th) = (0usize, 0usize);
    for (i, id) in tiles.iter().enumerate() {
        let item = info
            .item(*id)
            .filter(|t| &t.kind == b"av01")
            .ok_or_else(|| failed("AVIF grid tile is not an AV1 image"))?;
        let tile = decode_item(bytes, info, item)?;
        if i == 0 {
            (tw, th) = (tile.width as usize, tile.height as usize);
        } else if (tile.width as usize, tile.height as usize) != (tw, th) {
            return Err(failed("AVIF grid tiles of different sizes"));
        }
        let (r, c) = (i / g.columns as usize, i % g.columns as usize);
        let (x0, y0) = (c * tw, r * th);
        for y in 0..th {
            let oy = y0 + y;
            if oy >= h {
                break;
            }
            let cols = tw.min(w.saturating_sub(x0));
            if cols == 0 {
                break;
            }
            let src = &tile.data[y * tw * 3..y * tw * 3 + cols * 3];
            out[(oy * w + x0) * 3..(oy * w + x0 + cols) * 3].copy_from_slice(src);
        }
    }
    if tw * (g.columns as usize) < w || th * (g.rows as usize) < h {
        return Err(failed("AVIF grid tiles do not cover the image"));
    }
    Ok(RgbImage {
        width: g.width,
        height: g.height,
        data: out,
    })
}

/// Decode one `av01` item.
fn decode_item(bytes: &[u8], info: &HeifInfo, item: &HeifItem) -> Result<RgbImage, DecodeError> {
    let data = info
        .item_data(bytes, item, MAX_ITEM_BYTES)
        .ok_or_else(|| failed("AVIF item data unreadable"))?;
    if data.is_empty() {
        return Err(failed("empty AVIF item"));
    }
    let mut obus = item.av1c.clone().unwrap_or_default();
    obus.extend_from_slice(&data);
    check_obus(&obus)?;
    let frame = rav1d_frame(&obus)?;
    Ok(frame.to_rgb(item.nclx))
}

/// Check the OBU structure before rav1d sees it: rav1d's C entry points abort the process on an internal panic
/// (they cannot unwind), and some malformed streams reach one (a frame before any sequence header). Every OBU
/// must carry its size, sizes must tile the buffer exactly, the types must be known, and a sequence header must
/// come before the first frame.
pub fn check_obus(b: &[u8]) -> Result<(), DecodeError> {
    let bad = |m: &str| failed(format!("malformed AV1 data: {m}"));
    let mut p = 0usize;
    let mut seq = false;
    let mut reduced_still = false;
    let mut frame = false;
    while p < b.len() {
        let obu_start = p;
        let h = b[p];
        if h & 0x80 != 0 {
            return Err(bad("forbidden bit set"));
        }
        let kind = (h >> 3) & 0xF;
        let ext = h & 0x04 != 0;
        if h & 0x02 == 0 {
            return Err(bad("OBU without a size"));
        }
        p += 1 + usize::from(ext);
        // leb128, at most 8 bytes.
        let mut size: u64 = 0;
        let mut done = false;
        for i in 0..8 {
            let Some(&c) = b.get(p) else {
                return Err(bad("truncated OBU size"));
            };
            p += 1;
            size |= u64::from(c & 0x7F) << (7 * i);
            if c & 0x80 == 0 {
                done = true;
                break;
            }
        }
        if !done {
            return Err(bad("OBU size too long"));
        }
        let end = (p as u64)
            .checked_add(size)
            .filter(|e| *e <= b.len() as u64)
            .ok_or_else(|| bad("OBU past the end of the item"))?;
        match kind {
            1 => {
                let Some((reduced, max_px)) = sequence_header(&b[obu_start..end as usize]) else {
                    return Err(bad("unreadable sequence header"));
                };
                // No frame of this sequence can exceed the decoder's size limit: a picture allocation refused
                // for size takes rav1d into an error path that aborts the process.
                if max_px > MAX_PIXELS {
                    return Err(DecodeError::TooLarge(format!(
                        "AV1 sequence allowing frames of {max_px} pixels"
                    )));
                }
                seq = true;
                reduced_still = reduced;
            }
            3 | 6 | 7 => {
                if !seq {
                    return Err(bad("frame before the sequence header"));
                }
                if size == 0 {
                    return Err(bad("empty frame"));
                }
                // An AVIF still is one key frame (AV1 Image File Format 7.3.1): uncompressed_header starts
                // with show_existing_frame = 0 and frame_type = KEY_FRAME (0) unless the sequence header is
                // the reduced still-picture one, which implies both. Inter frames have no references here and
                // take rav1d into error paths that abort the process.
                if !reduced_still && b[p] & 0xE0 != 0 {
                    return Err(bad("not a key frame"));
                }
                frame = true;
            }
            4 if !frame => return Err(bad("tile group before a frame header")),
            2 | 4 | 5 | 15 => {}
            _ => return Err(bad("unexpected OBU type")),
        }
        p = end as usize;
    }
    if !frame {
        return Err(bad("no frame"));
    }
    Ok(())
}

/// Parse this sequence header OBU with rav1d (`dav1d_parse_sequence_header`): whether it uses the reduced
/// still-picture header, and the most pixels a frame of it can have (`2^width_bits x 2^height_bits`); `None`
/// when it does not parse.
#[allow(unsafe_code)]
fn sequence_header(obu: &[u8]) -> Option<(bool, u64)> {
    use rav1d::include::dav1d::headers::Dav1dSequenceHeader;
    use std::mem::MaybeUninit;
    use std::ptr::NonNull;
    let mut out = MaybeUninit::<Dav1dSequenceHeader>::uninit();
    // SAFETY: `out` is valid to write a `Dav1dSequenceHeader` to; `obu` is a live slice of `obu.len()` bytes.
    let r = unsafe {
        rav1d::src::lib::dav1d_parse_sequence_header(
            NonNull::new(out.as_mut_ptr()),
            NonNull::new(obu.as_ptr().cast_mut()),
            obu.len(),
        )
    };
    if r.0 != 0 {
        return None;
    }
    // SAFETY: a zero result means `out` was written.
    let h = unsafe { out.assume_init_ref() };
    let px = (1u64 << h.width_n_bits.min(32)) * (1u64 << h.height_n_bits.min(32));
    Some((h.reduced_still_picture_header != 0, px))
}

/// A decoded AV1 frame: planes as 16-bit samples.
struct Frame {
    width: usize,
    height: usize,
    bits: u32,
    /// 0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4.
    layout: u32,
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
    /// Chroma plane size.
    cw: usize,
    ch: usize,
    /// From the sequence header.
    matrix: u16,
    full_range: bool,
}

/// Kr and Kb of an H.273 matrix code (BT.601 when unknown).
fn coefficients(matrix: u16) -> (f32, f32) {
    match matrix {
        1 => (0.2126, 0.0722),
        4 => (0.30, 0.11),
        7 => (0.212, 0.087),
        9 | 10 => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    }
}

impl Frame {
    fn to_rgb(&self, nclx: Option<bmff::Nclx>) -> RgbImage {
        let (matrix, full) = match nclx {
            // 2: unspecified in the container; the stream decides.
            Some(n) if n.matrix != 2 => (n.matrix, n.full_range),
            Some(n) => (self.matrix, n.full_range),
            None => (self.matrix, self.full_range),
        };
        let max = ((1u32 << self.bits) - 1) as f32;
        let scale = (1u32 << (self.bits - 8)) as f32;
        let mid = (1u32 << (self.bits - 1)) as f32;
        let (kr, kb) = coefficients(matrix);
        let kg = 1.0 - kr - kb;
        let (w, h) = (self.width, self.height);
        let mut out = Vec::with_capacity(w * h * 3);
        let (sx, sy) = match self.layout {
            1 => (2.0f32, 2.0f32),
            2 => (2.0, 1.0),
            _ => (1.0, 1.0),
        };
        let to8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        for y in 0..h {
            for x in 0..w {
                let yy = f32::from(self.y[y * w + x]);
                let ny = if full {
                    yy / max
                } else {
                    (yy - 16.0 * scale) / (219.0 * scale)
                };
                if self.layout == 0 {
                    let g = to8(ny);
                    out.extend_from_slice(&[g, g, g]);
                    continue;
                }
                let (cu, cv) = self.chroma(x, y, sx, sy);
                let (nu, nv) = if full {
                    ((cu - mid) / max, (cv - mid) / max)
                } else {
                    (
                        (cu - 128.0 * scale) / (224.0 * scale),
                        (cv - 128.0 * scale) / (224.0 * scale),
                    )
                };
                if matrix == 0 {
                    // Identity (GBR): Y is G, U is B, V is R.
                    let (g, b, r) = if full {
                        (yy / max, cu / max, cv / max)
                    } else {
                        (ny, nu + 0.5, nv + 0.5)
                    };
                    out.extend_from_slice(&[to8(r), to8(g), to8(b)]);
                    continue;
                }
                let r = ny + 2.0 * (1.0 - kr) * nv;
                let b = ny + 2.0 * (1.0 - kb) * nu;
                let g = (ny - kr * r - kb * b) / kg;
                out.extend_from_slice(&[to8(r), to8(g), to8(b)]);
            }
        }
        RgbImage {
            width: w as u32,
            height: h as u32,
            data: out,
        }
    }

    /// Bilinear chroma (U, V) at luma position (x, y), chroma sited between luma samples.
    fn chroma(&self, x: usize, y: usize, sx: f32, sy: f32) -> (f32, f32) {
        let fx = ((x as f32 + 0.5) / sx - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) / sy - 0.5).max(0.0);
        let x0 = (fx as usize).min(self.cw - 1);
        let y0 = (fy as usize).min(self.ch - 1);
        let x1 = (x0 + 1).min(self.cw - 1);
        let y1 = (y0 + 1).min(self.ch - 1);
        let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
        let at = |p: &[u16], xx: usize, yy: usize| f32::from(p[yy * self.cw + xx]);
        let lerp = |p: &[u16]| {
            let top = at(p, x0, y0) * (1.0 - ax) + at(p, x1, y0) * ax;
            let bot = at(p, x0, y1) * (1.0 - ax) + at(p, x1, y1) * ax;
            top * (1.0 - ay) + bot * ay
        };
        (lerp(&self.u), lerp(&self.v))
    }
}

/// `-EAGAIN` as rav1d returns it.
const EAGAIN: i32 = -(libc::EAGAIN);

/// Decode one AV1 still from its OBUs with a fresh single-threaded rav1d decoder.
#[allow(unsafe_code)]
fn rav1d_frame(obus: &[u8]) -> Result<Frame, DecodeError> {
    use rav1d::include::dav1d::data::Dav1dData;
    use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
    use rav1d::include::dav1d::picture::Dav1dPicture;
    use rav1d::src::lib as d;
    use std::mem::MaybeUninit;
    use std::ptr::NonNull;

    let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
    // SAFETY: `dav1d_default_settings` writes a complete `Dav1dSettings` to the valid, writable pointer.
    let mut settings = unsafe {
        d::dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
        settings.assume_init()
    };
    // Two frame contexts: with one, rav1d 1.1 handles a tile decode error (corrupt data) through an error path
    // that panics inside its C entry point and so aborts the process; with frame threading the error is
    // returned. The analysis pool already parallelises across files, so two threads is the minimum, not a
    // speed-up (ADR-0090).
    settings.n_threads = 2;
    settings.max_frame_delay = 2;
    // No log lines on stderr for malformed files (the error is returned).
    // SAFETY: the logger is a cookie pointer and an optional callback; all-zero is "no logger" (None, None).
    unsafe { std::ptr::write_bytes(std::ptr::addr_of_mut!(settings.logger), 0, 1) };
    settings.frame_size_limit = MAX_PIXELS as u32;
    settings.all_layers = 0;
    let mut ctx: Option<Dav1dContext> = None;
    // SAFETY: both pointers are valid for the call; `ctx` receives the context or stays `None`.
    let r = unsafe {
        d::dav1d_open(
            Some(NonNull::from(&mut ctx)),
            Some(NonNull::from(&mut settings)),
        )
    };
    let Some(ctx) = ctx.filter(|_| r.0 == 0) else {
        return Err(failed(format!("AV1 decoder could not start ({})", r.0)));
    };
    /// Closes the context on every exit path.
    struct Ctx(Option<Dav1dContext>);
    impl Drop for Ctx {
        fn drop(&mut self) {
            // SAFETY: the context came from `dav1d_open` and is closed exactly once, here.
            unsafe { d::dav1d_close(Some(NonNull::from(&mut self.0))) };
        }
    }
    let guard = Ctx(Some(ctx));
    // `Dav1dContext` is a `Copy` handle (C callers pass the pointer by value); `guard` owns the close.
    let ctx = || guard.0;

    let mut data = Dav1dData::default();
    // SAFETY: `data` is a valid `Dav1dData` to write to; on success `ptr` points at `obus.len()` writable bytes
    // owned by rav1d (freed by its reference counting when the data is consumed or unreferenced).
    let ptr = unsafe { d::dav1d_data_create(Some(NonNull::from(&mut data)), obus.len()) };
    if ptr.is_null() {
        return Err(failed("AV1 decoder out of memory"));
    }
    // SAFETY: `ptr` is valid for `obus.len()` bytes and does not overlap `obus`.
    unsafe { std::ptr::copy_nonoverlapping(obus.as_ptr(), ptr, obus.len()) };
    /// Unreferences unconsumed data on every exit path.
    struct DataGuard(Dav1dData);
    impl Drop for DataGuard {
        fn drop(&mut self) {
            if self.0.sz > 0 {
                // SAFETY: the data came from `dav1d_data_create` and is still referenced.
                unsafe { d::dav1d_data_unref(Some(NonNull::from(&mut self.0))) };
            }
        }
    }
    let mut data = DataGuard(data);
    let mut pic = Dav1dPicture::default();
    let mut got = false;
    let mut drains = 0;
    for _ in 0..64 {
        if data.0.sz > 0 {
            // SAFETY: the context is open; `data` is valid to read and write.
            let r = unsafe { d::dav1d_send_data(ctx(), Some(NonNull::from(&mut data.0))) };
            if r.0 != 0 && r.0 != EAGAIN {
                return Err(failed(format!("AV1 data rejected ({})", r.0)));
            }
        }
        // SAFETY: the context is open; `pic` is valid to write.
        let r = unsafe { d::dav1d_get_picture(ctx(), Some(NonNull::from(&mut pic))) };
        if r.0 == 0 {
            got = true;
            break;
        }
        if r.0 != EAGAIN {
            return Err(failed(format!("AV1 decode failed ({})", r.0)));
        }
        if data.0.sz == 0 {
            // Everything sent: drain the frame threads (each call waits for one), then give up.
            drains += 1;
            if drains > 4 {
                break;
            }
        }
    }
    if !got {
        return Err(failed("no AV1 frame in the AVIF item"));
    }
    /// Unreferences the picture on every exit path.
    struct PicGuard(Dav1dPicture);
    impl Drop for PicGuard {
        fn drop(&mut self) {
            // SAFETY: the picture came from `dav1d_get_picture` and is unreferenced once.
            unsafe { d::dav1d_picture_unref(Some(NonNull::from(&mut self.0))) };
        }
    }
    let pic = PicGuard(pic);
    let p = &pic.0;
    let (w, h) = (p.p.w, p.p.h);
    let bits = u32::try_from(p.p.bpc).unwrap_or(0);
    if w <= 0 || h <= 0 || !matches!(bits, 8 | 10 | 12) {
        return Err(failed("AV1 frame of unexpected size or depth"));
    }
    let (w, h) = (w as usize, h as usize);
    let layout = p.p.layout;
    let (cw, ch) = match layout {
        1 => (w.div_ceil(2), h.div_ceil(2)),
        2 => (w.div_ceil(2), h),
        3 => (w, h),
        _ => (0, 0),
    };
    let (matrix, full_range) = match p.seq_hdr {
        // SAFETY: the sequence header lives as long as the picture reference held by `pic`.
        Some(s) => unsafe {
            let s = s.as_ref();
            (s.mtrx as u16, s.color_range != 0)
        },
        None => (2, false),
    };
    let read =
        |plane: usize, pw: usize, ph: usize, stride: isize| -> Result<Vec<u16>, DecodeError> {
            let base = p.data[plane]
                .ok_or_else(|| failed("AV1 frame without plane data"))?
                .as_ptr()
                .cast::<u8>();
            let mut out = Vec::with_capacity(pw * ph);
            for row in 0..ph {
                // SAFETY: rav1d guarantees `ph` rows of `stride` bytes per plane, each holding `pw` samples of
                // `bits` > 8 ? 2 : 1 bytes, valid while the picture is referenced.
                unsafe {
                    let r = base.offset(row as isize * stride);
                    if bits > 8 {
                        let r = r.cast::<u16>();
                        for x in 0..pw {
                            out.push(r.add(x).read_unaligned());
                        }
                    } else {
                        for x in 0..pw {
                            out.push(u16::from(*r.add(x)));
                        }
                    }
                }
            }
            Ok(out)
        };
    let y = read(0, w, h, p.stride[0])?;
    let (u, v) = if layout == 0 {
        (Vec::new(), Vec::new())
    } else {
        (read(1, cw, ch, p.stride[1])?, read(2, cw, ch, p.stride[1])?)
    };
    drop(pic);
    drop(data);
    drop(guard);
    Ok(Frame {
        width: w,
        height: h,
        bits,
        layout,
        y,
        u,
        v,
        cw,
        ch,
        matrix,
        full_range,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What doctor shows for a build without `avif-asm` (task 8.2).
    #[test]
    fn assembly_off_is_reported() {
        let off = report_entry_for(false);
        assert!(off.available);
        assert_eq!(
            off.reason.as_deref(),
            Some("AVIF assembly is off (built without nasm): AVIF decoding is slower")
        );
        assert_eq!(
            off.detail(),
            "bundled AV1 decoder (rav1d) (AVIF assembly is off (built without nasm): AVIF decoding is slower)"
        );
        let on = report_entry_for(true);
        assert_eq!(on.reason, None);
        assert_eq!(on.assembly, Some(true));
    }
}
