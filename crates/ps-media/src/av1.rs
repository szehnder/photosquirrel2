//! AVIF stills through the bundled AV1 decoder, `rav1d` (ADR-0090): the container is read with [`crate::bmff`]
//! (primary `av01` item or a `grid` of them, `av1C`, `colr nclx`, `pixi`), each coded image is decoded by a fresh
//! rav1d decoder with two frame contexts (ADR-0094), and YUV is converted to 8-bit RGB here, read in place from
//! rav1d's picture (nclx matrix and range, else the
//! sequence header's, else BT.601; bilinear chroma; 4:0:0 as grey; the identity matrix passed through).
//! Orientation (`irot`/`imir`) is applied by the caller from the probe, once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ps_model::{DecoderEntry, DecoderFormat};

use crate::bmff::{self, HeifInfo, HeifItem};
use crate::decode::{DecodeError, RgbImage};

/// Pixel cap of one AVIF (the same as other decoders: `MAX_DECODE_BYTES / 3`).
pub const MAX_PIXELS: u64 = 768 * 1024 * 1024 / 3;
/// Largest coded item read into memory.
const MAX_ITEM_BYTES: u64 = 512 * 1024 * 1024;
/// Most tiles of a grid.
const MAX_TILES: usize = 1024;

/// The AVIF backend as decode-failure records name it (ADR-0098): a new rav1d or wrapper behaviour gets a new
/// name, so files that failed with the old one are decoded again.
pub const BACKEND: &str = "rav1d 1.1 (wrapper 2)";

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

/// The longest one AVIF decode may take before it is abandoned (ADR-0094). A 12 MP AVIF decodes in about half
/// a second on the reference machine; the largest allowed (268 MP) in well under this.
pub const DECODE_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the caller of a decode checks its stop function while it waits (ADR-0097).
pub const POLL: Duration = Duration::from_millis(50);
/// Abandoned decodes still running (their threads cannot be stopped): while [`MAX_ABANDONED`] are alive, AVIF
/// decoding is refused until one finishes, so a run of hanging files cannot pile up threads and memory.
pub const MAX_ABANDONED: u64 = 2;
/// Most AVIF decode threads alive at once, in flight and abandoned together (ADR-0097).
pub const MAX_LIVE: u64 = 64;

/// AVIF decode threads alive: in flight in the low 32 bits, abandoned in the high 32. Admission tests and counts in
/// one compare-exchange ([`admit`]), so concurrent callers cannot all pass a check made before any of them counted.
static DECODES: AtomicU64 = AtomicU64::new(0);
const ONE_ABANDONED: u64 = 1 << 32;

/// AVIF decode threads alive now: (in flight, abandoned).
pub fn live_decodes() -> (u64, u64) {
    let v = DECODES.load(Ordering::Acquire);
    (v & 0xffff_ffff, v >> 32)
}

/// Count a new decode thread in, or refuse it: while [`MAX_ABANDONED`] abandoned decodes are alive, or
/// [`MAX_LIVE`] threads are. A refusal is not the file's doing ([`DecodeError::Refused`]): it is retried on a later
/// scan without using up one of its attempts (ADR-0100).
fn admit() -> Result<(), DecodeError> {
    let mut cur = DECODES.load(Ordering::Acquire);
    loop {
        let (in_flight, abandoned) = (cur & 0xffff_ffff, cur >> 32);
        if abandoned >= MAX_ABANDONED {
            return Err(DecodeError::Refused(
                "AV1 decoding is paused: earlier AVIF decodes did not finish".into(),
            ));
        }
        if in_flight + abandoned >= MAX_LIVE {
            return Err(DecodeError::Refused("too many AV1 decodes at once".into()));
        }
        match DECODES.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(()),
            Err(now) => cur = now,
        }
    }
}

/// Something a decode keeps until its decode thread really ends (ADR-0097): the scanner's decode-budget permit,
/// so the memory of an abandoned decode stays reserved while its thread runs.
pub trait DecodeHold: Send + Sync {
    /// The caller stopped waiting (timeout or cancel); the thread still runs and keeps this hold until it ends.
    fn abandoned(&self) {}
}

/// How the caller waits for a decode running on its own thread.
#[derive(Clone)]
pub struct Wait<'a> {
    /// Polled every [`POLL`] while waiting; `true` abandons the decode and returns [`DecodeError::Cancelled`].
    /// Time spent inside it (a paused scan parks there) does not count towards the timeout.
    pub stop: &'a dyn Fn() -> bool,
    /// Moved into the decode thread and dropped when it ends.
    pub hold: Option<Arc<dyn DecodeHold>>,
}

impl Wait<'_> {
    /// Never stops, holds nothing.
    pub fn none() -> Wait<'static> {
        Wait {
            stop: &|| false,
            hold: None,
        }
    }
}

/// Decode an AVIF still (the whole file in `bytes`) to RGB, unoriented. Runs on its own thread under a watchdog
/// ([`DECODE_TIMEOUT`]): rav1d's frame threads wait on each other, and a decode that never returns must not
/// stall a scan worker forever (ADR-0094).
pub fn decode(bytes: &[u8]) -> Result<RgbImage, DecodeError> {
    decode_waiting(bytes, Wait::none())
}

/// [`decode`] with the caller's stop check and hold (ADR-0097).
pub fn decode_waiting(bytes: &[u8], wait: Wait<'_>) -> Result<RgbImage, DecodeError> {
    // Copied for the decode thread, which may outlive the caller's buffer; the scanner's estimate counts it.
    let owned = bytes.to_vec();
    with_watchdog(hooks::timeout(), wait, move || {
        hooks::wait_while_hung();
        crate::util::contain(|| decode_inner(&owned))
            .unwrap_or_else(|| Err(DecodeError::Failed("AV1 decoder failure".into())))
    })
}

/// Run `f` on a new thread and wait for it at most `timeout` of waiting time, checking `wait.stop` every [`POLL`].
/// A decode that does not finish in time, or whose caller is stopped, is abandoned: its thread keeps running
/// detached with `wait.hold`, counted as abandoned until it ends, and the caller gets a transient failure
/// ([`DecodeError::Interrupted`]) or [`DecodeError::Cancelled`].
pub fn with_watchdog(
    timeout: Duration,
    wait: Wait<'_>,
    f: impl FnOnce() -> Result<RgbImage, DecodeError> + Send + 'static,
) -> Result<RgbImage, DecodeError> {
    admit()?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // (abandoned by the caller, done). Set under the lock so the thread and the caller agree on who accounts for
    // an abandoned decode.
    let state = Arc::new(Mutex::new((false, false)));
    let st = state.clone();
    let hold = wait.hold.clone();
    let spawned = std::thread::Builder::new()
        .name("ps-av1-decode".into())
        .spawn(move || {
            let r = f();
            let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
            g.1 = true;
            if g.0 {
                DECODES.fetch_sub(ONE_ABANDONED, Ordering::AcqRel);
            } else {
                DECODES.fetch_sub(1, Ordering::AcqRel);
                let _ = tx.send(r);
            }
            drop(g);
            // Released only now that the decode's memory is gone.
            drop(hold);
        });
    if let Err(e) = spawned {
        DECODES.fetch_sub(1, Ordering::AcqRel);
        return Err(DecodeError::Refused(format!(
            "AV1 decoder thread could not start ({e})"
        )));
    }
    let mut waited = Duration::ZERO;
    loop {
        let slice = POLL
            .min(timeout.saturating_sub(waited))
            .max(Duration::from_millis(1));
        let t = Instant::now();
        match rx.recv_timeout(slice) {
            Ok(r) => return r,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(failed("AV1 decoder thread ended without a result"));
            }
            Err(RecvTimeoutError::Timeout) => waited += t.elapsed(),
        }
        let stopped = (wait.stop)();
        if !stopped && waited < timeout {
            continue;
        }
        let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
        if g.1 {
            // Finished just now: its result is in the channel.
            drop(g);
            return rx
                .recv()
                .unwrap_or_else(|_| Err(failed("AV1 decoder thread ended without a result")));
        }
        g.0 = true;
        // In flight -> abandoned (in flight is at least this decode's 1).
        DECODES.fetch_add(ONE_ABANDONED - 1, Ordering::AcqRel);
        drop(g);
        if let Some(h) = &wait.hold {
            h.abandoned();
        }
        return Err(if stopped {
            DecodeError::Cancelled
        } else {
            DecodeError::Interrupted(format!(
                "AV1 decoder did not finish within {} s; abandoned",
                timeout.as_secs()
            ))
        });
    }
}

/// Test-only hooks (`test-support`, dev-dependencies only): make AVIF decodes hang until released, to test the
/// watchdog, cancellation and the abandoned-decode limit with real files (ADR-0097).
pub mod hooks {
    #[cfg(any(test, feature = "test-support"))]
    static HANG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Every AVIF decode started from now on waits (before decoding) until [`release`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn hang() {
        HANG.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Hanging decodes continue.
    #[cfg(any(test, feature = "test-support"))]
    pub fn release() {
        HANG.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    static TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Abandon AVIF decodes after `t` instead of [`super::DECODE_TIMEOUT`] (`None`: the default again).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_timeout(t: Option<std::time::Duration>) {
        TIMEOUT_MS.store(
            t.map_or(0, |t| t.as_millis().max(1) as u64),
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    pub(super) fn timeout() -> std::time::Duration {
        #[cfg(any(test, feature = "test-support"))]
        {
            let ms = TIMEOUT_MS.load(std::sync::atomic::Ordering::SeqCst);
            if ms > 0 {
                return std::time::Duration::from_millis(ms);
            }
        }
        super::DECODE_TIMEOUT
    }

    pub(super) fn wait_while_hung() {
        #[cfg(any(test, feature = "test-support"))]
        while HANG.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
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
    rav1d_decode(&obus, item.nclx)
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
/// still-picture header, and the most pixels a frame of it can have (`max_frame_width x max_frame_height`: the
/// AV1 specification bounds every frame of the sequence by them, and the bit widths of those fields only say
/// how they are coded); `None` when it does not parse. A frame header that overrides its size beyond the
/// decoder's limit is refused by rav1d's own `frame_size_limit` check while parsing (an `ERANGE` return).
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
    let px = u64::try_from(h.max_width).unwrap_or(0) * u64::try_from(h.max_height).unwrap_or(0);
    Some((h.reduced_still_picture_header != 0, px))
}

/// A decoded AV1 frame, read in place from rav1d's picture: no copy of the planes is made, so a decode holds
/// rav1d's picture and the RGB output only (ADR-0094).
struct Frame {
    width: usize,
    height: usize,
    bits: u32,
    /// 0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4.
    layout: u32,
    /// Plane base pointers (Y, U, V) and their row strides in bytes.
    planes: [*const u8; 3],
    strides: [isize; 3],
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
                let yy = self.sample(0, x, y);
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

    /// One sample of `plane` at (x, y), inside the plane's size.
    #[allow(unsafe_code)]
    fn sample(&self, plane: usize, x: usize, y: usize) -> f32 {
        // SAFETY: `planes` and `strides` come from a picture rav1d returned and that is referenced for the
        // frame's whole life; (x, y) is inside the plane (luma: width x height; chroma: cw x ch), and rav1d
        // guarantees each row holds that many samples of 1 (8-bit) or 2 (higher depths) bytes.
        unsafe {
            let row = self.planes[plane].offset(y as isize * self.strides[plane]);
            if self.bits > 8 {
                f32::from(row.cast::<u16>().add(x).read_unaligned())
            } else {
                f32::from(*row.add(x))
            }
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
        let lerp = |p: usize| {
            let top = self.sample(p, x0, y0) * (1.0 - ax) + self.sample(p, x1, y0) * ax;
            let bot = self.sample(p, x0, y1) * (1.0 - ax) + self.sample(p, x1, y1) * ax;
            top * (1.0 - ay) + bot * ay
        };
        (lerp(1), lerp(2))
    }
}

/// `-EAGAIN` as rav1d returns it.
const EAGAIN: i32 = -(libc::EAGAIN);

/// Decode one AV1 still from its OBUs with a fresh rav1d decoder (two frame contexts, ADR-0094) and convert it
/// to RGB straight from rav1d's picture.
#[allow(unsafe_code)]
fn rav1d_decode(obus: &[u8], nclx: Option<bmff::Nclx>) -> Result<RgbImage, DecodeError> {
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
    let plane = |i: usize| -> Result<*const u8, DecodeError> {
        Ok(p.data[i]
            .ok_or_else(|| failed("AV1 frame without plane data"))?
            .as_ptr()
            .cast::<u8>()
            .cast_const())
    };
    let y = plane(0)?;
    let (u, v) = if layout == 0 {
        (y, y)
    } else {
        (plane(1)?, plane(2)?)
    };
    let frame = Frame {
        width: w,
        height: h,
        bits,
        layout,
        planes: [y, u, v],
        // rav1d's chroma planes share one stride.
        strides: [p.stride[0], p.stride[1], p.stride[1]],
        cw,
        ch,
        matrix,
        full_range,
    };
    // Converted while `pic` holds the picture.
    let rgb = frame.to_rgb(nclx);
    drop(pic);
    drop(data);
    drop(guard);
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The watchdog tests share the process-wide decode counts: one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn img(w: u32) -> RgbImage {
        RgbImage {
            width: w,
            height: 1,
            data: vec![0; 3 * w as usize],
        }
    }

    /// Wait (bounded) until no decode thread is alive.
    fn settle() {
        let t = Instant::now();
        while live_decodes() != (0, 0) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(live_decodes(), (0, 0));
    }

    /// A decode that blocks until `rx` receives (or its sender is dropped).
    fn blocked(
        rx: std::sync::mpsc::Receiver<()>,
    ) -> impl FnOnce() -> Result<RgbImage, DecodeError> {
        move || {
            let _ = rx.recv();
            Ok(img(1))
        }
    }

    /// A hold that records its drop and its abandonment.
    struct Probe {
        dropped: Arc<std::sync::atomic::AtomicBool>,
        abandoned: Arc<std::sync::atomic::AtomicBool>,
    }

    impl DecodeHold for Probe {
        fn abandoned(&self) {
            self.abandoned.store(true, Ordering::SeqCst);
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    /// A decode that does not return is abandoned after the timeout and reported as a transient failure, and its
    /// thread is counted until it ends (ADR-0094, ADR-0097).
    #[test]
    fn scenario_decoder_support_hanging_avif_decode_is_abandoned() {
        let _s = serial();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let t = Instant::now();
        let r = with_watchdog(
            Duration::from_millis(100),
            Wait::none(),
            blocked(release_rx),
        );
        assert!(t.elapsed() < Duration::from_secs(5));
        match r {
            Err(DecodeError::Interrupted(m)) => assert!(m.contains("did not finish"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(live_decodes(), (0, 1));
        release_tx.send(()).unwrap();
        // The abandoned thread ends and is no longer counted.
        settle();
        // A decode that finishes in time returns its result.
        let ok = with_watchdog(Duration::from_secs(5), Wait::none(), || Ok(img(2))).unwrap();
        assert_eq!(ok.width, 2);
        assert_eq!(live_decodes(), (0, 0));
    }

    /// At [`MAX_ABANDONED`] abandoned decodes alive, further AVIF decodes are refused (transiently, so the file is
    /// retried later) and nothing is started; once one ends, decoding resumes (ADR-0094, ADR-0097).
    #[test]
    fn scenario_decoder_support_abandoned_avif_decodes_are_capped() {
        let _s = serial();
        let mut releases = Vec::new();
        for i in 0..MAX_ABANDONED {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            releases.push(tx);
            let r = with_watchdog(Duration::from_millis(20), Wait::none(), blocked(rx));
            assert!(matches!(r, Err(DecodeError::Interrupted(_))), "{r:?}");
            assert_eq!(live_decodes(), (0, i + 1));
        }
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran2 = ran.clone();
        let r = with_watchdog(Duration::from_secs(5), Wait::none(), move || {
            ran2.store(true, Ordering::SeqCst);
            Ok(img(1))
        });
        match r {
            Err(DecodeError::Refused(m)) => assert!(m.contains("paused"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(
            !ran.load(Ordering::SeqCst),
            "a refused decode does not start"
        );
        assert_eq!(live_decodes(), (0, MAX_ABANDONED));
        // Many callers at once while the limit holds: every one refused, none counted.
        let refused: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    with_watchdog(Duration::from_secs(5), Wait::none(), || Ok(img(1)))
                })
            })
            .collect();
        for t in refused {
            assert!(matches!(t.join().unwrap(), Err(DecodeError::Refused(_))));
        }
        assert_eq!(live_decodes(), (0, MAX_ABANDONED));
        releases.pop().unwrap().send(()).unwrap();
        let t = Instant::now();
        while live_decodes().1 >= MAX_ABANDONED && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            with_watchdog(Duration::from_secs(5), Wait::none(), || Ok(img(3)))
                .unwrap()
                .width,
            3
        );
        drop(releases);
        settle();
    }

    /// A stop request reaches a hanging decode within a poll slice: the caller gets `Cancelled` at once, the thread
    /// is abandoned, and the hold (the decode budget permit) is released only when the thread really ends
    /// (ADR-0097).
    #[test]
    fn scenario_decoder_support_cancel_stops_waiting_for_a_hung_avif_decode() {
        let _s = serial();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let abandoned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hold: Arc<dyn DecodeHold> = Arc::new(Probe {
            dropped: dropped.clone(),
            abandoned: abandoned.clone(),
        });
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let st = stop.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            st.store(true, Ordering::SeqCst);
            Instant::now()
        });
        let check = || stop.load(Ordering::SeqCst);
        let r = with_watchdog(
            DECODE_TIMEOUT,
            Wait {
                stop: &check,
                hold: Some(hold),
            },
            blocked(rx),
        );
        let latency = setter.join().unwrap().elapsed();
        assert!(matches!(r, Err(DecodeError::Cancelled)), "{r:?}");
        assert!(latency < Duration::from_secs(2), "{latency:?}");
        assert!(abandoned.load(Ordering::SeqCst));
        assert!(
            !dropped.load(Ordering::SeqCst),
            "held while the thread runs"
        );
        assert_eq!(live_decodes(), (0, 1));
        tx.send(()).unwrap();
        settle();
        let t = Instant::now();
        while !dropped.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(dropped.load(Ordering::SeqCst));
    }

    /// Time the caller spends inside its stop check (a paused scan parks there) does not count towards the
    /// timeout: a decode finishing after a long pause still returns its result.
    #[test]
    fn a_pause_does_not_count_towards_the_timeout() {
        let _s = serial();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let paused_once = std::sync::atomic::AtomicBool::new(false);
        let tx = std::sync::Mutex::new(Some(tx));
        let check = || {
            if !paused_once.swap(true, Ordering::SeqCst) {
                // Parked for longer than the timeout; the decode finishes meanwhile.
                std::thread::sleep(Duration::from_millis(400));
                drop(tx.lock().unwrap().take());
            }
            false
        };
        let r = with_watchdog(
            Duration::from_millis(300),
            Wait {
                stop: &check,
                hold: None,
            },
            blocked(rx),
        );
        assert_eq!(r.unwrap().width, 1);
        settle();
    }

    /// A sequence header whose size fields are coded with more bits than its maximum frame size needs (any
    /// encoder may write 16 bits) is bounded by that maximum frame size, not by 2^bits (gate review fix 8).
    #[test]
    fn scenario_decoder_support_avif_size_fields_wider_than_needed_are_accepted() {
        // 64x48 coded with 16-bit fields: 2^16 x 2^16 is far over the cap, 64 x 48 is not.
        let seq = sequence_header_obu(16, 64, 48);
        let (reduced, px) = sequence_header(&seq).expect("parses");
        assert!(reduced);
        assert_eq!(px, 64 * 48);
        let mut obus = seq.clone();
        // A frame OBU with a key-frame first byte (reduced still header: no frame type bits needed).
        obus.extend_from_slice(&[(6 << 3) | 0x02, 1, 0x10]);
        assert!(check_obus(&obus).is_ok(), "{:?}", check_obus(&obus));
        // A maximum frame size over the cap is refused before rav1d decodes anything.
        let huge = sequence_header_obu(16, 65_000, 65_000);
        let mut obus = huge;
        obus.extend_from_slice(&[(6 << 3) | 0x02, 1, 0x10]);
        assert!(matches!(check_obus(&obus), Err(DecodeError::TooLarge(_))));
    }

    /// A reduced still-picture sequence header OBU (AV1 5.5), 8-bit 4:2:0, with `bits`-bit size fields.
    fn sequence_header_obu(bits: u32, w: u32, h: u32) -> Vec<u8> {
        let mut b = Bits::default();
        b.put(0, 3); // seq_profile
        b.put(1, 1); // still_picture
        b.put(1, 1); // reduced_still_picture_header
        b.put(0, 5); // seq_level_idx[0]
        b.put(bits - 1, 4); // frame_width_bits_minus_1
        b.put(bits - 1, 4); // frame_height_bits_minus_1
        b.put(w - 1, bits); // max_frame_width_minus_1
        b.put(h - 1, bits); // max_frame_height_minus_1
        b.put(0, 1); // use_128x128_superblock
        b.put(0, 1); // enable_filter_intra
        b.put(0, 1); // enable_intra_edge_filter
        b.put(0, 1); // enable_superres
        b.put(0, 1); // enable_cdef
        b.put(0, 1); // enable_restoration
        b.put(0, 1); // high_bitdepth
        b.put(0, 1); // mono_chrome
        b.put(0, 1); // color_description_present_flag
        b.put(0, 1); // color_range
        b.put(0, 2); // chroma_sample_position
        b.put(0, 1); // separate_uv_delta_q
        b.put(0, 1); // film_grain_params_present
        b.put(1, 1); // trailing one bit
        let payload = b.finish();
        let mut obu = vec![(1 << 3) | 0x02, payload.len() as u8];
        obu.extend_from_slice(&payload);
        obu
    }

    #[derive(Default)]
    struct Bits {
        out: Vec<u8>,
        n: u32,
    }

    impl Bits {
        fn put(&mut self, v: u32, bits: u32) {
            for i in (0..bits).rev() {
                if self.n.is_multiple_of(8) {
                    self.out.push(0);
                }
                if (v >> i) & 1 == 1 {
                    let last = self.out.len() - 1;
                    self.out[last] |= 0x80 >> (self.n % 8);
                }
                self.n += 1;
            }
        }

        fn finish(self) -> Vec<u8> {
            self.out
        }
    }

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
