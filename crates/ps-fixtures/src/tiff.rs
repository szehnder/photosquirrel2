//! A minimal TIFF IFD writer, shared by the EXIF (APP1) block of camera JPEGs and the DNG writer.
//!
//! An [`Ifd`] is a list of tagged values plus sub-IFD pointers (Exif, GPS, SubIFDs) and out-of-line blobs
//! (EXIF thumbnail, DNG strips). [`write_tiff`] lays the tree out with every offset relative to the TIFF header,
//! entries sorted by tag, values word-aligned, in either byte order.

/// TIFF byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    /// `II` (Canon, Sony, Apple, Fujifilm, Samsung).
    Little,
    /// `MM` (Nikon, Olympus, older Pentax).
    Big,
}

impl Endian {
    fn u16(self, v: u16) -> [u8; 2] {
        match self {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        }
    }

    fn u32(self, v: u32) -> [u8; 4] {
        match self {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        }
    }
}

/// A TIFF field value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(Vec<u8>),
    /// NUL terminator added on write.
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    Undefined(Vec<u8>),
    SRational(Vec<(i32, i32)>),
}

impl Value {
    fn type_and_count(&self) -> (u16, u32) {
        let n = |l: usize| u32::try_from(l).unwrap_or(u32::MAX);
        match self {
            Value::Byte(v) => (1, n(v.len())),
            Value::Ascii(s) => (2, n(s.len() + 1)),
            Value::Short(v) => (3, n(v.len())),
            Value::Long(v) => (4, n(v.len())),
            Value::Rational(v) => (5, n(v.len())),
            Value::Undefined(v) => (7, n(v.len())),
            Value::SRational(v) => (10, n(v.len())),
        }
    }

    fn encode(&self, e: Endian) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Value::Byte(v) | Value::Undefined(v) => out.extend_from_slice(v),
            Value::Ascii(s) => {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Value::Short(v) => v.iter().for_each(|x| out.extend_from_slice(&e.u16(*x))),
            Value::Long(v) => v.iter().for_each(|x| out.extend_from_slice(&e.u32(*x))),
            Value::Rational(v) => v.iter().for_each(|(a, b)| {
                out.extend_from_slice(&e.u32(*a));
                out.extend_from_slice(&e.u32(*b));
            }),
            Value::SRational(v) => v.iter().for_each(|(a, b)| {
                out.extend_from_slice(&e.u32(*a as u32));
                out.extend_from_slice(&e.u32(*b as u32));
            }),
        }
        out
    }
}

/// An image file directory.
#[derive(Clone, Debug, Default)]
pub struct Ifd {
    values: Vec<(u16, Value)>,
    /// (pointer tag, child IFD), e.g. 0x8769 Exif, 0x8825 GPS, 0x014A SubIFDs.
    children: Vec<(u16, Ifd)>,
    /// (offset tag, length tag, bytes), e.g. 0x0201/0x0202 JPEG thumbnail, 0x0111/0x0117 strips.
    blobs: Vec<(u16, u16, Vec<u8>)>,
    next: Option<Box<Ifd>>,
}

impl Ifd {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, tag: u16, value: Value) -> &mut Self {
        self.values.retain(|(t, _)| *t != tag);
        self.values.push((tag, value));
        self
    }

    pub fn ascii(&mut self, tag: u16, s: impl Into<String>) -> &mut Self {
        self.set(tag, Value::Ascii(s.into()))
    }

    pub fn short(&mut self, tag: u16, v: u16) -> &mut Self {
        self.set(tag, Value::Short(vec![v]))
    }

    pub fn long(&mut self, tag: u16, v: u32) -> &mut Self {
        self.set(tag, Value::Long(vec![v]))
    }

    pub fn rational(&mut self, tag: u16, num: u32, den: u32) -> &mut Self {
        self.set(tag, Value::Rational(vec![(num, den)]))
    }

    pub fn child(&mut self, tag: u16, ifd: Ifd) -> &mut Self {
        self.children.retain(|(t, _)| *t != tag);
        self.children.push((tag, ifd));
        self
    }

    pub fn blob(&mut self, offset_tag: u16, length_tag: u16, bytes: Vec<u8>) -> &mut Self {
        self.blobs.push((offset_tag, length_tag, bytes));
        self
    }

    pub fn set_next(&mut self, ifd: Ifd) -> &mut Self {
        self.next = Some(Box::new(ifd));
        self
    }

    pub fn has(&self, tag: u16) -> bool {
        self.values.iter().any(|(t, _)| *t == tag)
    }
}

enum Slot {
    Inline(u16, u32, Vec<u8>),
    Child(usize),
    Blob(usize),
}

/// Serialise `ifd0` (and everything it points to) as a complete TIFF stream (header included).
pub fn write_tiff(ifd0: &Ifd, e: Endian) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096);
    buf.extend_from_slice(match e {
        Endian::Little => b"II",
        Endian::Big => b"MM",
    });
    buf.extend_from_slice(&e.u16(42));
    buf.extend_from_slice(&e.u32(8));
    // IFDs first, then every blob (strips, thumbnails) at the end, as cameras lay files out.
    let mut blobs: Vec<(usize, &[u8])> = Vec::new();
    write_ifd(&mut buf, ifd0, e, &mut blobs);
    for (at, bytes) in blobs {
        align(&mut buf);
        let off = offset(&buf);
        buf.extend_from_slice(bytes);
        patch_u32(&mut buf, at, off, e);
    }
    buf
}

fn patch_u32(buf: &mut [u8], at: usize, v: u32, e: Endian) {
    buf[at..at + 4].copy_from_slice(&e.u32(v));
}

fn offset(buf: &[u8]) -> u32 {
    u32::try_from(buf.len()).unwrap_or(u32::MAX)
}

fn align(buf: &mut Vec<u8>) {
    if buf.len() % 2 == 1 {
        buf.push(0);
    }
}

/// Writes one IFD at the end of `buf`, returns its offset.
fn write_ifd<'a>(
    buf: &mut Vec<u8>,
    ifd: &'a Ifd,
    e: Endian,
    blobs: &mut Vec<(usize, &'a [u8])>,
) -> u32 {
    align(buf);
    let start = buf.len();

    let mut entries: Vec<(u16, Slot)> = Vec::new();
    for (tag, v) in &ifd.values {
        let (ty, count) = v.type_and_count();
        entries.push((*tag, Slot::Inline(ty, count, v.encode(e))));
    }
    for (i, (tag, _)) in ifd.children.iter().enumerate() {
        entries.push((*tag, Slot::Child(i)));
    }
    for (i, (otag, ltag, bytes)) in ifd.blobs.iter().enumerate() {
        entries.push((*otag, Slot::Blob(i)));
        let len = offset(bytes);
        entries.push((*ltag, Slot::Inline(4, 1, e.u32(len).to_vec())));
    }
    entries.sort_by_key(|(t, _)| *t);

    let n = entries.len();
    buf.extend_from_slice(&e.u16(u16::try_from(n).unwrap_or(u16::MAX)));
    let table = buf.len();
    buf.resize(table + 12 * n + 4, 0);

    let mut child_patches = Vec::new();
    let mut blob_patches = Vec::new();
    for (i, (tag, slot)) in entries.iter().enumerate() {
        let at = table + 12 * i;
        buf[at..at + 2].copy_from_slice(&e.u16(*tag));
        match slot {
            Slot::Inline(ty, count, data) => {
                buf[at + 2..at + 4].copy_from_slice(&e.u16(*ty));
                buf[at + 4..at + 8].copy_from_slice(&e.u32(*count));
                if data.len() <= 4 {
                    buf[at + 8..at + 8 + data.len()].copy_from_slice(data);
                } else {
                    align(buf);
                    let off = offset(buf);
                    buf.extend_from_slice(data);
                    patch_u32(buf, at + 8, off, e);
                }
            }
            Slot::Child(ci) => {
                // SubIFDs (0x014A) are conventionally LONG too; one child per tag here.
                buf[at + 2..at + 4].copy_from_slice(&e.u16(4));
                buf[at + 4..at + 8].copy_from_slice(&e.u32(1));
                child_patches.push((at + 8, *ci));
            }
            Slot::Blob(bi) => {
                buf[at + 2..at + 4].copy_from_slice(&e.u16(4));
                buf[at + 4..at + 8].copy_from_slice(&e.u32(1));
                blob_patches.push((at + 8, *bi));
            }
        }
    }
    for (at, bi) in blob_patches {
        blobs.push((at, &ifd.blobs[bi].2));
    }
    for (at, ci) in child_patches {
        let off = write_ifd(buf, &ifd.children[ci].1, e, blobs);
        patch_u32(buf, at, off, e);
    }
    let next_at = table + 12 * n;
    if let Some(next) = &ifd.next {
        let off = write_ifd(buf, next, e, blobs);
        patch_u32(buf, next_at, off, e);
    }
    offset(&buf[..start])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd16(b: &[u8], at: usize, e: Endian) -> u16 {
        let a = [b[at], b[at + 1]];
        match e {
            Endian::Little => u16::from_le_bytes(a),
            Endian::Big => u16::from_be_bytes(a),
        }
    }

    #[test]
    fn entries_are_sorted_and_offsets_resolve() {
        for e in [Endian::Little, Endian::Big] {
            let mut exif = Ifd::new();
            exif.ascii(0x9003, "2014:07:04 18:30:05");
            let mut ifd0 = Ifd::new();
            ifd0.ascii(0x0110, "Model X")
                .ascii(0x010F, "Make")
                .child(0x8769, exif);
            let t = write_tiff(&ifd0, e);
            assert_eq!(rd16(&t, 2, e), 42);
            let n = rd16(&t, 8, e) as usize;
            assert_eq!(n, 3);
            let tags: Vec<u16> = (0..n).map(|i| rd16(&t, 10 + 12 * i, e)).collect();
            assert_eq!(tags, vec![0x010F, 0x0110, 0x8769]);
        }
    }
}
