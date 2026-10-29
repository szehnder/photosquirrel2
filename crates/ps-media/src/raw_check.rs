//! Evidence that a RAW file was cut short (`damaged-raw-reason`, ADR-0115, ADR-0117), judged by our own code from the
//! file's structure: a directory, strip, tile, embedded JPEG or media box that the file records but that ends past
//! its end. rawler's errors cannot tell a cut file from a parse quirk, and when the embedded preview decodes rawler
//! is never asked, so a cut sensor strip behind an intact preview would never be seen.
//!
//! What counts, and only that (a false negative is the priority-1 bug, a false positive only mislabels a file that
//! is still catalogued and exported as is):
//! - TIFF-based RAWs (CR2, NEF, ARW, DNG, ORF, RW2, PEF, ...): IFD0's chain, SubIFDs (0x014A) and the Exif IFD
//!   pointer (0x8769) past the end, or a directory whose entries run past it (its trailing 4-byte next pointer may
//!   be missing at the very end: some writers end the file with the last table); `StripOffsets`/`StripByteCounts`,
//!   `TileOffsets`/`TileByteCounts` and `JPEGInterchangeFormat`/`Length` ranges (both tags present, a count of 0
//!   ignored) that end past it. Maker notes and the values of every other tag are never evidence (real cameras
//!   write private tags with odd counts).
//! - CR3: a top-level box whose declared size ends past the file (a `size = 0` box runs to the end: not
//!   evidence), and the samples of each `moov` track (`stco`/`co64` chunk offsets with their `stsz` sizes), so a
//!   file cut exactly at a box boundary, before or inside `mdat`, is found too.
//! - RW2 (and Leica's RWL) with Panasonic's "RAW 1" compression and a fixed packing (`RawFormat` 1-7 or none): the
//!   sensor data from `RawDataOffset`, whose least size follows from the sensor's width and height (the file records
//!   no length); `RawFormat` 8 (variable-size Huffman strips, GH6 and later): each strip its tables record.
//! - RAF: the header's JPEG, CFA header and CFA ranges.
//!
//! Other RAW formats (MRW, X3F, CRW) and layouts (other Panasonic compressions and `RawFormat` values, the Sony
//! DSLR-A100's ARW, whose sensor data is located through Minolta's private records) get no check.
//!
//! [`truncation`] judges the whole file in memory (the analysis). [`check_by_reads`] judges a file from its first
//! bytes plus bounded targeted reads of its directories (TIFF IFDs, CR3 box headers and `moov`), the one-time
//! re-check of rows an earlier version catalogued (ADR-0117) and the cleanup's pre-move check: a few kilobytes per
//! file, never a full read unless a bound stops the targeted reads.

use ps_model::{Format, ProbeRecord, RawFormat};

use crate::tiff::{IfdKind, IfdView, Segment, Tiff, View, be_u32, be_u64, u16_at, u32_at};

/// A RAW file cut short: it ends at `file_size` but its `what` needs bytes up to `needed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Truncation {
    pub file_size: u64,
    pub needed: u64,
    /// "image data", "preview" or "directory".
    pub what: &'static str,
}

impl Truncation {
    /// "the file ends at 3.1 MB but its image data runs to 24.6 MB: it was cut short (a card removed or a copy
    /// stopped while writing)".
    pub fn message(&self) -> String {
        let (mut end, mut need) = (size_text(self.file_size), size_text(self.needed));
        if end == need {
            end = format!("{} bytes", self.file_size);
            need = format!("{} bytes", self.needed);
        }
        format!(
            "the file ends at {end} but its {} runs to {need}: it was cut short (a card removed or a copy stopped \
             while writing)",
            self.what
        )
    }
}

/// "24.6 MB", "812.3 KB", "512 bytes" (decimal units).
pub fn size_text(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1} KB", n as f64 / 1e3)
    } else {
        format!("{n} bytes")
    }
}

/// What a structure check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Structure {
    /// Every range the check looks at lies inside the file (or the format has no check).
    Complete,
    /// Cut short.
    Cut(Truncation),
    /// A range the check needs was not read (a bound of the targeted reads stopped it) and nothing found so far is
    /// evidence.
    Unknown,
}

impl Structure {
    pub fn cut(&self) -> Option<&Truncation> {
        match self {
            Structure::Cut(t) => Some(t),
            _ => None,
        }
    }
}

/// The evidence that `bytes`, the whole RAW file `probe` describes, was cut short; `None` when there is none, when
/// `bytes` is not the whole file (a partial buffer is never judged) or for a format without a check.
pub fn truncation(bytes: &[u8], probe: &ProbeRecord) -> Option<Truncation> {
    let file_size = bytes.len() as u64;
    if file_size != probe.file_size || !ps_model::frame::structure_check_enabled() {
        return None;
    }
    let Some(Format::Raw(raw)) = probe.format else {
        return None;
    };
    match judge(raw, View::new(bytes, &[]), file_size) {
        Structure::Cut(t) => Some(t),
        Structure::Complete | Structure::Unknown => None,
    }
}

/// Bytes [`check_by_reads`] wants of a file's start when nothing else needs more (the directories of a CR2, NEF or
/// ARW usually lie in it; others are fetched).
pub const STRUCTURE_HEAD: usize = 64 * 1024;

/// Largest CR3 `moov` box [`check_by_reads`] fetches (its track tables; a camera's is a few tens of kilobytes).
const MOOV_MAX_BYTES: u64 = 1024 * 1024;

/// CR3 top-level boxes walked at most.
const MAX_BOXES: usize = 64;

/// The structure of the RAW file `probe` describes, of `file_size` bytes, from its first bytes `head` and bounded
/// targeted reads through `fetch(offset, len)` (read-only): the TIFF directories within the ADR-0084 bounds
/// ([`crate::tiff::load_directories`]), CR3 box headers and `moov`, or the RAF header, never the image data. When a
/// bound leaves a range unknown and the file is at most `whole_max` bytes, the whole file is read once and judged
/// like the analysis does. [`Structure::Complete`] for a format without a check or that is not a RAW.
pub fn check_by_reads<E>(
    head: &[u8],
    file_size: u64,
    probe: &ProbeRecord,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
    whole_max: u64,
) -> Result<Structure, E> {
    let Some(Format::Raw(raw)) = probe.format else {
        return Ok(Structure::Complete);
    };
    if !ps_model::frame::structure_check_enabled() {
        return Ok(Structure::Complete);
    }
    if head.len() as u64 >= file_size {
        let whole = &head[..usize::try_from(file_size).unwrap_or(head.len())];
        return Ok(full(raw, whole));
    }
    let segments = match raw {
        RawFormat::Cr3 => cr3_segments(head, file_size, fetch)?,
        RawFormat::Raf | RawFormat::Crw | RawFormat::X3f => Vec::new(),
        _ if Tiff::parse(head).is_some() => {
            crate::tiff::load_directories(head, file_size, fetch)?.segments
        }
        _ => Vec::new(),
    };
    check_loaded(raw, head, &segments, file_size, fetch, whole_max)
}

/// [`check_by_reads`] over directories already fetched (`segments`, at absolute offsets): judged, and read whole
/// once when the result is [`Structure::Unknown`] and the file is at most `whole_max` bytes.
pub fn check_loaded<E>(
    raw: RawFormat,
    head: &[u8],
    segments: &[Segment],
    file_size: u64,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
    whole_max: u64,
) -> Result<Structure, E> {
    let mut rw2;
    let segments = if raw == RawFormat::Rw2 {
        rw2 = segments.to_vec();
        rw2_segments(head, &mut rw2, file_size, fetch)?;
        rw2.as_slice()
    } else {
        segments
    };
    let found = judge(raw, View::new(head, segments), file_size);
    if found != Structure::Unknown || file_size > whole_max {
        return Ok(found);
    }
    let whole = fetch(0, usize::try_from(file_size).unwrap_or(usize::MAX))?;
    Ok(if whole.len() as u64 == file_size {
        full(raw, &whole)
    } else {
        Structure::Unknown
    })
}

/// The judgement of a whole file in memory: nothing can be unknown there.
fn full(raw: RawFormat, bytes: &[u8]) -> Structure {
    match judge(raw, View::new(bytes, &[]), bytes.len() as u64) {
        Structure::Unknown => Structure::Complete,
        s => s,
    }
}

/// Judge the file of `file_size` bytes known through `view`.
fn judge(raw: RawFormat, view: View<'_>, file_size: u64) -> Structure {
    let mut worst = Worst::new(file_size);
    match raw {
        RawFormat::Cr3 => cr3(view, &mut worst),
        RawFormat::Raf => match view.get(0, 108) {
            Some(h) => raf(h, &mut worst),
            None => worst.unknown = true,
        },
        RawFormat::Crw | RawFormat::X3f => {}
        _ => {
            if let Some(t) = Tiff::parse_view(view) {
                tiff(&t, &mut worst);
                if raw == RawFormat::Rw2 {
                    panasonic(&t, &mut worst);
                }
            }
        }
    }
    match (worst.found, worst.unknown) {
        (Some(t), _) => Structure::Cut(t),
        (None, true) => Structure::Unknown,
        (None, false) => Structure::Complete,
    }
}

/// The range that ends furthest past the end of the file, and whether a range was not known.
struct Worst {
    file_size: u64,
    found: Option<Truncation>,
    unknown: bool,
}

impl Worst {
    fn new(file_size: u64) -> Self {
        Self {
            file_size,
            found: None,
            unknown: false,
        }
    }

    /// A range the file records, ending at `end`.
    fn range(&mut self, end: u64, what: &'static str) {
        if end > self.file_size && self.found.as_ref().is_none_or(|f| end > f.needed) {
            self.found = Some(Truncation {
                file_size: self.file_size,
                needed: end,
                what,
            });
        }
    }
}

/// At most this many strip or tile ranges per IFD are checked (a 100 MP DNG has a few thousand tiles).
const MAX_RANGES: usize = 1 << 20;

/// Every value of an integer entry (SHORT, LONG, LONG8, IFD, IFD8), up to [`MAX_RANGES`]: `Ok(None)` when the
/// value array lies inside the file but was not read, `Err(end)` when it lies past the end of the file, ending at
/// `end`.
fn values(t: &Tiff<'_>, e: &crate::tiff::Entry, file_size: u64) -> Result<Option<Vec<u64>>, u64> {
    let Some(size) = crate::tiff::type_size(e.typ) else {
        return Ok(Some(Vec::new()));
    };
    if !matches!(e.typ, 3 | 4 | 13 | 16 | 18) {
        return Ok(Some(Vec::new()));
    }
    let n = (e.count as usize).min(MAX_RANGES);
    let total = size as u64 * u64::from(e.count);
    let Some(b) = t.value_bytes(e) else {
        // Out of line (an inline value is always inside the directory): past the end, or not read.
        let at = t.value_offset(e).map_or(0, u64::from);
        let end = at.saturating_add(total);
        return if end > file_size { Err(end) } else { Ok(None) };
    };
    let endian = t.endian;
    Ok(Some(
        (0..n)
            .filter_map(|i| match size {
                2 => u16_at(b, i * 2, endian).map(u64::from),
                4 => u32_at(b, i * 4, endian).map(u64::from),
                _ => {
                    let (lo, hi) = (u32_at(b, i * 8, endian)?, u32_at(b, i * 8 + 4, endian)?);
                    Some(match endian {
                        crate::tiff::Endian::Little => u64::from(lo) | (u64::from(hi) << 32),
                        crate::tiff::Endian::Big => (u64::from(lo) << 32) | u64::from(hi),
                    })
                }
            })
            .collect(),
    ))
}

fn tiff(t: &Tiff<'_>, worst: &mut Worst) {
    let size = worst.file_size;
    // An IFD the file points to: its count and entries must be inside. The 4-byte next pointer after the entries is
    // not required: a table that ends the file without it reads as "no next IFD" everywhere.
    let pointer = |worst: &mut Worst, at: u64| {
        if at < 8 {
            return;
        }
        if at.saturating_add(2) > size {
            worst.range(at.saturating_add(2), "directory");
            return;
        }
        let count = usize::try_from(at)
            .ok()
            .and_then(|o| t.data.get(o, 2))
            .and_then(|b| u16_at(b, 0, t.endian));
        match count {
            Some(n) => worst.range(at + 2 + 12 * u64::from(n), "directory"),
            None => worst.unknown = true,
        }
    };
    pointer(worst, u64::from(t.ifd0));
    let (ifds, _) = t.walk();
    for ifd in &ifds {
        if matches!(ifd.kind, IfdKind::Gps | IfdKind::Interop) {
            continue;
        }
        if let IfdKind::Chain(_) = ifd.kind
            && let Some(next) = t.next_ifd(ifd)
        {
            pointer(worst, u64::from(next));
        }
        for e in &ifd.entries {
            if matches!(e.tag, 0x8769 | 0x014A) {
                match values(t, e, size) {
                    Ok(Some(v)) => v.into_iter().take(8).for_each(|p| pointer(worst, p)),
                    Ok(None) => worst.unknown = true,
                    Err(end) => worst.range(end, "directory"),
                }
            }
        }
        let what = what_of(t, ifd);
        for (otag, ltag, what) in [
            (0x0111, 0x0117, what),
            (0x0144, 0x0145, what),
            (0x0201, 0x0202, "preview"),
        ] {
            let (Some(o), Some(l)) = (ifd.get(otag), ifd.get(ltag)) else {
                continue;
            };
            let (offs, lens) = match (values(t, o, size), values(t, l, size)) {
                (Err(end), _) | (_, Err(end)) => {
                    worst.range(end, "directory");
                    continue;
                }
                (Ok(Some(o)), Ok(Some(l))) => (o, l),
                _ => {
                    worst.unknown = true;
                    continue;
                }
            };
            for (off, len) in offs.into_iter().zip(lens) {
                if len > 0 {
                    worst.range(off.saturating_add(len), what);
                }
            }
        }
    }
}

/// Panasonic's "RAW 1" compression (its tag 0x000B = 34316) with a fixed packing (`RawFormat`, tag 0x002D, 1 to 7
/// or absent): the sensor data starts at `RawDataOffset` (0x0118) and holds at least 16 bytes per 14 pixels of
/// `SensorWidth` x `SensorHeight` (0x0002, 0x0003; 14 per 16 bytes is the densest packing), with no length tag (its
/// strip byte count is 0). Cameras pad the data to 16 KB blocks; the minimum without padding is what counts.
/// `RawFormat` 8 has its strips checked ([`panasonic_v8`]); other formats and compressions get no check.
fn panasonic(t: &Tiff<'_>, worst: &mut Worst) {
    let (ifds, _) = t.walk();
    let Some(ifd0) = ifds.iter().find(|i| i.kind == IfdKind::Chain(0)) else {
        return;
    };
    let get = |tag: u16| ifd0.get(tag).and_then(|e| t.uint(e)).map(u64::from);
    match get(0x002D) {
        // RawFormat 8 (GH6, G9 II, S5 II/IIX, GH7, S1R II, ...): Huffman-coded strips of variable size, each
        // recorded (its own check; these files carry no 0x000B).
        Some(8) => return panasonic_v8(t, ifd0, worst),
        // Formats 1-7 (and files without the tag) pack at most 14 pixels in 16 bytes; others get no check.
        Some(1..=7) | None => {}
        Some(_) => return,
    }
    if get(0x000B) != Some(34316) {
        return;
    }
    let (Some(offset), Some(w), Some(h)) = (get(0x0118), get(0x0002), get(0x0003)) else {
        return;
    };
    // A size that overflows is no evidence (a hostile or garbled header).
    let Some(needed) = w
        .checked_mul(h)
        .and_then(|pixels| pixels.div_ceil(14).checked_mul(16))
        .and_then(|bytes| offset.checked_add(bytes))
    else {
        return;
    };
    if offset > 0 && needed > offset {
        worst.range(needed, "image data");
    }
}

/// Panasonic `RawFormat` 8 (rawler's `v8decompressor`): IFD0's `CF2StripByteOffsets` (0x0044) and
/// `CF2StripDataSize` (0x0046), each a little-endian u16 count then u32 values (absolute byte offsets, sizes in
/// bits). Each strip ends at its offset plus its size in bytes.
fn panasonic_v8(t: &Tiff<'_>, ifd0: &IfdView, worst: &mut Worst) {
    let (Some(o), Some(d)) = (ifd0.get(PANA_V8_OFFSETS), ifd0.get(PANA_V8_SIZES)) else {
        return;
    };
    let (Some(o), Some(d)) = (t.value_bytes(o), t.value_bytes(d)) else {
        worst.unknown = true;
        return;
    };
    let list = |b: &[u8]| -> Vec<u64> {
        let n = b
            .get(..2)
            .map_or(0, |c| usize::from(u16::from_le_bytes([c[0], c[1]])));
        (0..n)
            .map_while(|i| {
                b.get(2 + i * 4..6 + i * 4)
                    .map(|v| u64::from(u32::from_le_bytes([v[0], v[1], v[2], v[3]])))
            })
            .collect()
    };
    for (off, bits) in list(o).into_iter().zip(list(d)) {
        if off > 0 && bits > 0 {
            worst.range(off.saturating_add(bits.div_ceil(8)), "image data");
        }
    }
}

/// Panasonic `RawFormat` 8's strip tags ([`panasonic_v8`]).
const PANA_V8_OFFSETS: u16 = 0x0044;
const PANA_V8_SIZES: u16 = 0x0046;

/// Largest Panasonic strip table fetched (a camera's is 50 bytes).
const PANA_V8_MAX_BYTES: u64 = 64 * 1024;

/// The RW2 strip tables of [`panasonic_v8`] beyond `head` and `segments`, fetched (the directory load does not
/// fetch the values of private tags).
fn rw2_segments<E>(
    head: &[u8],
    segments: &mut Vec<Segment>,
    file_size: u64,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
) -> Result<(), E> {
    let wanted: Vec<(u64, u64)> = {
        let Some(t) = Tiff::parse_view(View::new(head, segments)) else {
            return Ok(());
        };
        let (ifds, _) = t.walk();
        let Some(ifd0) = ifds.iter().find(|i| i.kind == IfdKind::Chain(0)) else {
            return Ok(());
        };
        if ifd0.get(0x002D).and_then(|e| t.uint(e)) != Some(8) {
            return Ok(());
        }
        [PANA_V8_OFFSETS, PANA_V8_SIZES]
            .iter()
            .filter_map(|tag| ifd0.get(*tag))
            .filter(|e| t.value_bytes(e).is_none())
            .filter_map(|e| {
                let at = u64::from(t.value_offset(e)?);
                let len = u64::try_from(crate::tiff::type_size(e.typ)?)
                    .ok()?
                    .checked_mul(u64::from(e.count))?;
                (len <= PANA_V8_MAX_BYTES && at.saturating_add(len) <= file_size)
                    .then_some((at, len))
            })
            .collect()
    };
    for (at, len) in wanted {
        let bytes = fetch(at, usize::try_from(len).unwrap_or(0))?;
        segments.push(Segment { offset: at, bytes });
    }
    Ok(())
}

/// What an IFD's strips or tiles hold: the sensor data (CFA, linear raw, Canon's raw slices, uncompressed or
/// lossless) or a JPEG preview.
fn what_of(t: &Tiff<'_>, ifd: &IfdView) -> &'static str {
    let get = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e));
    let raw = matches!(get(0x0106), Some(32803 | 34892)) || ifd.get(0xC640).is_some();
    if !raw && matches!(get(0x0103), Some(6 | 7 | 99)) {
        "preview"
    } else {
        "image data"
    }
}

/// One top-level box header at `pos`: (type, header length, declared size); `None` when the view lacks it, the
/// size is 0 (to the end) or malformed.
fn box_at(view: View<'_>, pos: u64) -> Result<Option<([u8; 4], u64, u64)>, ()> {
    let p = usize::try_from(pos).map_err(|_| ())?;
    let h = view.get(p, 8).ok_or(())?;
    let mut kind = [0u8; 4];
    kind.copy_from_slice(&h[4..8]);
    let (header, size) = match be_u32(h, 0).ok_or(())? {
        0 => return Ok(None),
        1 => (16, view.get(p + 8, 8).and_then(|b| be_u64(b, 0)).ok_or(())?),
        s => (8, u64::from(s)),
    };
    Ok((size >= header).then_some((kind, header, size)))
}

fn cr3(view: View<'_>, worst: &mut Worst) {
    let size = worst.file_size;
    let mut pos = 0u64;
    let mut moov = None;
    for _ in 0..MAX_BOXES {
        // Fewer than 8 bytes left: no further box (trailing bytes are not evidence).
        if pos.saturating_add(8) > size {
            break;
        }
        let (kind, header, len) = match box_at(view, pos) {
            Ok(Some(b)) => b,
            Ok(None) => break,
            Err(()) => {
                worst.unknown = true;
                break;
            }
        };
        let end = pos.saturating_add(len);
        worst.range(
            end,
            if &kind == b"mdat" {
                "image data"
            } else {
                "directory"
            },
        );
        if &kind == b"moov" && moov.is_none() {
            moov = Some((pos.saturating_add(header), end.min(size)));
        }
        pos = end;
    }
    if let Some((body, end)) = moov {
        let body_bytes = usize::try_from(body)
            .ok()
            .zip(usize::try_from(end.saturating_sub(body)).ok())
            .and_then(|(b, l)| view.get(b, l));
        match body_bytes {
            Some(b) => cr3_tracks(b, worst),
            None => worst.unknown = true,
        }
    }
}

/// The samples of every track in a CR3 `moov` body: each chunk offset (`stco`/`co64`) with its sample size
/// (`stsz`), when every chunk holds one sample (Canon's layout); otherwise the chunk offsets alone.
fn cr3_tracks(moov: &[u8], worst: &mut Worst) {
    use crate::bmff::{boxes, children, find, find_path};
    let top = boxes(moov, 0, moov.len());
    for trak in top.iter().filter(|b| &b.kind == b"trak") {
        let Some(stbl) = find_path(
            moov,
            std::slice::from_ref(trak),
            &[b"trak", b"mdia", b"minf", b"stbl"],
        ) else {
            continue;
        };
        let kids = children(moov, &stbl, 0);
        let sizes: Vec<u64> = find(&kids, b"stsz").map_or_else(Vec::new, |b| {
            let body = b.body(moov);
            let (Some(fixed), Some(count)) = (be_u32(body, 4), be_u32(body, 8)) else {
                return Vec::new();
            };
            let count = (count as usize).min(MAX_BOXES);
            if fixed != 0 {
                vec![u64::from(fixed); count]
            } else {
                (0..count)
                    .map_while(|i| be_u32(body, 12 + i * 4).map(u64::from))
                    .collect()
            }
        });
        let offsets: Vec<u64> = if let Some(b) = find(&kids, b"co64") {
            let body = b.body(moov);
            let n = be_u32(body, 4).map_or(0, |n| (n as usize).min(MAX_BOXES));
            (0..n).map_while(|i| be_u64(body, 8 + i * 8)).collect()
        } else if let Some(b) = find(&kids, b"stco") {
            let body = b.body(moov);
            let n = be_u32(body, 4).map_or(0, |n| (n as usize).min(MAX_BOXES));
            (0..n)
                .map_while(|i| be_u32(body, 8 + i * 4).map(u64::from))
                .collect()
        } else {
            Vec::new()
        };
        let one_each = sizes.len() == offsets.len();
        for (i, off) in offsets.iter().enumerate() {
            let len = if one_each { sizes[i] } else { 1 };
            if *off > 0 && len > 0 {
                worst.range(off.saturating_add(len), "image data");
            }
        }
    }
}

/// The CR3 directory bytes beyond `head`: each top-level box header and the `moov` box (at most
/// [`MOOV_MAX_BYTES`]).
fn cr3_segments<E>(
    head: &[u8],
    file_size: u64,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
) -> Result<Vec<Segment>, E> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut pos = 0u64;
    for _ in 0..MAX_BOXES {
        if pos.saturating_add(8) > file_size {
            break;
        }
        if !View::new(head, &segments).has(pos, 16.min(file_size - pos)) {
            let bytes = fetch(pos, 16)?;
            if bytes.len() < 8 {
                break;
            }
            segments.push(Segment { offset: pos, bytes });
        }
        let (kind, header, len) = match box_at(View::new(head, &segments), pos) {
            Ok(Some(b)) => b,
            _ => break,
        };
        let end = pos.saturating_add(len);
        if &kind == b"moov" {
            let body_end = end.min(file_size);
            let body = pos.saturating_add(header);
            let body_len = body_end.saturating_sub(body);
            if body_len <= MOOV_MAX_BYTES && !View::new(head, &segments).has(body, body_len) {
                let bytes = fetch(body, usize::try_from(body_len).unwrap_or(0))?;
                segments.push(Segment {
                    offset: body,
                    bytes,
                });
            }
        }
        pos = end;
    }
    Ok(segments)
}

fn raf(h: &[u8], worst: &mut Worst) {
    for (at, what) in [(84, "preview"), (92, "directory"), (100, "image data")] {
        let (Some(off), Some(len)) = (be_u32(h, at), be_u32(h, at + 4)) else {
            continue;
        };
        if off > 0 && len > 0 {
            worst.range(u64::from(off) + u64::from(len), what);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::probe_buffer;

    fn probe_full(bytes: &[u8], format: Format) -> ProbeRecord {
        let mut p = probe_buffer(bytes, None);
        p.format = Some(format);
        p
    }

    #[test]
    fn sizes_read_in_decimal_units() {
        assert_eq!(size_text(24_600_000), "24.6 MB");
        assert_eq!(size_text(3_100_000), "3.1 MB");
        assert_eq!(size_text(812_300), "812.3 KB");
        assert_eq!(size_text(512), "512 bytes");
        let t = Truncation {
            file_size: 3_100_000,
            needed: 24_600_000,
            what: "image data",
        };
        assert_eq!(
            t.message(),
            "the file ends at 3.1 MB but its image data runs to 24.6 MB: it was cut short (a card removed or a \
             copy stopped while writing)"
        );
        let close = Truncation {
            file_size: 1_000_000,
            needed: 1_000_010,
            what: "preview",
        };
        assert!(
            close.message().contains("1000000 bytes"),
            "{}",
            close.message()
        );
    }

    /// A minimal CR3: `ftyp`, `moov`, then an `mdat` of `mdat_len` declared bytes holding `have` of them.
    fn cr3_bytes(mdat_len: u32, have: usize, mdat_size_zero: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(b"ftypcrx \0\0\0\x01crx ");
        b.extend_from_slice(&16u32.to_be_bytes());
        b.extend_from_slice(b"moov\0\0\0\0\0\0\0\0");
        let at = b.len();
        b.extend_from_slice(&(if mdat_size_zero { 0 } else { 8 + mdat_len }).to_be_bytes());
        b.extend_from_slice(b"mdat");
        b.extend(std::iter::repeat_n(7u8, have));
        assert_eq!(at, 36);
        b
    }

    #[test]
    fn cr3_with_its_mdat_cut_is_found() {
        let b = cr3_bytes(1000, 400, false);
        let t = truncation(&b, &probe_full(&b, Format::Raw(RawFormat::Cr3))).unwrap();
        assert_eq!(t.file_size, 444);
        assert_eq!(t.needed, 36 + 8 + 1000);
        assert_eq!(t.what, "image data");
        let whole = cr3_bytes(400, 400, false);
        assert_eq!(
            truncation(&whole, &probe_full(&whole, Format::Raw(RawFormat::Cr3))),
            None
        );
        // A box of size 0 runs to the end of the file: not evidence.
        let zero = cr3_bytes(0, 400, true);
        assert_eq!(
            truncation(&zero, &probe_full(&zero, Format::Raw(RawFormat::Cr3))),
            None
        );
    }

    /// A minimal RAF header: magic, then the JPEG, CFA header and CFA (offset, length) pairs at 84..108.
    fn raf_bytes(cfa_len: u32, len: usize) -> Vec<u8> {
        let mut b = vec![0u8; 108];
        b[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
        let put =
            |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_be_bytes());
        put(&mut b, 84, 200);
        put(&mut b, 88, 100);
        put(&mut b, 92, 300);
        put(&mut b, 96, 50);
        put(&mut b, 100, 400);
        put(&mut b, 104, cfa_len);
        b.resize(len, 1);
        b
    }

    #[test]
    fn raf_with_its_cfa_cut_is_found() {
        let b = raf_bytes(1000, 900);
        let t = truncation(&b, &probe_full(&b, Format::Raw(RawFormat::Raf))).unwrap();
        assert_eq!((t.file_size, t.needed, t.what), (900, 1400, "image data"));
        let whole = raf_bytes(500, 900);
        assert_eq!(
            truncation(&whole, &probe_full(&whole, Format::Raw(RawFormat::Raf))),
            None
        );
    }

    #[test]
    fn a_partial_buffer_is_never_judged() {
        let b = cr3_bytes(1000, 400, false);
        let mut p = probe_full(&b, Format::Raw(RawFormat::Cr3));
        p.file_size = 1044;
        assert_eq!(truncation(&b, &p), None, "the buffer is not the whole file");
    }

    #[test]
    fn both_cr2_cuts_are_found_with_the_length_they_need() {
        use ps_fixtures::dng::{Cut, cut, raw_ranges};
        let full = ps_fixtures::dng::converted_burst_frame(5, 0, 0, 960).cr2;
        let ((p_off, p_len), sensor) = raw_ranges(&full).unwrap();
        let (s_off, s_len) = sensor.unwrap();
        let p = |b: &[u8]| probe_buffer(b, None);
        assert_eq!(truncation(&full, &p(&full)), None, "complete");
        let a = cut(&full, Cut::InPreview).unwrap();
        let t = truncation(&a, &p(&a)).unwrap();
        // Both the preview and the sensor strip after it run past the end; the furthest is named.
        assert_eq!(t.file_size, a.len() as u64);
        assert_eq!(t.needed, (s_off + s_len) as u64);
        assert!((p_off + p_len) < s_off + s_len);
        let b = cut(&full, Cut::AfterPreview).unwrap();
        let t = truncation(&b, &p(&b)).unwrap();
        assert_eq!(
            (t.file_size, t.needed, t.what),
            (b.len() as u64, (s_off + s_len) as u64, "image data")
        );
        // cr2_unknown_lens: only the preview, which is IFD0's strip.
        let lens = ps_fixtures::dng::cr2_unknown_lens(11);
        assert_eq!(
            truncation(&lens, &p(&lens)),
            None,
            "its 0xC5D8 count is not evidence"
        );
        let ((lo, ll), _) = raw_ranges(&lens).unwrap();
        let c = cut(&lens, Cut::InPreview).unwrap();
        let t = truncation(&c, &p(&c)).unwrap();
        assert_eq!((t.needed, t.what), ((lo + ll) as u64, "preview"));
        let eighth = &lens[..lens.len() / 8];
        assert!(truncation(eighth, &p(eighth)).is_some(), "the first eighth");
    }

    /// [`check_by_reads`] over `b` from its first `head` bytes, counting what it fetched: (result, bytes, reads).
    fn by_reads(b: &[u8], head: usize, whole_max: u64) -> (Structure, u64, u32) {
        let pr = probe_full(b, probe_buffer(b, None).format.unwrap_or(Format::Tiff));
        let h = &b[..b.len().min(head)];
        let (mut bytes, mut reads) = (0u64, 0u32);
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let start = (off as usize).min(b.len());
            let end = start.saturating_add(len).min(b.len());
            bytes += (end - start) as u64;
            reads += 1;
            Ok(b[start..end].to_vec())
        };
        let s = check_by_reads(h, b.len() as u64, &pr, &mut fetch, whole_max).unwrap();
        (s, bytes, reads)
    }

    #[test]
    fn targeted_reads_agree_with_the_whole_file() {
        use ps_fixtures::dng::{Cut, cut};
        let full = ps_fixtures::dng::converted_burst_frame(5, 0, 0, 960).cr2;
        let lens = ps_fixtures::dng::cr2_unknown_lens(11);
        let mut files = vec![full.clone(), lens.clone()];
        files.extend(cut(&full, Cut::InPreview));
        files.extend(cut(&full, Cut::AfterPreview));
        files.extend(cut(&lens, Cut::InPreview));
        for b in &files {
            let whole = truncation(b, &probe_buffer(b, None));
            // A head of 64 bytes: every directory comes from targeted reads.
            for head in [64, STRUCTURE_HEAD] {
                let (s, bytes, _) = by_reads(b, head, 0);
                assert_eq!(s.cut(), whole.as_ref(), "head {head}");
                assert!(s != Structure::Unknown);
                assert!(
                    bytes <= crate::tiff::DIRECTORY_READ_MAX_BYTES,
                    "{bytes} bytes fetched"
                );
            }
        }
    }

    /// A little-endian TIFF with IFD0 at 8 holding `entries` (tag, type, count, value), its next pointer left out
    /// when `next` is false, then `tail` bytes of data.
    fn tiff_bytes(
        magic: u16,
        entries: &[(u16, u16, u32, u32)],
        next: bool,
        tail: usize,
    ) -> Vec<u8> {
        let mut b = vec![b'I', b'I'];
        b.extend_from_slice(&magic.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, typ, count, value) in entries {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            b.extend_from_slice(&value.to_le_bytes());
        }
        if next {
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        b.extend(std::iter::repeat_n(0u8, tail));
        b
    }

    #[test]
    fn a_last_directory_without_its_next_pointer_is_not_a_cut() {
        let raw = Format::Raw(RawFormat::Nef);
        // Data first (strip 8..), then a SubIFD table that ends the file without its 4-byte next pointer.
        let mut b = tiff_bytes(42, &[(0x0100, 4, 1, 16), (0x014A, 4, 1, 0)], true, 64);
        let sub = b.len() as u32;
        b[8 + 2 + 12 + 8..8 + 2 + 12 + 12].copy_from_slice(&sub.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0x0100u16.to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&16u32.to_le_bytes());
        assert_eq!(truncation(&b, &probe_full(&b, raw)), None, "entries inside");
        // The entries themselves run past the end: cut.
        let short = &b[..b.len() - 3];
        let t = truncation(short, &probe_full(short, raw)).unwrap();
        assert_eq!((t.needed, t.what), (b.len() as u64, "directory"));
    }

    #[test]
    fn panasonic_raw_1_needs_its_sensor_data() {
        // SensorWidth 28 x SensorHeight 10 = 280 pixels: 20 packets of 16 bytes = 320 bytes from RawDataOffset 200.
        let entries = [
            (0x0002, 3, 1, 28),
            (0x0003, 3, 1, 10),
            (0x000B, 3, 1, 34316),
            (0x0111, 4, 1, u32::MAX),
            (0x0117, 4, 1, 0),
            (0x0118, 4, 1, 200),
        ];
        let rw2 = Format::Raw(RawFormat::Rw2);
        let whole = tiff_bytes(0x55, &entries, true, 520 - 86);
        assert_eq!(whole.len(), 520);
        assert_eq!(truncation(&whole, &probe_full(&whole, rw2)), None);
        let cut = &whole[..400];
        let t = truncation(cut, &probe_full(cut, rw2)).unwrap();
        assert_eq!((t.needed, t.what), (520, "image data"));
        // Another compression: no check.
        let mut other = entries;
        other[2].3 = 34826;
        let o = tiff_bytes(0x55, &other, true, 400 - 86);
        assert_eq!(truncation(&o, &probe_full(&o, rw2)), None);
    }

    #[test]
    fn panasonic_fixed_packings_are_checked_and_unknown_formats_are_not() {
        // RawFormat (0x002D) 8 is Huffman-coded in variable-size strips: smaller than the fixed packings' bound.
        let with_format = |f: u32, padding: usize| {
            let entries = [
                (0x0002, 3, 1, 28),
                (0x0003, 3, 1, 10),
                (0x000B, 3, 1, 34316),
                (0x002D, 3, 1, f),
                (0x0111, 4, 1, u32::MAX),
                (0x0117, 4, 1, 0),
                (0x0118, 4, 1, 200),
            ];
            tiff_bytes(0x55, &entries, true, padding)
        };
        let rw2 = Format::Raw(RawFormat::Rw2);
        // 8 without its strip tables, and unknown formats: no evidence.
        for f in [8, 9, 0x7FFF] {
            let b = with_format(f, 300 - 98);
            assert_eq!(truncation(&b, &probe_full(&b, rw2)), None, "RawFormat {f}");
        }
        for f in [1, 4, 7] {
            let b = with_format(f, 300 - 98);
            let t = truncation(&b, &probe_full(&b, rw2)).expect("a fixed packing is checked");
            assert_eq!((t.needed, t.what), (520, "image data"), "RawFormat {f}");
        }
    }

    /// Panasonic RawFormat 8: the strips its `CF2StripByteOffsets`/`CF2StripDataSize` tables record (u16 count,
    /// u32 values, sizes in bits), found by the whole-file check and by targeted reads.
    #[test]
    fn panasonic_raw_format_8_needs_its_strips() {
        // Header + 5 entries = 74 bytes, then the two 10-byte tables, then strips 100..200 and 200..501.
        let entries = [
            (0x0002, 3, 1, 28),
            (0x0003, 3, 1, 10),
            (0x002D, 3, 1, 8),
            (0x0044, 7, 10, 74),
            (0x0046, 7, 10, 84),
        ];
        let mut whole = tiff_bytes(0x55, &entries, true, 520 - 74);
        let table = |a: u32, b: u32| {
            let mut t = 2u16.to_le_bytes().to_vec();
            t.extend_from_slice(&a.to_le_bytes());
            t.extend_from_slice(&b.to_le_bytes());
            t
        };
        whole[74..84].copy_from_slice(&table(100, 200));
        whole[84..94].copy_from_slice(&table(800, 2401));
        let rw2 = Format::Raw(RawFormat::Rw2);
        assert_eq!(truncation(&whole, &probe_full(&whole, rw2)), None);
        let cut = &whole[..400];
        let p = probe_full(cut, rw2);
        let t = truncation(cut, &p).unwrap();
        assert_eq!((t.needed, t.what), (501, "image data"));
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let off = usize::try_from(off).unwrap_or(usize::MAX).min(cut.len());
            Ok(cut[off..cut.len().min(off.saturating_add(len))].to_vec())
        };
        // The tables lie beyond a head of 74 bytes: fetched, never the whole file.
        let s = check_by_reads(&cut[..74], cut.len() as u64, &p, &mut fetch, 0).unwrap();
        assert_eq!(s.cut().map(|t| t.needed), Some(501), "{s:?}");
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let off = usize::try_from(off).unwrap_or(usize::MAX).min(whole.len());
            Ok(whole[off..whole.len().min(off.saturating_add(len))].to_vec())
        };
        let p = probe_full(&whole, rw2);
        let s = check_by_reads(&whole[..74], whole.len() as u64, &p, &mut fetch, 0).unwrap();
        assert_eq!(s, Structure::Complete);
    }

    #[test]
    fn hostile_sizes_are_no_evidence_and_never_panic() {
        // A sensor of u32::MAX x u32::MAX: the needed size overflows, so it is no evidence.
        let entries = [
            (0x0002, 4, 1, u32::MAX),
            (0x0003, 4, 1, u32::MAX),
            (0x000B, 3, 1, 34316),
            (0x0111, 4, 1, u32::MAX),
            (0x0117, 4, 1, 0),
            (0x0118, 4, 1, u32::MAX),
        ];
        let rw2 = Format::Raw(RawFormat::Rw2);
        let b = tiff_bytes(0x55, &entries, true, 64);
        let p = probe_full(&b, rw2);
        assert_eq!(truncation(&b, &p), None);
        let whole = b.clone();
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let off = usize::try_from(off).unwrap_or(usize::MAX).min(whole.len());
            Ok(whole[off..whole.len().min(off.saturating_add(len))].to_vec())
        };
        let s = check_by_reads(&b[..16], b.len() as u64, &p, &mut fetch, 0).unwrap();
        assert!(s.cut().is_none(), "{s:?}");
        // A CR3 whose `moov` declares a 64-bit size of u64::MAX: its end saturates, the file is cut, nothing
        // overflows (the whole-file judgement and the targeted reads alike).
        let mut c = cr3_with_track(0, 0, None);
        c.extend_from_slice(&1u32.to_be_bytes());
        c.extend_from_slice(b"moov");
        c.extend_from_slice(&u64::MAX.to_be_bytes());
        c.extend_from_slice(&[0u8; 8]);
        let cr3 = Format::Raw(RawFormat::Cr3);
        let p = probe_full(&c, cr3);
        assert!(truncation(&c, &p).is_some());
        let whole = c.clone();
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let off = usize::try_from(off).unwrap_or(usize::MAX).min(whole.len());
            Ok(whole[off..whole.len().min(off.saturating_add(len))].to_vec())
        };
        let s = check_by_reads(&c[..24], c.len() as u64, &p, &mut fetch, 0).unwrap();
        assert!(s.cut().is_some(), "{s:?}");
    }

    /// A minimal CR3 whose one track has a sample of `len` bytes at `off`: `ftyp`, `moov` (trak/mdia/minf/stbl
    /// with `stsz` and `co64`), then `mdat` of `mdat_len` bytes when given.
    fn cr3_with_track(off: u64, len: u32, mdat_len: Option<u32>) -> Vec<u8> {
        fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut b = ((8 + body.len()) as u32).to_be_bytes().to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b
        }
        let mut stsz = vec![0u8; 4];
        stsz.extend_from_slice(&len.to_be_bytes());
        stsz.extend_from_slice(&1u32.to_be_bytes());
        let mut co64 = vec![0u8; 4];
        co64.extend_from_slice(&1u32.to_be_bytes());
        co64.extend_from_slice(&off.to_be_bytes());
        let stbl = [boxed(b"stsz", &stsz), boxed(b"co64", &co64)].concat();
        let trak = boxed(
            b"trak",
            &boxed(b"mdia", &boxed(b"minf", &boxed(b"stbl", &stbl))),
        );
        let mut b = boxed(b"ftyp", b"crx \0\0\0\x01crx ");
        b.extend(boxed(b"moov", &trak));
        if let Some(n) = mdat_len {
            b.extend(boxed(b"mdat", &vec![7u8; n as usize]));
        }
        b
    }

    #[test]
    fn cr3_cut_at_a_box_boundary_is_found_by_its_tracks() {
        let cr3 = Format::Raw(RawFormat::Cr3);
        let head = cr3_with_track(0, 0, None).len() as u64;
        // The sample lies in mdat, 8 bytes after its header.
        let whole = cr3_with_track(head + 8, 300, Some(300));
        assert_eq!(truncation(&whole, &probe_full(&whole, cr3)), None);
        // Cut right after moov: no box runs past the end, but the track's sample does.
        let at_boundary = &whole[..head as usize];
        let t = truncation(at_boundary, &probe_full(at_boundary, cr3)).unwrap();
        assert_eq!((t.needed, t.what), (head + 8 + 300, "image data"));
        // By targeted reads from a 16-byte head: the same, from the box headers and moov.
        let pr = probe_full(at_boundary, cr3);
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let s = (off as usize).min(at_boundary.len());
            Ok(at_boundary[s..(s + len).min(at_boundary.len())].to_vec())
        };
        let s = check_by_reads(&at_boundary[..16], head, &pr, &mut fetch, 0).unwrap();
        assert_eq!(s.cut().map(|t| t.needed), Some(head + 8 + 300));
        let pr = probe_full(&whole, cr3);
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let s = (off as usize).min(whole.len());
            Ok(whole[s..(s + len).min(whole.len())].to_vec())
        };
        let s = check_by_reads(&whole[..16], whole.len() as u64, &pr, &mut fetch, 0).unwrap();
        assert_eq!(s, Structure::Complete);
    }

    #[test]
    fn an_unknown_range_reads_the_file_whole_only_when_small_enough() {
        // A strip array of 200,000 offsets (800 KB) is beyond what the directory reads fetch.
        let n = 200_000u32;
        let mut b = tiff_bytes(42, &[(0x0111, 4, n, 0), (0x0117, 4, n, 0)], true, 0);
        let offs = b.len() as u32;
        b.extend(std::iter::repeat_n(0u8, n as usize * 4));
        let lens = b.len() as u32;
        b.extend(std::iter::repeat_n(0u8, n as usize * 4));
        b[8 + 2 + 8..8 + 2 + 12].copy_from_slice(&offs.to_le_bytes());
        b[8 + 2 + 12 + 8..8 + 2 + 24].copy_from_slice(&lens.to_le_bytes());
        // Every strip: 1 byte at the end of the file.
        let end = b.len() as u32;
        for i in 0..n as usize {
            b[offs as usize + i * 4..offs as usize + i * 4 + 4].copy_from_slice(&end.to_le_bytes());
            b[lens as usize + i * 4] = 1;
        }
        let complete = b.len();
        b.push(0);
        let nef = Format::Raw(RawFormat::Nef);
        assert_eq!(truncation(&b, &probe_full(&b, nef)), None);
        let (s, _, _) = by_reads_as(&b, nef, 0);
        assert_eq!(s, Structure::Unknown, "not judged within the bounds");
        let (s, bytes, _) = by_reads_as(&b, nef, u64::MAX);
        assert_eq!(s, Structure::Complete);
        assert!(bytes >= b.len() as u64, "read whole");
        let cut = &b[..complete];
        let (s, ..) = by_reads_as(cut, nef, u64::MAX);
        assert_eq!(s.cut().map(|t| t.needed), Some(complete as u64 + 1));
    }

    fn by_reads_as(b: &[u8], format: Format, whole_max: u64) -> (Structure, u64, u32) {
        let pr = probe_full(b, format);
        let h = &b[..64];
        let (mut bytes, mut reads) = (0u64, 0u32);
        let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
            let start = (off as usize).min(b.len());
            let end = start.saturating_add(len).min(b.len());
            bytes += (end - start) as u64;
            reads += 1;
            Ok(b[start..end].to_vec())
        };
        let s = check_by_reads(h, b.len() as u64, &pr, &mut fetch, whole_max).unwrap();
        (s, bytes, reads)
    }
}
