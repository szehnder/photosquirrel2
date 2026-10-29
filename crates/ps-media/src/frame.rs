//! Frame identity and maker notes kept in `DNGPrivateData` (ADR-0105), read from bytes already in memory.
//!
//! - A DNG's `OriginalRawFileName` (IFD0 tag 0xC68B): the stem of the RAW file it was converted from.
//! - The Canon file number: maker-note tag 0x0008 (`FileNumber`, older bodies), else `FileIndex` and
//!   `DirectoryIndex` inside `CameraInfo` (tag 0x000D) at the offsets of the models in [`CAMERA_INFO`].
//! - Adobe's converter moves the camera's maker note into `DNGPrivateData` (IFD0 tag 0xC634): `Adobe\0`, then
//!   blocks of a 4-byte name and a big-endian length; the `MakN` block holds the note's byte order (`II`/`MM`),
//!   its offset in the original file (big-endian) and the note, whose own offsets refer to that original file.
//!
//! Everything is bounded and panic-free: values outside the bytes given are not read (no extra file reads), and a
//! number outside the camera's range (directory 100-999, index 1-9999) is ignored.

use ps_model::{FrameId, MediaKind, ProbeRecord};

use crate::tiff::{Endian, IfdKind, Segment, Tiff, View, u16_at, u32_at};

/// What the header of a file says about its frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameFacts {
    pub frame_id: Option<FrameId>,
    /// Maker notes present: the Exif IFD's (0x927C) or a `MakN` block in `DNGPrivateData`.
    pub maker_notes: bool,
    /// The maker notes inside `DNGPrivateData` were found.
    pub private_maker_notes: bool,
    /// Camera-original signal from these facts alone: maker notes and no editing-software signature.
    pub camera_original: bool,
}

/// Canon models whose `CameraInfo` holds `FileIndex` and `DirectoryIndex` (byte offsets, both int32u), with the
/// adjustment the camera stores them with: the index is stored minus 1, the directory plus 1 (`dir_plus_one`) or
/// as is. From ExifTool's Canon tables. `names` are matched against the end of the EXIF model.
struct CameraInfoLayout {
    names: &'static [&'static str],
    file_index: usize,
    directory_index: usize,
    dir_plus_one: bool,
}

const CAMERA_INFO: &[CameraInfoLayout] = &[
    CameraInfoLayout {
        names: &["EOS-1D Mark III", "EOS-1Ds Mark III"],
        file_index: 0x172,
        directory_index: 0x17e,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS-1D Mark IV"],
        file_index: 0x22c,
        directory_index: 0x238,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS-1D X"],
        file_index: 0x2d0,
        directory_index: 0x2dc,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 5D Mark II"],
        file_index: 0x1bb,
        directory_index: 0x1c7,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 5D Mark III"],
        file_index: 0x28c,
        directory_index: 0x298,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 6D"],
        file_index: 0x2aa,
        directory_index: 0x2b6,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 7D"],
        file_index: 0x1eb,
        directory_index: 0x1f7,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 40D"],
        file_index: 0x133,
        directory_index: 0x13f,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 50D"],
        file_index: 0x19b,
        directory_index: 0x1a7,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 60D"],
        file_index: 0x1d9,
        directory_index: 0x1e5,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 70D"],
        file_index: 0x2b3,
        directory_index: 0x2bf,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 80D"],
        file_index: 0x4ae,
        directory_index: 0x4ba,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 450D", "REBEL XSi", "Kiss X2"],
        file_index: 0x13f,
        directory_index: 0x133,
        dir_plus_one: false,
    },
    CameraInfoLayout {
        names: &["EOS 500D", "REBEL T1i", "Kiss X3"],
        file_index: 0x1d3,
        directory_index: 0x1df,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 550D", "REBEL T2i", "Kiss X4"],
        file_index: 0x1e4,
        directory_index: 0x1f0,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &[
            "EOS 600D",
            "REBEL T3i",
            "Kiss X5",
            "EOS 1100D",
            "REBEL T3",
            "Kiss X50",
        ],
        file_index: 0x1db,
        directory_index: 0x1e7,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 650D", "REBEL T4i", "Kiss X6i"],
        file_index: 0x270,
        directory_index: 0x27c,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 700D", "REBEL T5i", "Kiss X7i"],
        file_index: 0x274,
        directory_index: 0x280,
        dir_plus_one: true,
    },
    CameraInfoLayout {
        names: &["EOS 1000D", "REBEL XS", "Kiss F"],
        file_index: 0x143,
        directory_index: 0x137,
        dir_plus_one: false,
    },
];

/// The `CameraInfo` layout of a Canon `model` (case-insensitive, matched at the end after a space or `-`).
fn camera_info_layout(model: &str) -> Option<&'static CameraInfoLayout> {
    let m = model.trim().to_ascii_uppercase();
    CAMERA_INFO.iter().find(|l| {
        l.names.iter().any(|n| {
            let n = n.to_ascii_uppercase();
            m == n
                || m.strip_suffix(&n)
                    .is_some_and(|rest| rest.ends_with(' ') || rest.ends_with('-'))
        })
    })
}

/// Largest maker-note IFD read (entries).
const MAX_NOTE_ENTRIES: usize = 512;

/// A maker note: its bytes and where they sat in the file its offsets refer to.
struct Note<'a> {
    bytes: &'a [u8],
    /// Offset of `bytes[0]` in that file.
    base: u64,
    endian: Endian,
}

impl Note<'_> {
    /// The value bytes of an entry at `pos` (inline when at most 4 bytes, else at the offset it holds).
    fn value(&self, pos: usize, typ: u16, count: u32) -> Option<&[u8]> {
        let size = match typ {
            1 | 2 | 6 | 7 => 1usize,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        }
        .checked_mul(usize::try_from(count).ok()?)?;
        if size <= 4 {
            return self.bytes.get(pos + 8..pos + 8 + size);
        }
        let off = u64::from(u32_at(self.bytes, pos + 8, self.endian)?);
        let start = usize::try_from(off.checked_sub(self.base)?).ok()?;
        self.bytes.get(start..start.checked_add(size)?)
    }

    /// The Canon file number this note records, for a body of `model`.
    fn canon_file_number(&self, model: Option<&str>) -> Option<u32> {
        let n = usize::from(u16_at(self.bytes, 0, self.endian)?);
        if n == 0 || n > MAX_NOTE_ENTRIES {
            return None;
        }
        let mut camera_info = None;
        for i in 0..n {
            let pos = 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) = (
                u16_at(self.bytes, pos, self.endian),
                u16_at(self.bytes, pos + 2, self.endian),
                u32_at(self.bytes, pos + 4, self.endian),
            ) else {
                break;
            };
            match tag {
                // An unreadable or implausible FileNumber falls through to CameraInfo.
                0x0008 if typ == 4 && count == 1 => {
                    if let Some(n) = self
                        .value(pos, typ, count)
                        .and_then(|v| u32_at(v, 0, self.endian))
                        .and_then(|v| plausible(v / 10_000, v % 10_000))
                    {
                        return Some(n);
                    }
                }
                0x000D => camera_info = self.value(pos, typ, count),
                _ => {}
            }
        }
        let info = camera_info?;
        let layout = camera_info_layout(model?)?;
        let index = u32_at(info, layout.file_index, self.endian)?.checked_add(1)?;
        let dir = u32_at(info, layout.directory_index, self.endian)?;
        let dir = if layout.dir_plus_one {
            dir.checked_sub(1)?
        } else {
            dir
        };
        plausible(dir, index)
    }
}

impl Note<'_> {
    /// A one-SHORT entry of the note's directory (`ColorSpace` 0x00B4, ADR-0113).
    fn short(&self, wanted: u16) -> Option<u16> {
        let n = usize::from(u16_at(self.bytes, 0, self.endian)?);
        if n == 0 || n > MAX_NOTE_ENTRIES {
            return None;
        }
        (0..n).find_map(|i| {
            let pos = 2 + i * 12;
            let tag = u16_at(self.bytes, pos, self.endian)?;
            let typ = u16_at(self.bytes, pos + 2, self.endian)?;
            let count = u32_at(self.bytes, pos + 4, self.endian)?;
            (tag == wanted && typ == 3 && count >= 1).then_some(())?;
            u16_at(self.value(pos, typ, 1)?, 0, self.endian)
        })
    }
}

/// The colour space a Canon maker note records (tag 0x00B4: 1 sRGB, 2 Adobe RGB; ADR-0113), from the Exif IFD's
/// maker-note entry `e` of `t`. `None` when the note is not in `t`'s bytes or records none.
pub(crate) fn canon_colour_space(t: &Tiff<'_>, e: &crate::tiff::Entry) -> Option<u16> {
    let note = Note {
        bytes: t.value_bytes(e)?,
        base: u64::from(t.value_offset(e)?),
        endian: t.endian,
    };
    note.short(0x00B4)
}

/// The colour space recorded by a bare Canon maker-note directory at the start of `bytes` (a CR3's `CMT3` box
/// holds it as a TIFF whose IFD0 is the note).
pub(crate) fn canon_colour_space_in_ifd(t: &Tiff<'_>, ifd: &crate::tiff::IfdView) -> Option<u16> {
    let e = ifd.get(0x00B4).filter(|e| e.typ == 3 && e.count >= 1)?;
    t.uint(e).and_then(|v| u16::try_from(v).ok())
}

/// `dir * 10000 + index` when both are in a camera's range (folders 100-999, files 0001-9999).
fn plausible(dir: u32, index: u32) -> Option<u32> {
    ((100..=999).contains(&dir) && (1..=9999).contains(&index)).then(|| dir * 10_000 + index)
}

/// The `MakN` block of a `DNGPrivateData` value: (byte order, original offset, note).
fn private_maker_note(data: &[u8]) -> Option<(Endian, u32, &[u8])> {
    let mut rest = data.strip_prefix(b"Adobe\0")?;
    // A handful of blocks at most; each names itself and its length.
    for _ in 0..16 {
        let name = rest.get(..4)?;
        let len = usize::try_from(u32_at(rest, 4, Endian::Big)?).ok()?;
        let body = rest.get(8..8usize.checked_add(len)?)?;
        if name == b"MakN" {
            let endian = match body.get(..2)? {
                b"II" => Endian::Little,
                b"MM" => Endian::Big,
                _ => return None,
            };
            let offset = u32_at(body, 2, Endian::Big)?;
            let note = body.get(6..)?;
            return (!note.is_empty()).then_some((endian, offset, note));
        }
        rest = rest.get(8 + len..)?;
    }
    None
}

/// The TIFF block of a file: the file itself for TIFF-based formats, else the probe's EXIF block.
fn tiff_block<'a>(bytes: &'a [u8], probe: &ProbeRecord) -> Option<&'a [u8]> {
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(bytes);
    }
    let block = probe.exif_range.and_then(|r| r.slice(bytes))?;
    (block.starts_with(b"II") || block.starts_with(b"MM")).then_some(block)
}

/// Read the frame facts of a still image from `bytes` (the analysis buffer, or the probe window on a re-read).
/// Empty facts for anything else, or while the ADR-0105 test hook has frame identity off.
pub fn read_frame_facts(bytes: &[u8], probe: &ProbeRecord) -> FrameFacts {
    if !ps_model::frame::frame_identity_enabled() || probe.kind() != Some(MediaKind::Image) {
        return FrameFacts::default();
    }
    crate::util::contain(|| read(bytes, probe))
        .flatten()
        .unwrap_or_default()
}

/// Largest frame value (`OriginalRawFileName`, `DNGPrivateData`, the Exif maker note) a re-read fetches from
/// beyond the probe window (ADR-0108); a larger one leaves the facts incomplete.
pub const FRAME_VALUE_MAX_BYTES: u64 = 1024 * 1024;

/// The frame facts of a file of `file_size` bytes whose first `head` bytes (the probe window) are known: the
/// one-time re-read of ADR-0105. For a TIFF-based file (CR2, DNG, ...), directories beyond the window are fetched
/// through `fetch(offset, len)` within the ADR-0084 bounds, then the frame values the window lacks, each at most
/// [`FRAME_VALUE_MAX_BYTES`] (ADR-0108). `Ok(None)` when the facts cannot be complete within those bounds: the
/// caller then leaves the row for a later read instead of recording "no frame identity".
pub fn reread_frame_facts<E>(
    head: &[u8],
    file_size: u64,
    probe: &ProbeRecord,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
) -> Result<Option<FrameFacts>, E> {
    Ok(reread_header(head, file_size, probe, fetch, false, 0)?.facts)
}

/// What a one-time header re-read found ([`reread_header`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderReread {
    /// The frame facts ([`reread_frame_facts`]); `None` when they cannot be complete within the bounds.
    pub facts: Option<FrameFacts>,
    /// The RAW structure check ([`crate::raw_check::check_by_reads`]) when asked for; `Complete` otherwise.
    pub structure: crate::raw_check::Structure,
}

/// [`reread_frame_facts`] and, with `structure`, the RAW structure check of ADR-0117 over the same directory reads
/// (one load of the TIFF directories serves both; a file of at most `whole_max` bytes is read whole only when a
/// bound leaves the structure unknown).
pub fn reread_header<E>(
    head: &[u8],
    file_size: u64,
    probe: &ProbeRecord,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
    structure: bool,
    whole_max: u64,
) -> Result<HeaderReread, E> {
    use crate::raw_check::{Structure, check_by_reads, check_loaded};
    let raw = match probe.format {
        Some(ps_model::Format::Raw(r)) if structure => Some(r),
        _ => None,
    };
    let enabled =
        ps_model::frame::frame_identity_enabled() && probe.kind() == Some(MediaKind::Image);
    let tiff_based = head.starts_with(b"II*\0") || head.starts_with(b"MM\0*");
    if !tiff_based || (head.len() as u64) >= file_size {
        // The whole file, or an EXIF block (JPEG APP1, at most 64 KB near the start) the probe found in the window.
        let facts = if !enabled {
            Some(FrameFacts::default())
        } else if probe.exif_range.is_some_and(|r| r.slice(head).is_none()) {
            None
        } else {
            Some(read_frame_facts(head, probe))
        };
        let structure = match raw {
            Some(_) => check_by_reads(head, file_size, probe, fetch, whole_max)?,
            None => Structure::Complete,
        };
        return Ok(HeaderReread { facts, structure });
    }
    let loaded = crate::tiff::load_directories(head, file_size, fetch)?;
    let structure = match raw {
        Some(r) => check_loaded(r, head, &loaded.segments, file_size, fetch, whole_max)?,
        None => Structure::Complete,
    };
    let facts = if !enabled {
        Some(FrameFacts::default())
    } else if !loaded.complete {
        None
    } else {
        tiff_facts(head, file_size, loaded.segments, fetch)?
    };
    Ok(HeaderReread { facts, structure })
}

/// The frame facts of a TIFF-based file from its loaded directories, fetching the frame values they lack.
fn tiff_facts<E>(
    head: &[u8],
    file_size: u64,
    mut segments: Vec<Segment>,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
) -> Result<Option<FrameFacts>, E> {
    let wanted = match crate::util::contain(|| {
        let t = Tiff::parse_view(View::new(head, &segments))?;
        frame_value_misses(&t, file_size)
    })
    .flatten()
    {
        Some(w) => w,
        None => return Ok(None),
    };
    for (off, len) in wanted {
        let bytes = fetch(off, usize::try_from(len).unwrap_or(usize::MAX))?;
        if (bytes.len() as u64) < len {
            return Ok(None);
        }
        segments.push(Segment { offset: off, bytes });
    }
    Ok(
        crate::util::contain(|| read_tiff(&Tiff::parse_view(View::new(head, &segments))?))
            .flatten(),
    )
}

/// The (offset, length) of the frame values the view lacks, or `None` when one is larger than
/// [`FRAME_VALUE_MAX_BYTES`] or lies outside the file.
fn frame_value_misses(t: &Tiff<'_>, file_size: u64) -> Option<Vec<(u64, u64)>> {
    let (ifds, _) = t.walk();
    let mut out = Vec::new();
    for ifd in &ifds {
        for e in &ifd.entries {
            let wanted = matches!(
                (ifd.kind, e.tag),
                (IfdKind::Chain(0), 0xC68B | 0xC634) | (IfdKind::Exif, 0x927C)
            );
            if !wanted || t.value_bytes(e).is_some() {
                continue;
            }
            let size = entry_size(e.typ, e.count)?;
            if size <= 4 {
                continue;
            }
            let off = u64::from(t.value_offset(e)?);
            if size > FRAME_VALUE_MAX_BYTES || off.checked_add(size)? > file_size {
                return None;
            }
            out.push((off, size));
        }
    }
    Some(out)
}

/// Bytes of a value of TIFF type `typ` and `count` (the types frame values use).
fn entry_size(typ: u16, count: u32) -> Option<u64> {
    let unit: u64 = match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    };
    unit.checked_mul(u64::from(count))
}

fn read(bytes: &[u8], probe: &ProbeRecord) -> Option<FrameFacts> {
    let block = tiff_block(bytes, probe)?;
    read_tiff(&Tiff::parse(block)?)
}

fn read_tiff(t: &Tiff<'_>) -> Option<FrameFacts> {
    let (ifds, _) = t.walk();
    let mut make = None;
    let mut model = None;
    let mut software = None;
    let mut raw_name = None;
    let mut exif_note: Option<Note<'_>> = None;
    let mut private: Option<Note<'_>> = None;
    for ifd in &ifds {
        for e in &ifd.entries {
            match (ifd.kind, e.tag) {
                (IfdKind::Chain(0), 0x010F) => make = t.ascii(e),
                (IfdKind::Chain(0), 0x0110) => model = t.ascii(e),
                (IfdKind::Chain(0), 0x0131) => software = t.ascii(e),
                (IfdKind::Chain(0), 0xC68B) => {
                    raw_name = t.ascii(e).as_deref().and_then(FrameId::raw_stem);
                }
                (IfdKind::Chain(0), 0xC634) => {
                    if let Some((endian, offset, note)) =
                        t.value_bytes(e).and_then(private_maker_note)
                    {
                        private = Some(Note {
                            bytes: note,
                            base: u64::from(offset),
                            endian,
                        });
                    }
                }
                (IfdKind::Exif, 0x927C) if e.count > 0 => {
                    let base = t.value_offset(e).map_or(0, u64::from);
                    exif_note = Some(Note {
                        bytes: t.value_bytes(e).unwrap_or_default(),
                        base,
                        endian: t.endian,
                    });
                }
                _ => {}
            }
        }
    }
    let canon = make
        .as_deref()
        .is_some_and(|m| m.to_ascii_lowercase().starts_with("canon"));
    let file_number = if canon {
        exif_note
            .as_ref()
            .and_then(|n| n.canon_file_number(model.as_deref()))
            .or_else(|| {
                private
                    .as_ref()
                    .and_then(|n| n.canon_file_number(model.as_deref()))
            })
    } else {
        None
    };
    let maker_notes = exif_note.is_some() || private.is_some();
    Some(FrameFacts {
        frame_id: FrameId::new(raw_name, file_number),
        maker_notes,
        private_maker_notes: private.is_some(),
        camera_original: maker_notes
            && !software
                .as_deref()
                .is_some_and(crate::metadata::is_editing_software),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_model_names() {
        let l = camera_info_layout("Canon EOS 70D").expect("70D");
        assert_eq!(l.file_index, 0x2b3);
        assert!(camera_info_layout("Canon EOS 5D Mark III").is_some_and(|l| l.file_index == 0x28c));
        assert!(camera_info_layout("Canon EOS 5D Mark II").is_some_and(|l| l.file_index == 0x1bb));
        assert!(camera_info_layout("Canon EOS 760D").is_none());
        assert!(camera_info_layout("Canon EOS 7D Mark II").is_none());
        assert!(camera_info_layout("Canon EOS REBEL T3i").is_some_and(|l| l.file_index == 0x1db));
        assert!(camera_info_layout("Canon EOS REBEL T3").is_some_and(|l| l.file_index == 0x1db));
        assert!(camera_info_layout("Canon EOS-1D X").is_some_and(|l| l.file_index == 0x2d0));
        assert!(camera_info_layout("Canon EOS-1D X Mark II").is_none());
    }

    #[test]
    fn plausible_numbers_only() {
        assert_eq!(plausible(101, 34), Some(1_010_034));
        assert_eq!(plausible(99, 34), None);
        assert_eq!(plausible(101, 0), None);
        assert_eq!(plausible(101, 10_000), None);
    }

    #[test]
    fn private_data_blocks() {
        let mut d = b"Adobe\0".to_vec();
        d.extend_from_slice(b"XXXX");
        d.extend_from_slice(&3u32.to_be_bytes());
        d.extend_from_slice(b"abc");
        d.extend_from_slice(b"MakN");
        d.extend_from_slice(&8u32.to_be_bytes());
        d.extend_from_slice(b"II");
        d.extend_from_slice(&0x1234u32.to_be_bytes());
        d.extend_from_slice(b"nt");
        let (e, off, note) = private_maker_note(&d).expect("MakN");
        assert_eq!((e, off, note), (Endian::Little, 0x1234, &b"nt"[..]));
        // Every truncation is refused or shorter, never a panic.
        for n in 0..d.len() {
            let _ = private_maker_note(&d[..n]);
        }
        assert_eq!(private_maker_note(b"Adobe\0MakN\xff\xff\xff\xff"), None);
        assert_eq!(private_maker_note(b"Nikon\0MakN"), None);
    }
}

#[cfg(test)]
mod camera_info_tests {
    use super::*;

    /// A Canon maker note whose `CameraInfo` (0x000D) records `FileIndex` and `DirectoryIndex` at the EOS 70D's
    /// offsets, placed at `base` in its file, as the owner's 70D writes them (file 101-0034: index 33, dir 102).
    fn note_70d(base: u32, index_raw: u32, dir_raw: u32) -> Vec<u8> {
        let info_len = 0x300u32;
        let mut v = Vec::new();
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&0x000Du16.to_le_bytes());
        v.extend_from_slice(&7u16.to_le_bytes());
        v.extend_from_slice(&info_len.to_le_bytes());
        v.extend_from_slice(&(base + 18).to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        let mut info = vec![0u8; info_len as usize];
        info[0x2b3..0x2b7].copy_from_slice(&index_raw.to_le_bytes());
        info[0x2bf..0x2c3].copy_from_slice(&dir_raw.to_le_bytes());
        v.extend_from_slice(&info);
        v
    }

    /// [`note_70d`] with a `FileNumber` (0x0008) entry of `file_number` before `CameraInfo`.
    fn note_70d_with_tag_8(base: u32, file_number: u32) -> Vec<u8> {
        let info_len = 0x300u32;
        let mut v = Vec::new();
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&0x0008u16.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&file_number.to_le_bytes());
        v.extend_from_slice(&0x000Du16.to_le_bytes());
        v.extend_from_slice(&7u16.to_le_bytes());
        v.extend_from_slice(&info_len.to_le_bytes());
        v.extend_from_slice(&(base + 30).to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        let mut info = vec![0u8; info_len as usize];
        info[0x2b3..0x2b7].copy_from_slice(&33u32.to_le_bytes());
        info[0x2bf..0x2c3].copy_from_slice(&102u32.to_le_bytes());
        v.extend_from_slice(&info);
        v
    }

    #[test]
    fn an_implausible_file_number_tag_falls_through_to_camera_info() {
        for tag_8 in [0, 99_999_999] {
            let bytes = note_70d_with_tag_8(0x100, tag_8);
            let note = Note {
                bytes: &bytes,
                base: 0x100,
                endian: Endian::Little,
            };
            assert_eq!(
                note.canon_file_number(Some("Canon EOS 70D")),
                Some(1_010_034),
                "tag 0x0008 = {tag_8}"
            );
        }
        // A plausible tag 0x0008 decides.
        let bytes = note_70d_with_tag_8(0x100, 1_000_007);
        let note = Note {
            bytes: &bytes,
            base: 0x100,
            endian: Endian::Little,
        };
        assert_eq!(
            note.canon_file_number(Some("Canon EOS 70D")),
            Some(1_000_007)
        );
    }

    #[test]
    fn camera_info_file_number_of_the_eos_70d() {
        let bytes = note_70d(0x1f94, 33, 102);
        let note = Note {
            bytes: &bytes,
            base: 0x1f94,
            endian: Endian::Little,
        };
        assert_eq!(
            note.canon_file_number(Some("Canon EOS 70D")),
            Some(1_010_034)
        );
        // Another model's layout, or none, reads nothing.
        assert_eq!(note.canon_file_number(Some("Canon EOS 77D")), None);
        assert_eq!(note.canon_file_number(None), None);
        // A wrong base points the offset elsewhere: refused, never a panic.
        let off = Note {
            bytes: &bytes,
            base: 0x2000,
            endian: Endian::Little,
        };
        assert_eq!(off.canon_file_number(Some("Canon EOS 70D")), None);
        // Implausible values are ignored.
        let zero = note_70d(0, 0, 0);
        let z = Note {
            bytes: &zero,
            base: 0,
            endian: Endian::Little,
        };
        assert_eq!(z.canon_file_number(Some("Canon EOS 70D")), None);
        for n in 0..bytes.len() {
            let t = Note {
                bytes: &bytes[..n],
                base: 0x1f94,
                endian: Endian::Little,
            };
            let _ = t.canon_file_number(Some("Canon EOS 70D"));
        }
    }
}
