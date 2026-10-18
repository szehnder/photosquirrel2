//! Bounded, panic-free ISO base media file format (HEIF, QuickTime, MP4) box parsing.

use crate::tiff::{be_u16, be_u32, be_u64};

/// One box: type, absolute offset of its body and body length (clipped to the buffer when truncated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxRef {
    pub kind: [u8; 4],
    /// Offset of the box header inside the buffer it was read from.
    pub start: usize,
    /// Offset of the body.
    pub body: usize,
    /// Declared body length (may extend past the buffer).
    pub declared_len: u64,
}

impl BoxRef {
    /// The body bytes available in `buf`.
    pub fn body<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let end = (self.body as u64).saturating_add(self.declared_len);
        let end = usize::try_from(end).unwrap_or(usize::MAX).min(buf.len());
        buf.get(self.body..end).unwrap_or(&[])
    }

    /// The whole body lies inside `buf`.
    pub fn complete(&self, buf: &[u8]) -> bool {
        (self.body as u64).saturating_add(self.declared_len) <= buf.len() as u64
    }

    pub fn end(&self) -> u64 {
        (self.body as u64).saturating_add(self.declared_len)
    }
}

/// Iterate the boxes of `buf[start..end]`. Stops at the first malformed header. `size == 0` means "to the
/// end of the enclosing buffer".
pub fn boxes(buf: &[u8], start: usize, end: usize) -> Vec<BoxRef> {
    let end = end.min(buf.len());
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end && out.len() < 4096 {
        let Some(size32) = be_u32(buf, pos) else {
            break;
        };
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&buf[pos + 4..pos + 8]);
        let (header, size) = match size32 {
            0 => (8usize, (end - pos) as u64),
            1 => match be_u64(buf, pos + 8) {
                Some(s) => (16, s),
                None => break,
            },
            s => (8, u64::from(s)),
        };
        if size < header as u64 {
            break;
        }
        out.push(BoxRef {
            kind,
            start: pos,
            body: pos + header,
            declared_len: size - header as u64,
        });
        let next = (pos as u64).saturating_add(size);
        if next > end as u64 {
            break;
        }
        pos = next as usize;
    }
    out
}

/// Top-level boxes of a whole file prefix (size may extend past the prefix for the last box).
pub fn top_level(buf: &[u8]) -> Vec<BoxRef> {
    boxes(buf, 0, buf.len())
}

pub fn find<'a>(list: &'a [BoxRef], kind: &[u8; 4]) -> Option<&'a BoxRef> {
    list.iter().find(|b| &b.kind == kind)
}

/// Children of a box (`skip` bytes of the body are a full-box header or fixed fields).
pub fn children(buf: &[u8], parent: &BoxRef, skip: usize) -> Vec<BoxRef> {
    let body_end = usize::try_from(parent.end())
        .unwrap_or(usize::MAX)
        .min(buf.len());
    boxes(buf, parent.body + skip, body_end)
}

/// Path lookup (`b"trak"`, `b"mdia"`, ...) through plain container boxes.
pub fn find_path(buf: &[u8], root: &[BoxRef], path: &[&[u8; 4]]) -> Option<BoxRef> {
    let (first, rest) = path.split_first()?;
    let mut cur = *find(root, first)?;
    for k in rest {
        let kids = children(buf, &cur, 0);
        cur = *find(&kids, k)?;
    }
    Some(cur)
}

// -------------------------------------------------------------------------------------------------------
// ftyp
// -------------------------------------------------------------------------------------------------------

/// Major and compatible brands of an `ftyp` at the start of `buf`.
pub fn ftyp_brands(buf: &[u8]) -> Option<Vec<[u8; 4]>> {
    if buf.get(4..8)? != b"ftyp" {
        return None;
    }
    let size = usize::try_from(be_u32(buf, 0)?).ok()?;
    if size < 16 {
        return None;
    }
    let end = size.min(buf.len()).min(256);
    let mut out = Vec::new();
    let mut major = [0u8; 4];
    major.copy_from_slice(buf.get(8..12)?);
    out.push(major);
    let mut p = 16;
    while p + 4 <= end {
        let mut b = [0u8; 4];
        b.copy_from_slice(&buf[p..p + 4]);
        out.push(b);
        p += 4;
    }
    Some(out)
}

// -------------------------------------------------------------------------------------------------------
// HEIF
// -------------------------------------------------------------------------------------------------------

/// One HEIF item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeifItem {
    pub id: u32,
    pub kind: [u8; 4],
    /// Absolute file extents (offset, length) for construction method 0.
    pub extents: Vec<(u64, u64)>,
    pub dims: Option<(u32, u32)>,
    /// `irot` angle in units of 90 degrees anti-clockwise.
    pub irot: u8,
}

/// What the HEIF `meta` box says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeifInfo {
    pub primary: Option<u32>,
    pub items: Vec<HeifItem>,
    /// (thumbnail item, master item) from `iref thmb`.
    pub thumbnails: Vec<(u32, u32)>,
    /// (exif item, described item) from `iref cdsc` where the item is `Exif`.
    pub exif_items: Vec<u32>,
}

impl HeifInfo {
    pub fn item(&self, id: u32) -> Option<&HeifItem> {
        self.items.iter().find(|i| i.id == id)
    }
}

fn read_sized(buf: &[u8], pos: usize, size: u8) -> Option<u64> {
    match size {
        0 => Some(0),
        4 => be_u32(buf, pos).map(u64::from),
        8 => be_u64(buf, pos),
        _ => None,
    }
}

/// Parse the top-level `meta` box of a HEIF/AVIF file found in `buf` (a file prefix).
pub fn parse_heif(buf: &[u8]) -> Option<HeifInfo> {
    let top = top_level(buf);
    let meta = find(&top, b"meta")?;
    let kids = children(buf, meta, 4);
    let mut info = HeifInfo::default();

    if let Some(pitm) = find(&kids, b"pitm") {
        let b = pitm.body(buf);
        let version = *b.first()?;
        info.primary = if version == 0 {
            be_u16(b, 4).map(u32::from)
        } else {
            be_u32(b, 4)
        };
    }

    // iinf: item ids and types.
    if let Some(iinf) = find(&kids, b"iinf") {
        let b = iinf.body(buf);
        let version = *b.first()?;
        let skip = if version == 0 { 6 } else { 8 };
        for infe in children(buf, iinf, skip)
            .iter()
            .filter(|c| &c.kind == b"infe")
        {
            let ib = infe.body(buf);
            let v = ib.first().copied().unwrap_or(0);
            if v < 2 {
                continue;
            }
            let (id, type_pos) = if v == 2 {
                (be_u16(ib, 4).map(u32::from), 8)
            } else {
                (be_u32(ib, 4), 10)
            };
            let (Some(id), Some(t)) = (id, ib.get(type_pos..type_pos + 4)) else {
                continue;
            };
            let mut kind = [0u8; 4];
            kind.copy_from_slice(t);
            info.items.push(HeifItem {
                id,
                kind,
                ..Default::default()
            });
        }
    }

    // iloc: extents.
    if let Some(iloc) = find(&kids, b"iloc") {
        let b = iloc.body(buf);
        parse_iloc(b, &mut info);
    }

    // iref: thmb / cdsc.
    if let Some(iref) = find(&kids, b"iref") {
        let b = iref.body(buf);
        let version = b.first().copied().unwrap_or(0);
        let id_size = if version == 0 { 2 } else { 4 };
        let read_id = |d: &[u8], p: usize| -> Option<u32> {
            if id_size == 2 {
                be_u16(d, p).map(u32::from)
            } else {
                be_u32(d, p)
            }
        };
        for r in children(buf, iref, 4) {
            let rb = r.body(buf);
            let Some(from) = read_id(rb, 0) else { continue };
            let Some(count) = be_u16(rb, id_size) else {
                continue;
            };
            let targets: Vec<u32> = (0..usize::from(count).min(64))
                .filter_map(|i| read_id(rb, id_size + 2 + i * id_size))
                .collect();
            match &r.kind {
                b"thmb" => info.thumbnails.extend(targets.iter().map(|t| (from, *t))),
                b"cdsc" if info.item(from).is_some_and(|i| &i.kind == b"Exif") => {
                    info.exif_items.push(from);
                }
                _ => {}
            }
        }
    }
    for it in &info.items {
        if &it.kind == b"Exif" && !info.exif_items.contains(&it.id) {
            info.exif_items.push(it.id);
        }
    }

    // iprp/ipco + ipma: ispe and irot per item.
    if let Some(iprp) = find(&kids, b"iprp") {
        let pk = children(buf, iprp, 0);
        let props: Vec<BoxRef> = find(&pk, b"ipco")
            .map(|ipco| children(buf, ipco, 0))
            .unwrap_or_default();
        for ipma in pk.iter().filter(|b| &b.kind == b"ipma") {
            parse_ipma(buf, ipma, &props, &mut info);
        }
    }
    Some(info)
}

fn parse_iloc(b: &[u8], info: &mut HeifInfo) -> Option<()> {
    let version = *b.first()?;
    let s1 = *b.get(4)?;
    let s2 = *b.get(5)?;
    let (offset_size, length_size) = (s1 >> 4, s1 & 0xF);
    let (base_size, index_size) = (s2 >> 4, if version >= 1 { s2 & 0xF } else { 0 });
    let mut p = 6;
    let count = if version < 2 {
        let c = u32::from(be_u16(b, p)?);
        p += 2;
        c
    } else {
        let c = be_u32(b, p)?;
        p += 4;
        c
    };
    for _ in 0..count.min(4096) {
        let id = if version < 2 {
            let v = u32::from(be_u16(b, p)?);
            p += 2;
            v
        } else {
            let v = be_u32(b, p)?;
            p += 4;
            v
        };
        let mut method = 0;
        if version >= 1 {
            method = be_u16(b, p)? & 0xF;
            p += 2;
        }
        p += 2; // data_reference_index
        let base = read_sized(b, p, base_size)?;
        p += usize::from(base_size);
        let n = be_u16(b, p)?;
        p += 2;
        let mut extents = Vec::new();
        for _ in 0..n.min(256) {
            if index_size > 0 {
                p += usize::from(index_size);
            }
            let off = read_sized(b, p, offset_size)?;
            p += usize::from(offset_size);
            let len = read_sized(b, p, length_size)?;
            p += usize::from(length_size);
            extents.push((base.saturating_add(off), len));
        }
        if method == 0
            && let Some(item) = info.items.iter_mut().find(|i| i.id == id)
        {
            item.extents = extents;
        }
    }
    Some(())
}

fn parse_ipma(buf: &[u8], ipma: &BoxRef, props: &[BoxRef], info: &mut HeifInfo) -> Option<()> {
    let b = ipma.body(buf);
    let version = *b.first()?;
    let flags = be_u32(b, 0)? & 0x00FF_FFFF;
    let mut p = 4;
    let count = be_u32(b, p)?;
    p += 4;
    for _ in 0..count.min(4096) {
        let id = if version < 1 {
            let v = u32::from(be_u16(b, p)?);
            p += 2;
            v
        } else {
            let v = be_u32(b, p)?;
            p += 4;
            v
        };
        let n = *b.get(p)?;
        p += 1;
        for _ in 0..n {
            let idx = if flags & 1 == 1 {
                let v = be_u16(b, p)? & 0x7FFF;
                p += 2;
                usize::from(v)
            } else {
                let v = b.get(p)? & 0x7F;
                p += 1;
                usize::from(v)
            };
            let Some(prop) = idx.checked_sub(1).and_then(|i| props.get(i)) else {
                continue;
            };
            let pb = prop.body(buf);
            let Some(item) = info.items.iter_mut().find(|i| i.id == id) else {
                continue;
            };
            match &prop.kind {
                b"ispe" => {
                    if let (Some(w), Some(h)) = (be_u32(pb, 4), be_u32(pb, 8)) {
                        item.dims = Some((w, h));
                    }
                }
                b"irot" => item.irot = pb.first().copied().unwrap_or(0) & 3,
                _ => {}
            }
        }
    }
    Some(())
}

// -------------------------------------------------------------------------------------------------------
// QuickTime / MP4
// -------------------------------------------------------------------------------------------------------

/// Facts read directly from `moov` (complementing nom-exif's track info).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovieBoxes {
    /// Seconds since 1904-01-01 (QuickTime epoch); 0 means unset.
    pub creation_mac: u64,
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Clockwise rotation in degrees from the video track matrix.
    pub rotation: u16,
    pub frame_rate: Option<f32>,
    pub codec: Option<String>,
}

/// Location of the `moov` box among the top-level boxes of `buf`.
pub fn moov_box(buf: &[u8]) -> Option<BoxRef> {
    top_level(buf).into_iter().find(|b| &b.kind == b"moov")
}

/// Parse `moov` (which must lie entirely inside `buf`).
pub fn parse_movie(buf: &[u8], moov: &BoxRef) -> MovieBoxes {
    let mut m = MovieBoxes::default();
    let kids = children(buf, moov, 0);
    if let Some(mvhd) = find(&kids, b"mvhd") {
        let b = mvhd.body(buf);
        let v = b.first().copied().unwrap_or(0);
        let (created, scale, dur) = if v == 1 {
            (be_u64(b, 4), be_u32(b, 20).map(u64::from), be_u64(b, 24))
        } else {
            (
                be_u32(b, 4).map(u64::from),
                be_u32(b, 12).map(u64::from),
                be_u32(b, 16).map(u64::from),
            )
        };
        m.creation_mac = created.unwrap_or(0);
        if let (Some(s), Some(d)) = (scale, dur)
            && s > 0
        {
            m.duration_ms = Some(d.saturating_mul(1000) / s);
        }
    }
    for trak in kids.iter().filter(|b| &b.kind == b"trak") {
        let tk = children(buf, trak, 0);
        let Some(mdia) = find(&tk, b"mdia") else {
            continue;
        };
        let mk = children(buf, mdia, 0);
        let is_video = find(&mk, b"hdlr").is_some_and(|h| h.body(buf).get(8..12) == Some(b"vide"));
        if !is_video {
            continue;
        }
        if let Some(tkhd) = find(&tk, b"tkhd") {
            let b = tkhd.body(buf);
            let v = b.first().copied().unwrap_or(0);
            let mpos = if v == 1 { 52 } else { 40 };
            let a = be_u32(b, mpos).map(|x| x as i32);
            let bb = be_u32(b, mpos + 4).map(|x| x as i32);
            m.rotation = match (a, bb) {
                (Some(0), Some(x)) if x > 0 => 90,
                (Some(0), Some(x)) if x < 0 => 270,
                (Some(x), Some(0)) if x < 0 => 180,
                _ => 0,
            };
            if let (Some(w), Some(h)) = (be_u32(b, mpos + 36), be_u32(b, mpos + 40)) {
                m.width = Some(w >> 16);
                m.height = Some(h >> 16);
            }
        }
        let mut timescale = None;
        if let Some(mdhd) = find(&mk, b"mdhd") {
            let b = mdhd.body(buf);
            let v = b.first().copied().unwrap_or(0);
            timescale = if v == 1 { be_u32(b, 20) } else { be_u32(b, 12) };
        }
        let stbl = find(&mk, b"minf")
            .map(|minf| children(buf, minf, 0))
            .and_then(|mi| find(&mi, b"stbl").copied());
        if let Some(stbl) = stbl {
            let sk = children(buf, &stbl, 0);
            if let Some(stsd) = find(&sk, b"stsd") {
                let b = stsd.body(buf);
                if let Some(c) = b.get(12..16) {
                    m.codec = Some(String::from_utf8_lossy(c).trim().to_owned());
                }
                if m.width.is_none_or(|w| w == 0)
                    && let (Some(w), Some(h)) = (be_u16(b, 8 + 32), be_u16(b, 8 + 34))
                {
                    m.width = Some(u32::from(w));
                    m.height = Some(u32::from(h));
                }
            }
            if let (Some(stts), Some(ts)) = (find(&sk, b"stts"), timescale) {
                let b = stts.body(buf);
                let n = be_u32(b, 4).unwrap_or(0).min(4096) as usize;
                let (mut samples, mut total) = (0u64, 0u64);
                for i in 0..n {
                    let (Some(c), Some(d)) = (be_u32(b, 8 + i * 8), be_u32(b, 12 + i * 8)) else {
                        break;
                    };
                    samples += u64::from(c);
                    total += u64::from(c) * u64::from(d);
                }
                if total > 0 && ts > 0 {
                    m.frame_rate = Some((samples as f64 * f64::from(ts) / total as f64) as f32);
                }
            }
        }
        break;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_stop_on_garbage() {
        assert!(boxes(&[0, 0, 0, 3, b'a', b'b', b'c', b'd'], 0, 8).is_empty());
        let mut v = 16u32.to_be_bytes().to_vec();
        v.extend_from_slice(b"free");
        v.extend_from_slice(&[0; 8]);
        v.extend_from_slice(&u32::MAX.to_be_bytes());
        v.extend_from_slice(b"mdat");
        let b = boxes(&v, 0, v.len());
        assert_eq!(b.len(), 2);
        assert!(!b[1].complete(&v));
        assert!(b[1].body(&v).is_empty());
    }
}
