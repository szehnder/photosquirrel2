//! A small, bounded, panic-free TIFF/EXIF IFD walker over a byte slice.
//!
//! The probe needs byte offsets (EXIF block, embedded previews) that general EXIF libraries do not expose,
//! and it must stay within the probe window, so this walker is deliberately minimal: it reads entries
//! lazily, never allocates more than the entry list, caps IFD counts and depth, and detects cycles.
//!
//! A plain TIFF may keep its directories after the pixel data, beyond the probe window (ADR-0084). The walker
//! then reads a [`View`]: the window plus [`Segment`]s fetched by [`load_directories`] with bounded targeted
//! reads, at their absolute offsets.

/// Byte order of a TIFF block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// Read helpers that return `None` instead of panicking.
pub(crate) fn u16_at(b: &[u8], off: usize, e: Endian) -> Option<u16> {
    let s = b.get(off..off.checked_add(2)?)?;
    let a = [s[0], s[1]];
    Some(match e {
        Endian::Little => u16::from_le_bytes(a),
        Endian::Big => u16::from_be_bytes(a),
    })
}

pub(crate) fn u32_at(b: &[u8], off: usize, e: Endian) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    let a = [s[0], s[1], s[2], s[3]];
    Some(match e {
        Endian::Little => u32::from_le_bytes(a),
        Endian::Big => u32::from_be_bytes(a),
    })
}

pub(crate) fn be_u16(b: &[u8], off: usize) -> Option<u16> {
    u16_at(b, off, Endian::Big)
}

pub(crate) fn be_u32(b: &[u8], off: usize) -> Option<u32> {
    u32_at(b, off, Endian::Big)
}

pub(crate) fn be_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_be_bytes(a))
}

/// Maximum IFDs visited per TIFF block (chain + sub-IFDs).
const MAX_IFDS: usize = 64;
/// Maximum IFDs followed in the main chain (IFD0 to IFD8): pages, reduced-resolution levels, thumbnails.
pub const MAX_CHAIN: usize = 9;
/// Maximum entries read per IFD.
const MAX_ENTRIES: usize = 1024;
/// Maximum sub-IFD nesting.
const MAX_DEPTH: usize = 4;

/// One IFD entry: tag, field type, count and the 4-byte value/offset field position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// Offset (inside the TIFF block) of the entry's 4-byte value field.
    field_pos: usize,
}

pub(crate) fn type_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        // RATIONAL, SRATIONAL, DOUBLE; LONG8, SLONG8 and IFD8 (BigTIFF types some classic writers use).
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

/// Bytes fetched from beyond the probe window, at their absolute offset in the TIFF block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// The bytes of a TIFF block known so far: a prefix starting at the TIFF header, plus segments further on.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    head: &'a [u8],
    segments: &'a [Segment],
}

impl<'a> View<'a> {
    pub fn new(head: &'a [u8], segments: &'a [Segment]) -> Self {
        Self { head, segments }
    }

    /// The `len` bytes at `off`, when one region (the prefix or a segment) holds all of them.
    pub fn get(&self, off: usize, len: usize) -> Option<&'a [u8]> {
        let end = off.checked_add(len)?;
        if end <= self.head.len() {
            return self.head.get(off..end);
        }
        self.segments.iter().find_map(|s| {
            let start = usize::try_from(s.offset).ok()?;
            let local = off.checked_sub(start)?;
            s.bytes.get(local..local.checked_add(len)?)
        })
    }

    /// The bytes from `off` to the end of the region holding `off` (empty when no region does).
    pub fn tail(&self, off: usize) -> &'a [u8] {
        if off < self.head.len() {
            return &self.head[off..];
        }
        self.segments
            .iter()
            .find_map(|s| {
                let local = off.checked_sub(usize::try_from(s.offset).ok()?)?;
                (local < s.bytes.len()).then(|| &s.bytes[local..])
            })
            .unwrap_or(&[])
    }

    /// Length of the prefix.
    pub fn head_len(&self) -> usize {
        self.head.len()
    }

    /// Whether every byte of `off..off + len` is known.
    pub fn has(&self, off: u64, len: u64) -> bool {
        match (usize::try_from(off), usize::try_from(len)) {
            (Ok(o), Ok(l)) => self.get(o, l).is_some(),
            _ => false,
        }
    }
}

/// A TIFF block: `data` starts at the TIFF header ("II*\0" / "MM\0*").
#[derive(Clone, Copy, Debug)]
pub struct Tiff<'a> {
    pub data: View<'a>,
    pub endian: Endian,
    pub ifd0: u32,
}

/// Where an IFD sits in the structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfdKind {
    /// IFD0, IFD1, ... in the main chain (`index` 0-based).
    Chain(usize),
    /// The Exif private IFD.
    Exif,
    Gps,
    Interop,
    /// A TIFF/DNG SubIFD (`0x014A`).
    Sub,
}

/// One parsed IFD: its entries plus where it lives.
#[derive(Clone, Debug)]
pub struct IfdView {
    pub kind: IfdKind,
    pub offset: u32,
    pub entries: Vec<Entry>,
}

impl IfdView {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

impl<'a> Tiff<'a> {
    /// Parse the 8-byte TIFF header. Accepts the classic magic 42 and the RAW variants (ORF "RO"/"RS",
    /// RW2 0x55).
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        Self::parse_view(View::new(data, &[]))
    }

    /// Parse the header of a block known through a [`View`].
    pub fn parse_view(data: View<'a>) -> Option<Self> {
        let head = data.get(0, 8)?;
        let endian = match head.get(..2)? {
            b"II" => Endian::Little,
            b"MM" => Endian::Big,
            _ => return None,
        };
        let magic = u16_at(head, 2, endian)?;
        if !matches!(magic, 42 | 0x4F52 | 0x5352 | 0x55) {
            return None;
        }
        let ifd0 = u32_at(head, 4, endian)?;
        Some(Self { data, endian, ifd0 })
    }

    fn u16(&self, off: usize) -> Option<u16> {
        u16_at(self.data.get(off, 2)?, 0, self.endian)
    }

    fn u32(&self, off: usize) -> Option<u32> {
        u32_at(self.data.get(off, 4)?, 0, self.endian)
    }

    /// Read one IFD; with `misses`, record the byte range still needed when its table is not fully known.
    fn read_ifd_noting(
        &self,
        offset: u32,
        kind: IfdKind,
        misses: &mut Option<&mut Vec<(u64, u64)>>,
    ) -> Option<IfdView> {
        let off = usize::try_from(offset).ok()?;
        let Some(n) = self.u16(off).map(usize::from) else {
            if let Some(m) = misses {
                m.push((u64::from(offset), DIRECTORY_READ_AHEAD));
            }
            return None;
        };
        if n == 0 || n > MAX_ENTRIES {
            return None;
        }
        if let Some(m) = misses
            && !self.data.has(u64::from(offset), 2 + 12 * n as u64 + 4)
        {
            m.push((u64::from(offset), 2 + 12 * n as u64 + 4));
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let p = off + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) =
                (self.u16(p), self.u16(p + 2), self.u32(p + 4))
            else {
                break;
            };
            entries.push(Entry {
                tag,
                typ,
                count,
                field_pos: p + 8,
            });
        }
        if entries.is_empty() {
            return None;
        }
        Some(IfdView {
            kind,
            offset,
            entries,
        })
    }

    /// Offset of the next IFD in the chain after the IFD at `offset`.
    pub(crate) fn next_ifd(&self, ifd: &IfdView) -> Option<u32> {
        let off = usize::try_from(ifd.offset).ok()?;
        let n = usize::from(self.u16(off)?);
        let next = self.u32(off + 2 + n * 12)?;
        (next != 0).then_some(next)
    }

    /// All IFDs reachable from IFD0 (chain, Exif, GPS, Interop, SubIFDs), bounded and cycle-safe.
    /// Returns the IFDs found and whether IFD0 itself was unreadable.
    pub fn walk(&self) -> (Vec<IfdView>, bool) {
        self.walk_noting(None)
    }

    /// [`Self::walk`], recording in `misses` the (offset, length) ranges of IFD tables and of the values of
    /// [`wanted_value`] entries that the view does not hold yet.
    pub fn walk_noting(&self, misses: Option<&mut Vec<(u64, u64)>>) -> (Vec<IfdView>, bool) {
        let mut misses = misses;
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let mut stack: Vec<(u32, IfdKind, usize)> = vec![(self.ifd0, IfdKind::Chain(0), 0)];
        let mut ifd0_bad = false;
        while let Some((off, kind, depth)) = stack.pop() {
            if out.len() >= MAX_IFDS || seen.contains(&off) || off < 8 {
                if kind == IfdKind::Chain(0) {
                    ifd0_bad = true;
                }
                continue;
            }
            seen.push(off);
            let Some(ifd) = self.read_ifd_noting(off, kind, &mut misses) else {
                if kind == IfdKind::Chain(0) {
                    ifd0_bad = true;
                }
                continue;
            };
            if let IfdKind::Chain(i) = kind
                && let Some(next) = self.next_ifd(&ifd)
                && i + 1 < MAX_CHAIN
            {
                stack.push((next, IfdKind::Chain(i + 1), depth));
            }
            if let Some(m) = &mut misses {
                for e in &ifd.entries {
                    if let Some(range) = self.missing_value(e) {
                        m.push(range);
                    }
                }
            }
            if depth < MAX_DEPTH {
                for e in &ifd.entries {
                    let child = match e.tag {
                        0x8769 => Some(IfdKind::Exif),
                        0x8825 => Some(IfdKind::Gps),
                        0xA005 => Some(IfdKind::Interop),
                        0x014A => Some(IfdKind::Sub),
                        _ => None,
                    };
                    if let Some(ck) = child {
                        for v in self.uints(e).into_iter().take(8) {
                            stack.push((v, ck, depth + 1));
                        }
                    }
                }
            }
            out.push(ifd);
        }
        (out, ifd0_bad)
    }

    /// The value bytes of an entry (inline or at its offset), when inside the block.
    pub fn value_bytes(&self, e: &Entry) -> Option<&'a [u8]> {
        let size = type_size(e.typ)?.checked_mul(usize::try_from(e.count).ok()?)?;
        if size <= 4 {
            self.data.get(e.field_pos, size)
        } else {
            let off = usize::try_from(self.u32(e.field_pos)?).ok()?;
            self.data.get(off, size)
        }
    }

    /// The (offset, length) of an out-of-line value the view lacks, for entries worth fetching.
    fn missing_value(&self, e: &Entry) -> Option<(u64, u64)> {
        let size = type_size(e.typ)?.checked_mul(usize::try_from(e.count).ok()?)?;
        if size <= 4 || size as u64 > wanted_value(e.tag)? || self.value_bytes(e).is_some() {
            return None;
        }
        Some((u64::from(self.value_offset(e)?), size as u64))
    }

    /// Absolute-in-block offset of an entry's out-of-line value (for maker notes etc.).
    pub fn value_offset(&self, e: &Entry) -> Option<u32> {
        self.u32(e.field_pos)
    }

    /// Non-negative integer values of an integer entry (at most 64 values): BYTE, SHORT, LONG and IFD, the
    /// signed SBYTE, SSHORT and SLONG, and LONG8, SLONG8 and IFD8, as the `tiff` decoder accepts them for
    /// dimensions. A negative value, or one above `u32::MAX`, is left out.
    pub fn uints(&self, e: &Entry) -> Vec<u32> {
        let Some(b) = self.value_bytes(e) else {
            // Value outside the window: an inline single value is still readable.
            return match e.typ {
                3 if e.count == 1 => self.u16(e.field_pos).map(u32::from).into_iter().collect(),
                8 if e.count == 1 => self
                    .u16(e.field_pos)
                    .and_then(|v| u32::try_from(v as i16).ok())
                    .into_iter()
                    .collect(),
                4 | 13 if e.count == 1 => self.u32(e.field_pos).into_iter().collect(),
                9 if e.count == 1 => self
                    .u32(e.field_pos)
                    .and_then(|v| u32::try_from(v as i32).ok())
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            };
        };
        let n = usize::try_from(e.count).unwrap_or(0).min(64);
        let endian = self.endian;
        let u64_at = |i: usize| -> Option<u64> {
            let (a, b) = (u32_at(b, i * 8, endian)?, u32_at(b, i * 8 + 4, endian)?);
            Some(match endian {
                Endian::Little => u64::from(a) | (u64::from(b) << 32),
                Endian::Big => (u64::from(a) << 32) | u64::from(b),
            })
        };
        match e.typ {
            1 | 7 => b.iter().take(n).map(|v| u32::from(*v)).collect(),
            6 => b
                .iter()
                .take(n)
                .filter_map(|v| u32::try_from(*v as i8).ok())
                .collect(),
            3 => (0..n)
                .filter_map(|i| u16_at(b, i * 2, endian).map(u32::from))
                .collect(),
            8 => (0..n)
                .filter_map(|i| u32::try_from(u16_at(b, i * 2, endian)? as i16).ok())
                .collect(),
            4 | 13 => (0..n).filter_map(|i| u32_at(b, i * 4, endian)).collect(),
            9 => (0..n)
                .filter_map(|i| u32::try_from(u32_at(b, i * 4, endian)? as i32).ok())
                .collect(),
            16 | 18 => (0..n)
                .filter_map(|i| u32::try_from(u64_at(i)?).ok())
                .collect(),
            17 => (0..n)
                .filter_map(|i| u32::try_from(u64_at(i)? as i64).ok())
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn uint(&self, e: &Entry) -> Option<u32> {
        self.uints(e).first().copied()
    }

    /// An ASCII value, trimmed of NULs and whitespace; `None` when empty.
    pub fn ascii(&self, e: &Entry) -> Option<String> {
        if e.typ != 2 && e.typ != 7 && e.typ != 1 {
            return None;
        }
        let b = self.value_bytes(e)?;
        let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
        let s = String::from_utf8_lossy(&b[..end]).trim().to_owned();
        (!s.is_empty()).then_some(s)
    }

    /// Unsigned rationals of a RATIONAL entry.
    pub fn rationals(&self, e: &Entry) -> Vec<(u32, u32)> {
        if e.typ != 5 && e.typ != 10 {
            return Vec::new();
        }
        let Some(b) = self.value_bytes(e) else {
            return Vec::new();
        };
        let n = usize::try_from(e.count).unwrap_or(0).min(16);
        (0..n)
            .filter_map(|i| {
                Some((
                    u32_at(b, i * 8, self.endian)?,
                    u32_at(b, i * 8 + 4, self.endian)?,
                ))
            })
            .collect()
    }
}

/// Bytes read past an IFD offset whose table is not known yet: covers a table of 340 entries and, usually, the
/// values written right after it, in one read.
pub const DIRECTORY_READ_AHEAD: u64 = 4096;

/// Total bytes the targeted directory reads of one probe may fetch (1 MiB, ADR-0084).
pub const DIRECTORY_READ_MAX_BYTES: u64 = 1024 * 1024;

/// Targeted reads of one probe at most (each a seek on a disk).
pub const DIRECTORY_MAX_READS: u32 = 64;

/// The largest out-of-line value fetched for `tag`, or `None` for values never fetched: large blobs nothing
/// in the probe or the decoder reads (XMP, ICC profiles, IPTC, Photoshop resources and layers, maker notes).
/// Strip and tile offset/byte-count arrays may be long; other values are small.
pub fn wanted_value(tag: u16) -> Option<u64> {
    match tag {
        0x02BC | 0x8773 | 0x83BB | 0x8649 | 0x935C | 0x927C | 0xC612..=0xC7FF => None,
        0x0111 | 0x0117 | 0x0144 | 0x0145 => Some(DIRECTORY_READ_MAX_BYTES / 2),
        _ => Some(64 * 1024),
    }
}

/// The result of [`load_directories`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedDirectories {
    /// The fetched ranges, at absolute offsets.
    pub segments: Vec<Segment>,
    /// Bytes fetched in total (at most [`DIRECTORY_READ_MAX_BYTES`]).
    pub bytes_read: u64,
    /// Targeted reads made (at most [`DIRECTORY_MAX_READS`]).
    pub reads: u32,
    /// Whether every directory and wanted value is now known (false when a bound stopped the reads).
    pub complete: bool,
}

/// The ranges a plain TIFF block still needs: IFD tables and wanted values the view lacks, clamped to the
/// file, merged when they touch or lie within [`DIRECTORY_READ_AHEAD`] of each other, in file order.
pub fn missing_ranges(view: View<'_>, file_size: u64) -> Vec<(u64, u64)> {
    let Some(t) = Tiff::parse_view(view) else {
        return Vec::new();
    };
    let mut misses = Vec::new();
    t.walk_noting(Some(&mut misses));
    let mut ranges: Vec<(u64, u64)> = misses
        .into_iter()
        .filter(|&(off, _)| off >= 8 && off < file_size)
        .map(|(off, len)| (off, off.saturating_add(len).min(file_size)))
        .collect();
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1.saturating_add(DIRECTORY_READ_AHEAD) => {
                last.1 = last.1.max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    merged.into_iter().map(|(s, e)| (s, e - s)).collect()
}

/// Fetch, with targeted reads through `fetch(offset, len)`, the directories of a TIFF block whose first
/// `head` bytes are known: the chain, Exif/GPS/Interop and SubIFDs, and their wanted values (ADR-0084). Rounds
/// repeat while new directories become known; the total is bounded by [`DIRECTORY_READ_MAX_BYTES`] and
/// [`DIRECTORY_MAX_READS`], a range is never requested twice, and the IFD walk's own bounds and cycle guard
/// apply. A fetch error stops the load and is returned.
pub fn load_directories<E>(
    head: &[u8],
    file_size: u64,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
) -> Result<LoadedDirectories, E> {
    let mut out = LoadedDirectories::default();
    let mut requested: Vec<(u64, u64)> = Vec::new();
    loop {
        let wanted: Vec<(u64, u64)> = missing_ranges(View::new(head, &out.segments), file_size)
            .into_iter()
            .filter(|r| !requested.contains(r))
            .collect();
        if wanted.is_empty() {
            out.complete = missing_ranges(View::new(head, &out.segments), file_size).is_empty();
            return Ok(out);
        }
        for (off, len) in wanted {
            requested.push((off, len));
            if out.reads >= DIRECTORY_MAX_READS
                || out.bytes_read.saturating_add(len) > DIRECTORY_READ_MAX_BYTES
            {
                return Ok(out);
            }
            let bytes = fetch(off, usize::try_from(len).unwrap_or(usize::MAX))?;
            out.reads += 1;
            out.bytes_read += bytes.len() as u64;
            if !bytes.is_empty() {
                out.segments.push(Segment { offset: off, bytes });
            }
        }
    }
}

/// Largest [`Tiff::metadata_block`] (1 MiB): IFD tables (at most 4 x 1,024 entries) plus values of at most 64 KB
/// each; values beyond the budget are left out.
pub const METADATA_BLOCK_MAX_BYTES: usize = 1024 * 1024;

impl Tiff<'_> {
    /// A compact TIFF holding only the metadata directories (IFD0, Exif, GPS, Interop) and the values known
    /// in the view, without strip/tile arrays, sub-images or blobs, offsets rewritten from header to end. It
    /// lets the EXIF parser read the tags of a TIFF whose directories were fetched by targeted reads, without
    /// the gigabytes of pixel data between them (ADR-0084).
    pub fn metadata_block(&self) -> Option<Vec<u8>> {
        let (ifds, bad) = self.walk();
        if bad {
            return None;
        }
        let find = |k: IfdKind| ifds.iter().find(|i| i.kind == k);
        let ifd0 = find(IfdKind::Chain(0))?;
        let keep = |e: &Entry| {
            !matches!(
                e.tag,
                0x0111
                    | 0x0117
                    | 0x0144
                    | 0x0145
                    | 0x014A
                    | 0x8769
                    | 0x8825
                    | 0xA005
                    | 0x0201
                    | 0x0202
            ) && wanted_value(e.tag).is_some()
        };
        // Values: each at most what the probe would fetch for its tag (64 KB), one per tag (the first), and all
        // together within the block budget, leaving room for the tables.
        let budget =
            std::cell::Cell::new(METADATA_BLOCK_MAX_BYTES - 4 * (2 + 12 * MAX_ENTRIES + 4) - 64);
        let entries = |ifd: Option<&IfdView>| -> Vec<(u16, u16, u32, Vec<u8>)> {
            let mut out: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
            for e in ifd.map(|i| i.entries.as_slice()).unwrap_or_default() {
                if !keep(e) || out.iter().any(|o| o.0 == e.tag) {
                    continue;
                }
                let Some(v) = self.value_bytes(e) else {
                    continue;
                };
                let size = v.len().max(4) + 1;
                if v.len() as u64 > wanted_value(e.tag).unwrap_or(0) || size > budget.get() {
                    continue;
                }
                budget.set(budget.get() - size);
                out.push((e.tag, e.typ, e.count, v.to_vec()));
            }
            out
        };
        let mut w = BlockWriter {
            out: Vec::with_capacity(4096),
            endian: self.endian,
        };
        w.out.extend_from_slice(match self.endian {
            Endian::Little => b"II\x2A\x00",
            Endian::Big => b"MM\x00\x2A",
        });
        w.put_u32(8);
        let exif = find(IfdKind::Exif);
        let gps = find(IfdKind::Gps);
        let interop = find(IfdKind::Interop);
        let pointer = |list: &mut Vec<(u16, u16, u32, Vec<u8>)>, tag: u16| {
            list.retain(|e| e.0 != tag);
            list.push((tag, 4, 1, vec![0; 4]));
        };
        let mut e0 = entries(Some(ifd0));
        if exif.is_some() {
            pointer(&mut e0, 0x8769);
        }
        if gps.is_some() {
            pointer(&mut e0, 0x8825);
        }
        let (_, fields0) = w.write_ifd(e0);
        let field =
            |fields: &[(u16, usize)], tag: u16| fields.iter().find(|f| f.0 == tag).map(|f| f.1);
        if exif.is_some() {
            let mut ex = entries(exif);
            if interop.is_some() {
                pointer(&mut ex, 0xA005);
            }
            let (at, fields) = w.write_ifd(ex);
            if let Some(f) = field(&fields0, 0x8769) {
                w.patch(f, at);
            }
            if interop.is_some() {
                let (at, _) = w.write_ifd(entries(interop));
                if let Some(f) = field(&fields, 0xA005) {
                    w.patch(f, at);
                }
            }
        }
        if gps.is_some() {
            let (at, _) = w.write_ifd(entries(gps));
            if let Some(f) = field(&fields0, 0x8825) {
                w.patch(f, at);
            }
        }
        Some(w.out)
    }
}

/// Writes the compact block of [`Tiff::metadata_block`].
struct BlockWriter {
    out: Vec<u8>,
    endian: Endian,
}

impl BlockWriter {
    fn put_u16(&mut self, v: u16) {
        self.out.extend_from_slice(&match self.endian {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        });
    }

    fn put_u32(&mut self, v: u32) {
        self.out.extend_from_slice(&match self.endian {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        });
    }

    fn offset(&self) -> u32 {
        u32::try_from(self.out.len()).unwrap_or(u32::MAX)
    }

    /// Write an IFD (word-aligned, entries sorted by tag, values after the table); returns its offset and each
    /// tag's value-field position.
    fn write_ifd(
        &mut self,
        mut entries: Vec<(u16, u16, u32, Vec<u8>)>,
    ) -> (u32, Vec<(u16, usize)>) {
        entries.sort_by_key(|e| e.0);
        let n = entries.len().min(MAX_ENTRIES);
        entries.truncate(n);
        if self.out.len() % 2 == 1 {
            self.out.push(0);
        }
        let start = self.offset();
        self.put_u16(n as u16);
        let table = self.out.len();
        self.out.resize(table + 12 * n + 4, 0);
        let mut fields = Vec::with_capacity(n);
        for (i, (tag, typ, count, bytes)) in entries.into_iter().enumerate() {
            let at = table + 12 * i;
            let (t, c) = match self.endian {
                Endian::Little => (tag.to_le_bytes(), typ.to_le_bytes()),
                Endian::Big => (tag.to_be_bytes(), typ.to_be_bytes()),
            };
            self.out[at..at + 2].copy_from_slice(&t);
            self.out[at + 2..at + 4].copy_from_slice(&c);
            let cnt = match self.endian {
                Endian::Little => count.to_le_bytes(),
                Endian::Big => count.to_be_bytes(),
            };
            self.out[at + 4..at + 8].copy_from_slice(&cnt);
            fields.push((tag, at + 8));
            if bytes.len() <= 4 {
                self.out[at + 8..at + 8 + bytes.len()].copy_from_slice(&bytes);
            } else {
                if self.out.len() % 2 == 1 {
                    self.out.push(0);
                }
                let off = self.offset();
                self.out.extend_from_slice(&bytes);
                self.patch(at + 8, off);
            }
        }
        (start, fields)
    }

    fn patch(&mut self, at: usize, v: u32) {
        let b = match self.endian {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        };
        self.out[at..at + 4].copy_from_slice(&b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_and_cycles() {
        assert!(Tiff::parse(b"XX").is_none());
        // IFD0 at 8 with one entry, next IFD pointing back to itself.
        let mut d = b"II\x2A\x00\x08\x00\x00\x00".to_vec();
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&0x0112u16.to_le_bytes());
        d.extend_from_slice(&3u16.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&6u32.to_le_bytes());
        d.extend_from_slice(&8u32.to_le_bytes());
        let t = Tiff::parse(&d).expect("header");
        let (ifds, bad) = t.walk();
        assert!(!bad);
        assert_eq!(ifds.len(), 1);
        let e = ifds[0].get(0x0112).copied().expect("orientation");
        assert_eq!(t.uint(&e), Some(6));
    }

    #[test]
    fn huge_counts_do_not_allocate_or_panic() {
        let mut d = b"MM\x00\x2A\x00\x00\x00\x08".to_vec();
        d.extend_from_slice(&1u16.to_be_bytes());
        d.extend_from_slice(&0x010Fu16.to_be_bytes());
        d.extend_from_slice(&2u16.to_be_bytes());
        d.extend_from_slice(&u32::MAX.to_be_bytes());
        d.extend_from_slice(&u32::MAX.to_be_bytes());
        d.extend_from_slice(&0u32.to_be_bytes());
        let t = Tiff::parse(&d).expect("header");
        let (ifds, _) = t.walk();
        let e = ifds[0].entries[0];
        assert_eq!(t.ascii(&e), None);
        assert!(t.uints(&e).is_empty());
    }

    #[test]
    fn uints_read_every_integer_type() {
        // One IFD with an entry per type, each value out of line at 200.
        let cases: [(u16, Vec<u8>, Vec<u32>); 9] = [
            (1, vec![7, 9], vec![7, 9]),
            (6, vec![7, 0xF9], vec![7]),
            (
                3,
                [1600u16, 1200]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
                vec![1600, 1200],
            ),
            (
                8,
                [1600i16, -2].iter().flat_map(|v| v.to_le_bytes()).collect(),
                vec![1600],
            ),
            (
                4,
                [70_000u32, 3]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
                vec![70_000, 3],
            ),
            (
                9,
                [70_000i32, -3]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
                vec![70_000],
            ),
            (
                16,
                [1600u64, 1 << 40]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
                vec![1600],
            ),
            (
                17,
                [1200i64, -1].iter().flat_map(|v| v.to_le_bytes()).collect(),
                vec![1200],
            ),
            (
                18,
                [512u64, 9].iter().flat_map(|v| v.to_le_bytes()).collect(),
                vec![512, 9],
            ),
        ];
        for (typ, value, want) in cases {
            let mut d = b"II\x2A\x00\x08\x00\x00\x00".to_vec();
            d.extend_from_slice(&1u16.to_le_bytes());
            d.extend_from_slice(&0x0100u16.to_le_bytes());
            d.extend_from_slice(&typ.to_le_bytes());
            d.extend_from_slice(&2u32.to_le_bytes());
            let inline = value.len() <= 4;
            if inline {
                let mut v = value.clone();
                v.resize(4, 0);
                d.extend_from_slice(&v);
            } else {
                d.extend_from_slice(&200u32.to_le_bytes());
            }
            d.extend_from_slice(&0u32.to_le_bytes());
            d.resize(200, 0);
            d.extend_from_slice(&value);
            let t = Tiff::parse(&d).expect("header");
            let (ifds, bad) = t.walk();
            assert!(!bad, "type {typ}");
            let e = ifds[0].get(0x0100).copied().expect("entry");
            assert_eq!(t.uints(&e), want, "type {typ}");
        }
    }

    /// A little-endian IFD0 at 8 whose `n` entries `(tag, type, count, offset)` come from `entry(i)`.
    fn ifd_with(n: usize, entry: impl Fn(usize) -> (u16, u16, u32, u32)) -> Vec<u8> {
        let mut d = b"II\x2A\x00\x08\x00\x00\x00".to_vec();
        d.extend_from_slice(&(n as u16).to_le_bytes());
        for i in 0..n {
            let (tag, typ, count, value) = entry(i);
            d.extend_from_slice(&tag.to_le_bytes());
            d.extend_from_slice(&typ.to_le_bytes());
            d.extend_from_slice(&count.to_le_bytes());
            d.extend_from_slice(&value.to_le_bytes());
        }
        d.extend_from_slice(&0u32.to_le_bytes());
        d
    }

    #[test]
    fn scenario_media_metadata_metadata_of_a_large_tiff_gathered_in_bounded_memory() {
        // 1000 ImageDescription entries sharing one 200 KB value, and a 60 KB Artist: one description at most
        // (64 KB per value: this one is left out), duplicates dropped, the block small.
        let at = 16 * 1024u32;
        let mut d = ifd_with(1000, |i| match i {
            0 => (0x0100, 4, 1, 640),
            1 => (0x0101, 4, 1, 480),
            2 => (0x013B, 2, 60_000, at + 200_000),
            _ => (0x010E, 2, 200_000, at),
        });
        d.resize(at as usize, 0);
        d.extend(std::iter::repeat_n(b'x', 260_000));
        let t = Tiff::parse(&d).expect("header");
        let block = t.metadata_block().expect("block");
        assert!(block.len() < 70_000, "{} bytes", block.len());
        let b = Tiff::parse(&block).expect("block parses");
        let (ifds, bad) = b.walk();
        assert!(!bad);
        let tags: Vec<u16> = ifds[0].entries.iter().map(|e| e.tag).collect();
        assert_eq!(tags, vec![0x0100, 0x0101, 0x013B]);
        // Many distinct 60 KB values: the block stops at 1 MiB.
        let mut d = ifd_with(1000, |i| (0x8000 + i as u16, 2, 60_000, at));
        d.resize(at as usize, 0);
        d.extend(std::iter::repeat_n(b'x', 60_000));
        let block = Tiff::parse(&d)
            .expect("header")
            .metadata_block()
            .expect("block");
        assert!(
            block.len() <= METADATA_BLOCK_MAX_BYTES,
            "{} bytes",
            block.len()
        );
        assert!(
            block.len() > METADATA_BLOCK_MAX_BYTES / 2,
            "{} bytes",
            block.len()
        );
    }
}
