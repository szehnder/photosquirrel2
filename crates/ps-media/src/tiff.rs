//! A small, bounded, panic-free TIFF/EXIF IFD walker over a byte slice.
//!
//! The probe needs byte offsets (EXIF block, embedded previews) that general EXIF libraries do not expose,
//! and it must stay within the probe window, so this walker is deliberately minimal: it reads entries
//! lazily, never allocates more than the entry list, caps IFD counts and depth, and detects cycles.

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

fn type_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// A TIFF block: `data` starts at the TIFF header ("II*\0" / "MM\0*").
#[derive(Clone, Copy, Debug)]
pub struct Tiff<'a> {
    pub data: &'a [u8],
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
        let endian = match data.get(..2)? {
            b"II" => Endian::Little,
            b"MM" => Endian::Big,
            _ => return None,
        };
        let magic = u16_at(data, 2, endian)?;
        if !matches!(magic, 42 | 0x4F52 | 0x5352 | 0x55) {
            return None;
        }
        let ifd0 = u32_at(data, 4, endian)?;
        Some(Self { data, endian, ifd0 })
    }

    fn read_ifd(&self, offset: u32, kind: IfdKind) -> Option<IfdView> {
        let off = usize::try_from(offset).ok()?;
        let n = usize::from(u16_at(self.data, off, self.endian)?);
        if n == 0 || n > MAX_ENTRIES {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let p = off + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) = (
                u16_at(self.data, p, self.endian),
                u16_at(self.data, p + 2, self.endian),
                u32_at(self.data, p + 4, self.endian),
            ) else {
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
    fn next_ifd(&self, ifd: &IfdView) -> Option<u32> {
        let off = usize::try_from(ifd.offset).ok()?;
        let n = usize::from(u16_at(self.data, off, self.endian)?);
        let next = u32_at(self.data, off + 2 + n * 12, self.endian)?;
        (next != 0).then_some(next)
    }

    /// All IFDs reachable from IFD0 (chain, Exif, GPS, Interop, SubIFDs), bounded and cycle-safe.
    /// Returns the IFDs found and whether IFD0 itself was unreadable.
    pub fn walk(&self) -> (Vec<IfdView>, bool) {
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
            let Some(ifd) = self.read_ifd(off, kind) else {
                if kind == IfdKind::Chain(0) {
                    ifd0_bad = true;
                }
                continue;
            };
            if let IfdKind::Chain(i) = kind
                && let Some(next) = self.next_ifd(&ifd)
                && i < 8
            {
                stack.push((next, IfdKind::Chain(i + 1), depth));
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
            self.data.get(e.field_pos..e.field_pos + size)
        } else {
            let off = usize::try_from(u32_at(self.data, e.field_pos, self.endian)?).ok()?;
            self.data.get(off..off.checked_add(size)?)
        }
    }

    /// Absolute-in-block offset of an entry's out-of-line value (for maker notes etc.).
    pub fn value_offset(&self, e: &Entry) -> Option<u32> {
        u32_at(self.data, e.field_pos, self.endian)
    }

    /// Unsigned integer values of a BYTE/SHORT/LONG entry (at most 64 values).
    pub fn uints(&self, e: &Entry) -> Vec<u32> {
        let Some(b) = self.value_bytes(e) else {
            // Value outside the window: an inline single LONG/SHORT is still readable.
            return match e.typ {
                3 if e.count == 1 => u16_at(self.data, e.field_pos, self.endian)
                    .map(u32::from)
                    .into_iter()
                    .collect(),
                4 | 13 if e.count == 1 => u32_at(self.data, e.field_pos, self.endian)
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            };
        };
        let n = usize::try_from(e.count).unwrap_or(0).min(64);
        match e.typ {
            1 | 7 => b.iter().take(n).map(|v| u32::from(*v)).collect(),
            3 => (0..n)
                .filter_map(|i| u16_at(b, i * 2, self.endian).map(u32::from))
                .collect(),
            4 | 13 => (0..n)
                .filter_map(|i| u32_at(b, i * 4, self.endian))
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
}
