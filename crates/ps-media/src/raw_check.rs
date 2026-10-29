//! Evidence that a RAW file was cut short (`damaged-raw-reason`, ADR-0115), judged by our own code from the file's
//! structure over the whole file in memory: a directory, strip, tile or embedded JPEG that the file records but that
//! ends past its end. rawler's errors cannot tell a cut file from a parse quirk, and when the embedded preview
//! decodes rawler is never asked, so a cut sensor strip behind an intact preview would never be seen.
//!
//! What counts, and only that (a false negative is the priority-1 bug, a false positive only mislabels a file that
//! is still catalogued and exported as is):
//! - TIFF-based RAWs (CR2, NEF, ARW, DNG, ORF, RW2, PEF, ...): IFD0's chain, SubIFDs (0x014A) and the Exif IFD
//!   pointer (0x8769) past the end; `StripOffsets`/`StripByteCounts`, `TileOffsets`/`TileByteCounts` and
//!   `JPEGInterchangeFormat`/`Length` ranges (both tags present, a count of 0 ignored) that end past it. Maker notes
//!   and the values of every other tag are never evidence (real cameras write private tags with odd counts).
//! - CR3: a top-level box whose declared size ends past the file (a `size = 0` box runs to the end: not evidence).
//! - RAF: the header's JPEG, CFA header and CFA ranges.
//!
//! Other RAW formats (MRW, X3F, CRW) get no check.

use ps_model::{Format, ProbeRecord, RawFormat};

use crate::tiff::{IfdKind, IfdView, Tiff, u16_at, u32_at};

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

/// The evidence that `bytes`, the whole RAW file `probe` describes, was cut short; `None` when there is none, when
/// `bytes` is not the whole file (a partial buffer is never judged) or for a format without a check.
pub fn truncation(bytes: &[u8], probe: &ProbeRecord) -> Option<Truncation> {
    let file_size = bytes.len() as u64;
    if file_size != probe.file_size {
        return None;
    }
    let Some(Format::Raw(raw)) = probe.format else {
        return None;
    };
    let mut worst = Worst::new(file_size);
    match raw {
        RawFormat::Cr3 => cr3(bytes, &mut worst),
        RawFormat::Raf => raf(bytes, &mut worst),
        RawFormat::Crw | RawFormat::X3f => {}
        _ => {
            if let Some(t) = Tiff::parse(bytes) {
                tiff(&t, bytes, &mut worst);
            }
        }
    }
    worst.found
}

/// The range that ends furthest past the end of the file.
struct Worst {
    file_size: u64,
    found: Option<Truncation>,
}

impl Worst {
    fn new(file_size: u64) -> Self {
        Self {
            file_size,
            found: None,
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

/// Every value of an integer entry (SHORT, LONG, LONG8, IFD, IFD8), up to [`MAX_RANGES`]; `Err(end)` when the value
/// array itself lies past the end of the file, ending at `end`.
fn values(t: &Tiff<'_>, e: &crate::tiff::Entry) -> Result<Vec<u64>, u64> {
    let Some(size) = crate::tiff::type_size(e.typ) else {
        return Ok(Vec::new());
    };
    if !matches!(e.typ, 3 | 4 | 13 | 16 | 18) {
        return Ok(Vec::new());
    }
    let n = (e.count as usize).min(MAX_RANGES);
    let total = size as u64 * u64::from(e.count);
    let Some(b) = t.value_bytes(e) else {
        // Out of line, past the end (an inline value is always inside the directory).
        let at = t.value_offset(e).map_or(0, u64::from);
        return Err(at.saturating_add(total));
    };
    let endian = t.endian;
    Ok((0..n)
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
        .collect())
}

/// The end of the IFD table at `offset` (count, entries and the next pointer).
fn table_end(bytes: &[u8], endian: crate::tiff::Endian, offset: u64) -> u64 {
    let n = usize::try_from(offset)
        .ok()
        .and_then(|o| u16_at(bytes, o, endian))
        .map_or(0, u64::from);
    offset.saturating_add(2 + 12 * n + 4)
}

fn tiff(t: &Tiff<'_>, bytes: &[u8], worst: &mut Worst) {
    let size = bytes.len() as u64;
    let pointer = |worst: &mut Worst, at: u64| {
        if at >= size {
            worst.range(at.saturating_add(2), "directory");
        } else {
            // The count is inside; the table (count, entries, next pointer) must be too.
            let end = table_end(bytes, t.endian, at);
            let end = if u16_at(bytes, at as usize, t.endian).is_none() {
                at + 2
            } else {
                end
            };
            worst.range(end, "directory");
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
                match values(t, e) {
                    Ok(v) => v.into_iter().take(8).for_each(|p| pointer(worst, p)),
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
            let offs = match values(t, o) {
                Ok(v) => v,
                Err(end) => {
                    worst.range(end, "directory");
                    continue;
                }
            };
            let lens = match values(t, l) {
                Ok(v) => v,
                Err(end) => {
                    worst.range(end, "directory");
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

fn cr3(bytes: &[u8], worst: &mut Worst) {
    for b in crate::bmff::top_level(bytes) {
        let what = if &b.kind == b"mdat" {
            "image data"
        } else {
            "directory"
        };
        worst.range(b.end(), what);
    }
}

fn raf(bytes: &[u8], worst: &mut Worst) {
    use crate::tiff::be_u32;
    for (at, what) in [(84, "preview"), (92, "directory"), (100, "image data")] {
        let (Some(off), Some(len)) = (be_u32(bytes, at), be_u32(bytes, at + 4)) else {
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
}
