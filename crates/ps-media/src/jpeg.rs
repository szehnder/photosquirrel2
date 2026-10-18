//! JPEG marker walking: SOF dimensions, EXIF APP1 location, quantization tables, truncation detection.

use crate::tiff::be_u16;

/// Frame header facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sof {
    pub marker: u8,
    pub width: u16,
    pub height: u16,
    pub components: u8,
}

impl Sof {
    /// Lossless (SOF3/7/11/15) frames are RAW sensor data in camera files, never previews.
    pub fn is_lossless(&self) -> bool {
        matches!(self.marker, 0xC3 | 0xC7 | 0xCB | 0xCF)
    }
}

/// Problems found while walking markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JpegIssue {
    Truncated(String),
    Corrupt(String),
}

/// What a marker walk found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JpegScan {
    pub sof: Option<Sof>,
    /// Offset and length of the TIFF block inside the first `APP1 Exif` segment.
    pub exif_tiff: Option<(usize, usize)>,
    /// Quantization tables by id (0-3), values in zigzag order.
    pub dqt: [Option<[u16; 64]>; 4],
    /// Offset of the first SOS marker.
    pub sos: Option<usize>,
    /// An EOI marker was found after SOS (only searched when the whole file is in `buf`).
    pub eoi_found: bool,
    pub issue: Option<JpegIssue>,
}

fn is_sof(m: u8) -> bool {
    matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC)
}

/// Walk the markers of the JPEG in `buf` (a prefix of a file of `file_size` bytes).
pub fn scan(buf: &[u8], file_size: u64) -> JpegScan {
    let mut s = JpegScan::default();
    let whole = buf.len() as u64 >= file_size;
    if buf.len() < 4 || buf[0] != 0xFF || buf[1] != 0xD8 {
        s.issue = Some(JpegIssue::Corrupt("missing start-of-image marker".into()));
        return s;
    }
    let mut pos = 2usize;
    let mut segments = 0;
    loop {
        segments += 1;
        if segments > 10_000 {
            s.issue = Some(JpegIssue::Corrupt("too many segments".into()));
            return s;
        }
        // Skip fill bytes.
        while buf.get(pos) == Some(&0xFF) && buf.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let (Some(&ff), Some(&m)) = (buf.get(pos), buf.get(pos + 1)) else {
            if whole {
                s.issue = Some(JpegIssue::Truncated(format!(
                    "file ends at byte {} before the image data",
                    buf.len()
                )));
            }
            return s;
        };
        if ff != 0xFF {
            s.issue = Some(JpegIssue::Corrupt(format!(
                "expected a marker at byte {pos}"
            )));
            return s;
        }
        match m {
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                pos += 2;
                continue;
            }
            0xD9 => {
                s.issue = Some(JpegIssue::Corrupt(
                    "end-of-image before any image data".into(),
                ));
                return s;
            }
            _ => {}
        }
        let Some(len) = be_u16(buf, pos + 2).map(usize::from) else {
            if whole {
                s.issue = Some(JpegIssue::Truncated(format!(
                    "file ends inside the marker at byte {pos}"
                )));
            }
            return s;
        };
        if len < 2 {
            s.issue = Some(JpegIssue::Corrupt(format!(
                "invalid segment length at byte {pos}"
            )));
            return s;
        }
        let body = pos + 4;
        let end = pos + 2 + len;
        if end as u64 > file_size {
            s.issue = Some(JpegIssue::Truncated(format!(
                "file ends at byte {file_size} inside a segment that needs {end} bytes"
            )));
            return s;
        }
        let seg = buf.get(body..end.min(buf.len())).unwrap_or(&[]);
        match m {
            0xE1 if s.exif_tiff.is_none() && seg.starts_with(b"Exif\0") && seg.len() > 6 => {
                s.exif_tiff = Some((body + 6, len - 2 - 6));
            }
            0xDB => parse_dqt(seg, &mut s.dqt),
            m if is_sof(m) && s.sof.is_none() => {
                if let (Some(&_p), Some(h), Some(w), Some(&c)) =
                    (seg.first(), be_u16(seg, 1), be_u16(seg, 3), seg.get(5))
                {
                    s.sof = Some(Sof {
                        marker: m,
                        width: w,
                        height: h,
                        components: c,
                    });
                }
            }
            0xDA => {
                s.sos = Some(pos);
                if s.sof.is_none() {
                    s.issue = Some(JpegIssue::Corrupt(
                        "image data before any frame header".into(),
                    ));
                } else if whole {
                    s.eoi_found = has_eoi_after(buf, end);
                    if !s.eoi_found {
                        s.issue = Some(JpegIssue::Truncated(format!(
                            "no end-of-image marker; the image data stops at byte {}",
                            buf.len()
                        )));
                    }
                }
                return s;
            }
            _ => {}
        }
        if end > buf.len() {
            // The rest lies beyond the probe window.
            return s;
        }
        pos = end;
    }
}

/// An EOI (FFD9) exists after `from`. Entropy-coded data stuffs 0xFF as FF00, so FFD9 only appears as a
/// real marker.
fn has_eoi_after(buf: &[u8], from: usize) -> bool {
    buf.get(from..)
        .is_some_and(|b| b.windows(2).any(|w| w[0] == 0xFF && w[1] == 0xD9))
}

fn parse_dqt(seg: &[u8], out: &mut [Option<[u16; 64]>; 4]) {
    let mut p = 0;
    while p < seg.len() {
        let pq = seg[p] >> 4;
        let tq = usize::from(seg[p] & 0xF);
        p += 1;
        let mut t = [0u16; 64];
        let size = if pq == 0 { 64 } else { 128 };
        let Some(vals) = seg.get(p..p + size) else {
            return;
        };
        for (i, v) in t.iter_mut().enumerate() {
            *v = if pq == 0 {
                u16::from(vals[i])
            } else {
                u16::from_be_bytes([vals[i * 2], vals[i * 2 + 1]])
            };
        }
        if tq < 4 {
            out[tq] = Some(t);
        }
        p += size;
    }
}

/// SOF of a standalone JPEG stream (preview), if its header is inside `buf`.
pub fn sof_of(buf: &[u8]) -> Option<Sof> {
    scan(buf, u64::MAX).sof
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_fixture_jpeg() {
        let j = ps_fixtures::media::jpeg(64, 48, 1, 80);
        let s = scan(&j, j.len() as u64);
        assert_eq!(s.sof.map(|f| (f.width, f.height)), Some((64, 48)));
        assert!(s.eoi_found);
        assert!(s.issue.is_none());
        assert!(s.dqt[0].is_some());
        let t = &j[..j.len() / 2];
        let s = scan(t, t.len() as u64);
        assert!(matches!(s.issue, Some(JpegIssue::Truncated(_))));
    }
}
