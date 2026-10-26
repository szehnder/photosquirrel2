//! A HEIF re-boxer (ADR-0089): writes an existing HEIC's coded payload again with `irot` and `imir` properties
//! on its primary item, so rotated and mirrored samples exist without any HEVC encoder.
//!
//! Supports what the project's own samples use: `ipma` version 0 with 1-byte associations, `iloc` versions 0
//! and 1 with 4- or 8-byte offsets and base offsets, construction method 0 (file offsets), and `meta` before
//! `mdat`.

fn be32(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(p..p + 4)?.try_into().ok()?))
}

fn be16(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(p..p + 2)?.try_into().ok()?))
}

/// (type, start, size) of the boxes of `b[start..end]` (32-bit sizes only).
fn boxes(b: &[u8], start: usize, end: usize) -> Option<Vec<([u8; 4], usize, usize)>> {
    let mut out = Vec::new();
    let mut p = start;
    while p + 8 <= end {
        let size = be32(b, p)? as usize;
        if size < 8 || p + size > end {
            return None;
        }
        out.push((b.get(p + 4..p + 8)?.try_into().ok()?, p, size));
        p += size;
    }
    Some(out)
}

fn find(list: &[([u8; 4], usize, usize)], kind: &[u8; 4]) -> Option<(usize, usize)> {
    list.iter()
        .find(|(k, _, _)| k == kind)
        .map(|(_, s, n)| (*s, *n))
}

fn set_size(b: &mut [u8], at: usize, size: usize) {
    b[at..at + 4].copy_from_slice(&(size as u32).to_be_bytes());
}

/// `heic` with `irot` (`angle` in units of 90 degrees anti-clockwise, 0-3) and, when `mirror` is set, `imir`
/// (axis 0: about the vertical axis) added to its primary item as essential properties. `None` when the file
/// uses a layout the re-boxer does not handle.
pub fn heif_with_transforms(heic: &[u8], angle: u8, mirror: Option<u8>) -> Option<Vec<u8>> {
    let top = boxes(heic, 0, heic.len())?;
    let (meta, meta_size) = find(&top, b"meta")?;
    if find(&top, b"mdat")?.0 < meta {
        return None;
    }
    let kids = boxes(heic, meta + 12, meta + meta_size)?;
    let (pitm, _) = find(&kids, b"pitm")?;
    if heic[pitm + 8] != 0 {
        return None;
    }
    let primary = be16(heic, pitm + 12)?;
    let (iprp, iprp_size) = find(&kids, b"iprp")?;
    let pk = boxes(heic, iprp + 8, iprp + iprp_size)?;
    let (ipco, ipco_size) = find(&pk, b"ipco")?;
    let props = boxes(heic, ipco + 8, ipco + ipco_size)?;
    let (ipma, ipma_size) = find(&pk, b"ipma")?;
    if be32(heic, ipma + 8)? != 0 {
        return None; // version 0, flags 0 only
    }

    // New property boxes, appended to ipco.
    let mut added = Vec::new();
    let mut new_idx = Vec::new();
    added.extend_from_slice(&9u32.to_be_bytes());
    added.extend_from_slice(b"irot");
    added.push(angle & 3);
    new_idx.push(props.len() as u8 + 1);
    if let Some(axis) = mirror {
        added.extend_from_slice(&9u32.to_be_bytes());
        added.extend_from_slice(b"imir");
        added.push(axis & 1);
        new_idx.push(props.len() as u8 + 2);
    }
    if props.len() + new_idx.len() > 127 {
        return None;
    }

    // ipma with the primary item's associations extended.
    let body = &heic[ipma + 12..ipma + ipma_size];
    let count = be32(body, 0)?;
    let mut new_ipma = body[..4].to_vec();
    let mut p = 4;
    for _ in 0..count {
        let id = be16(body, p)?;
        let n = *body.get(p + 2)?;
        let assoc = body.get(p + 3..p + 3 + n as usize)?;
        new_ipma.extend_from_slice(&id.to_be_bytes());
        if id == primary {
            new_ipma.push(n + new_idx.len() as u8);
            new_ipma.extend_from_slice(assoc);
            new_ipma.extend(new_idx.iter().map(|i| 0x80 | i));
        } else {
            new_ipma.push(n);
            new_ipma.extend_from_slice(assoc);
        }
        p += 3 + n as usize;
    }
    let ipma_growth = new_ipma.len() - body.len();
    let delta = added.len() + ipma_growth;

    // Rebuild: everything up to the end of ipco, the added properties, the rest of iprp with the new ipma, the
    // rest of the file; then fix the sizes and the iloc offsets.
    let ipco_end = ipco + ipco_size;
    let mut out = Vec::with_capacity(heic.len() + delta);
    out.extend_from_slice(&heic[..ipco_end]);
    out.extend_from_slice(&added);
    out.extend_from_slice(&heic[ipco_end..ipma + 12]);
    out.extend_from_slice(&new_ipma);
    out.extend_from_slice(&heic[ipma + ipma_size..]);
    let ipma_at = ipma + added.len();
    set_size(&mut out, ipco, ipco_size + added.len());
    set_size(&mut out, ipma_at, ipma_size + ipma_growth);
    set_size(&mut out, iprp, iprp_size + delta);
    set_size(&mut out, meta, meta_size + delta);

    let (iloc, _) = find(&kids, b"iloc")?;
    // iloc lies before iprp in every sample we write; it does not move.
    if iloc > iprp {
        return None;
    }
    let v = out[iloc + 8];
    let off_size = out[iloc + 12] >> 4;
    let len_size = out[iloc + 12] & 0xF;
    let base_size = out[iloc + 13] >> 4;
    let index_size = if v >= 1 { out[iloc + 13] & 0xF } else { 0 };
    if v > 1 || !matches!(off_size, 4 | 8) || !matches!(base_size, 0 | 4 | 8) || index_size != 0 {
        return None;
    }
    // Add `delta` to the `n`-byte big-endian offset at `q`.
    let shift = |out: &mut Vec<u8>, q: usize, n: u8| -> Option<()> {
        if n == 4 {
            let o = be32(out, q)? as usize + delta;
            out[q..q + 4].copy_from_slice(&(o as u32).to_be_bytes());
        } else {
            let o = u64::from_be_bytes(out.get(q..q + 8)?.try_into().ok()?) + delta as u64;
            out[q..q + 8].copy_from_slice(&o.to_be_bytes());
        }
        Some(())
    };
    let mut q = iloc + 14;
    let items = be16(&out, q)?;
    q += 2;
    for _ in 0..items {
        q += 2; // item id
        if v == 1 {
            if be16(&out, q)? & 0xF != 0 {
                return None;
            }
            q += 2;
        }
        q += 2; // data reference index
        // With a base offset the extents are relative to it: move the base; else move every extent.
        if base_size > 0 {
            shift(&mut out, q, base_size)?;
            q += usize::from(base_size);
        }
        let extents = be16(&out, q)?;
        q += 2;
        for _ in 0..extents {
            if base_size == 0 {
                shift(&mut out, q, off_size)?;
            }
            q += usize::from(off_size) + usize::from(len_size);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

    #[test]
    fn reboxed_payload_is_unchanged_and_moved() {
        let r = heif_with_transforms(TINY, 1, None).expect("rebox");
        assert_eq!(r.len(), TINY.len() + 9 + 1);
        // The mdat payload is byte-identical, just later in the file.
        assert!(r.ends_with(&TINY[636..]));
        let r2 = heif_with_transforms(TINY, 3, Some(0)).expect("rebox");
        assert_eq!(r2.len(), TINY.len() + 18 + 2);
        assert!(heif_with_transforms(b"not a heif", 1, None).is_none());
    }
}

/// `heic` with its image sizes (`ispe` properties) rewritten in place: the first (the primary image of the
/// project's samples) declares `width` x `height`, the others are scaled by the same factors. The coded payload
/// is unchanged, so a real decoder fails on it or decodes the old size; probes, size checks and fake decoders
/// see the new size. Cheap fixtures of huge or panoramic declared sizes (gate review of `heic-system-decoders`).
/// `None` when the file has no `ispe`.
pub fn heif_with_size(heic: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut out = heic.to_vec();
    let mut at = Vec::new();
    let mut i = 0;
    while let Some(p) = out
        .get(i..)
        .and_then(|s| s.windows(4).position(|w| w == b"ispe"))
    {
        let p = i + p;
        // `ispe`: version and flags, then two 32-bit sizes.
        if p + 16 <= out.len() {
            at.push(p + 8);
        }
        i = p + 4;
    }
    let first = *at.first()?;
    let (w0, h0) = (be32(heic, first)?, be32(heic, first + 4)?);
    for p in at {
        let (w, h) = (be32(heic, p)?, be32(heic, p + 4)?);
        let nw = (u64::from(w) * u64::from(width) / u64::from(w0.max(1))).max(1) as u32;
        let nh = (u64::from(h) * u64::from(height) / u64::from(h0.max(1))).max(1) as u32;
        out[p..p + 4].copy_from_slice(&nw.to_be_bytes());
        out[p + 4..p + 8].copy_from_slice(&nh.to_be_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn declared_size_is_rewritten() {
        let sample = include_bytes!("../../ps-sysdecode/data/selftest.heic");
        let b = heif_with_size(sample, 3000, 2000).expect("rewritten");
        assert_eq!(b.len(), sample.len());
        let p = b.windows(4).position(|w| w == b"ispe").unwrap() + 8;
        assert_eq!(be32(&b, p), Some(3000));
        assert_eq!(be32(&b, p + 4), Some(2000));
    }
}
