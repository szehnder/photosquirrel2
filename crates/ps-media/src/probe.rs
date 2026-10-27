//! The bounded probe (ADR-0010): format by magic number, header dimensions, orientation, EXIF block and
//! embedded preview location, from at most the first 256 KB of a file.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use ps_model::{
    ByteRange, CameraSignals, Dimensions, EmbeddedPreview, Format, Orientation, PreviewSource,
    ProbeError, ProbeErrorKind, ProbeRecord, RawFormat, VideoSummary,
};

use crate::bmff;
use crate::fs::{open_read_only, read_prefix};
use crate::jpeg::{self, JpegIssue};
use crate::tiff::{
    IfdKind, IfdView, LoadedDirectories, Segment, Tiff, View, be_u16, be_u32, u16_at, u32_at,
};
use crate::util::{
    MAC_EPOCH_OFFSET, capture_time_from_unix, contain, parse_exif_datetime, parse_offset,
    parse_subsec,
};

/// The probe never looks past this many bytes (256 KB).
pub const PROBE_LIMIT: usize = 262_144;

/// Probe a file on disk: opens it read-only, reads at most [`PROBE_LIMIT`] bytes, plus the directories of a
/// plain TIFF that keeps them beyond that prefix with bounded targeted reads (ADR-0084). I/O failures become an
/// error record, never an `Err`.
pub fn probe_path(path: &Path) -> ProbeRecord {
    let file = match open_read_only(path) {
        Ok(f) => f,
        Err(e) => return io_error_record(0, &e),
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    probe_seekable(file, size, Some(path))
}

/// Probe from any reader positioned at the start of the file; reads at most [`PROBE_LIMIT`] bytes.
pub fn probe_reader<R: Read>(reader: R, file_size: u64, path: Option<&Path>) -> ProbeRecord {
    match read_prefix(reader, PROBE_LIMIT) {
        Ok(buf) => probe_bytes(&buf, file_size.max(buf.len() as u64), path),
        Err(e) => io_error_record(file_size, &e),
    }
}

/// Probe from a seekable reader positioned at the start of the file: the [`PROBE_LIMIT`] prefix, plus the
/// bounded targeted directory reads of [`probe_with_directory_reads`] for a plain TIFF that needs them.
pub fn probe_seekable<R: Read + Seek>(
    mut reader: R,
    file_size: u64,
    path: Option<&Path>,
) -> ProbeRecord {
    let prefix = match read_prefix(&mut reader, PROBE_LIMIT) {
        Ok(buf) => buf,
        Err(e) => return io_error_record(file_size, &e),
    };
    let size = file_size.max(prefix.len() as u64);
    let mut fetch = |off: u64, len: usize| -> std::io::Result<Vec<u8>> {
        reader.seek(SeekFrom::Start(off))?;
        read_prefix(&mut reader, len)
    };
    match probe_with_directory_reads(&prefix, size, path, &mut fetch) {
        Ok((rec, _)) => rec,
        Err(e) => io_error_record(file_size, &e),
    }
}

/// Probe a file held entirely in memory (the analysis buffer, ADR-0010): [`probe_bytes`] of its first 256 KB,
/// except that a plain TIFF's directories are read wherever they lie in the buffer (ADR-0084).
pub fn probe_buffer(bytes: &[u8], path: Option<&Path>) -> ProbeRecord {
    let size = bytes.len() as u64;
    if !is_plain_tiff(bytes) {
        return probe_bytes(bytes, size, path);
    }
    probe_tiff_view(size, path, View::new(bytes, &[]), size)
}

/// Whether a file starting with `prefix` is a plain TIFF (not a TIFF-based RAW) whose directories, or the
/// values the probe and the decoder need, are not all within the first [`PROBE_LIMIT`] bytes (ADR-0084). RAW
/// files always answer false: their probe stays within the prefix.
pub fn needs_directory_reads(prefix: &[u8], file_size: u64) -> bool {
    let head = &prefix[..prefix.len().min(PROBE_LIMIT)];
    is_plain_tiff(head) && !crate::tiff::missing_ranges(View::new(head, &[]), file_size).is_empty()
}

/// [`probe_bytes`], plus, when [`needs_directory_reads`] says so, the TIFF directories fetched through
/// `fetch(offset, len)` (at most [`crate::tiff::DIRECTORY_READ_MAX_BYTES`] in
/// [`crate::tiff::DIRECTORY_MAX_READS`] reads). Returns the record and the fetched segments (empty when none
/// were needed); a fetch error stops the probe and is returned.
pub fn probe_with_directory_reads<E>(
    prefix: &[u8],
    file_size: u64,
    path: Option<&Path>,
    fetch: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, E>,
) -> Result<(ProbeRecord, LoadedDirectories), E> {
    let head = &prefix[..prefix.len().min(PROBE_LIMIT)];
    if !needs_directory_reads(head, file_size) {
        return Ok((
            probe_bytes(head, file_size, path),
            LoadedDirectories::default(),
        ));
    }
    let loaded = crate::tiff::load_directories(head, file_size, fetch)?;
    let examined = head.len() as u64 + loaded.bytes_read;
    let mut rec = probe_tiff_view(file_size, path, View::new(head, &loaded.segments), examined);
    if !loaded.complete {
        rec.warnings.push(
            "TIFF directories beyond the probe window exceed the directory read bounds; some were not read"
                .into(),
        );
    }
    Ok((rec, loaded))
}

/// The metadata directories of a TIFF known through its prefix and fetched segments, as a compact TIFF block
/// the EXIF parser can read on its own (ADR-0084). `None` when the prefix is not a TIFF or IFD0 is unknown.
pub fn tiff_metadata_block(prefix: &[u8], segments: &[Segment]) -> Option<Vec<u8>> {
    let head = &prefix[..prefix.len().min(PROBE_LIMIT)];
    Tiff::parse_view(View::new(head, segments))?.metadata_block()
}

fn is_plain_tiff(prefix: &[u8]) -> bool {
    let head = &prefix[..prefix.len().min(PROBE_LIMIT)];
    sniff(head) == Ok(Format::Tiff) && Tiff::parse(head).is_some()
}

/// A plain TIFF's probe over `view` (its prefix plus fetched segments, or the whole file). The format and the
/// extension check come from the prefix, as in [`probe_bytes`].
fn probe_tiff_view(
    file_size: u64,
    path: Option<&Path>,
    view: View<'_>,
    examined: u64,
) -> ProbeRecord {
    let mut rec = ProbeRecord::empty(file_size);
    rec.bytes_examined = examined;
    rec.format = Some(Format::Tiff);
    if let Some(p) = path {
        rec.extension_mismatch = extension_mismatch(p, Format::Tiff);
    }
    let ok = contain(|| probe_tiff(view, file_size, Format::Tiff, &mut rec));
    if ok.is_none() {
        rec.error = Some(ProbeError {
            kind: ProbeErrorKind::Corrupt,
            message: "parser failure on malformed data".into(),
        });
    }
    if let Some(d) = rec.stored {
        rec.display = Some(d.oriented(rec.orientation));
    }
    rec
}

fn io_error_record(size: u64, e: &std::io::Error) -> ProbeRecord {
    let mut r = ProbeRecord::empty(size);
    r.unsupported_reason = Some("file could not be read".into());
    r.error = Some(ProbeError {
        kind: ProbeErrorKind::Io,
        message: e.to_string(),
    });
    r
}

/// Probe a file prefix. `prefix` is truncated to [`PROBE_LIMIT`]; `file_size` is the size of the whole
/// file; `path` (optional) is used only for the extension check.
pub fn probe_bytes(prefix: &[u8], file_size: u64, path: Option<&Path>) -> ProbeRecord {
    let buf = &prefix[..prefix.len().min(PROBE_LIMIT)];
    let mut rec = ProbeRecord::empty(file_size.max(buf.len() as u64));
    rec.bytes_examined = buf.len() as u64;
    let file_size = rec.file_size;

    let format = match sniff(buf) {
        Ok(f) => f,
        Err(reason) => {
            rec.unsupported_reason = Some(reason);
            return rec;
        }
    };
    rec.format = Some(format);
    if let Some(p) = path {
        rec.extension_mismatch = extension_mismatch(p, format);
    }

    // Our parsers are panic-free by construction; third-party ones (imagesize, nom-exif) are contained.
    let ok = contain(|| probe_format(buf, file_size, format, &mut rec));
    if ok.is_none() {
        rec.error = Some(ProbeError {
            kind: ProbeErrorKind::Corrupt,
            message: "parser failure on malformed data".into(),
        });
    }
    if let Some(d) = rec.stored {
        rec.display = Some(d.oriented(rec.orientation));
    }
    rec
}

fn probe_format(buf: &[u8], file_size: u64, format: Format, rec: &mut ProbeRecord) {
    match format {
        Format::Jpeg => probe_jpeg(buf, file_size, 0, rec),
        Format::Png => probe_png(buf, file_size, rec),
        Format::Tiff | Format::Raw(_) if Tiff::parse(buf).is_some() => {
            probe_tiff(View::new(buf, &[]), file_size, format, rec)
        }
        Format::Tiff if is_bigtiff(buf) => probe_bigtiff(buf, rec),
        Format::Raw(RawFormat::Raf) => probe_raf(buf, file_size, rec),
        Format::Raw(RawFormat::Cr3) => probe_cr3(buf, rec),
        Format::Heic | Format::Avif => probe_heif(buf, file_size, rec),
        Format::Mov | Format::Mp4 | Format::M4v | Format::ThreeGp => probe_movie(buf, rec),
        Format::Gif => {
            rec.has_palette = true;
            header_dims(buf, rec);
        }
        Format::Webp => probe_webp(buf, rec),
        Format::Bmp => {
            let bpp = u16_at(buf, 28, crate::tiff::Endian::Little).unwrap_or(24);
            rec.has_palette = bpp <= 8;
            rec.has_alpha = bpp == 32;
            header_dims(buf, rec);
        }
        Format::Ico => {
            rec.has_alpha = true;
            header_dims(buf, rec);
        }
        Format::Psd => {
            if let (Some(h), Some(w)) = (be_u32(buf, 14), be_u32(buf, 18)) {
                rec.stored = Some(Dimensions::new(w, h));
            }
        }
        Format::Svg => rec.has_alpha = true,
        _ => {}
    }
}

/// A BigTIFF header (version 43 with 8-byte offsets), either byte order.
pub fn is_bigtiff(b: &[u8]) -> bool {
    b.starts_with(b"II\x2B\x00\x08\x00\x00\x00") || b.starts_with(b"MM\x00\x2B\x00\x08\x00\x00")
}

/// The first directory of a BigTIFF within the probe window: width, height and orientation. The file is not
/// decoded (a placeholder names the variant), but its dimensions make it a picture in the catalog.
fn probe_bigtiff(buf: &[u8], rec: &mut ProbeRecord) {
    let le = buf.starts_with(b"II");
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = buf.get(o..o + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u64_at = |o: usize| -> Option<u64> {
        let b: [u8; 8] = buf.get(o..o + 8)?.try_into().ok()?;
        Some(if le {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        })
    };
    let read = || -> Option<(u32, u32, u16)> {
        let ifd = usize::try_from(u64_at(8)?).ok()?;
        let n = usize::try_from(u64_at(ifd)?).ok()?.min(4096);
        let (mut w, mut h, mut orientation) = (None, None, 1u16);
        for i in 0..n {
            let e = ifd + 8 + i * 20;
            let tag = u16_at(e)?;
            let ty = u16_at(e + 2)?;
            // An inline SHORT, LONG or LONG8 value.
            let value = match ty {
                3 => u64::from(u16_at(e + 12)?),
                // A LONG is the first four bytes of the value field.
                4 if le => u64_at(e + 12)? & 0xFFFF_FFFF,
                4 => u64_at(e + 12)? >> 32,
                16 => u64_at(e + 12)?,
                _ => continue,
            };
            match tag {
                256 => w = u32::try_from(value).ok(),
                257 => h = u32::try_from(value).ok(),
                274 => orientation = u16::try_from(value).unwrap_or(1),
                _ => {}
            }
        }
        Some((w?, h?, orientation))
    };
    match read() {
        Some((w, h, o)) if w > 0 && h > 0 => {
            rec.stored = Some(Dimensions::new(w, h));
            if (1..=8).contains(&o) {
                rec.orientation = Orientation(o as u8);
            }
        }
        _ => {
            rec.error.get_or_insert(ProbeError {
                kind: ProbeErrorKind::Truncated,
                message: "BigTIFF directory beyond the probe window".into(),
            });
        }
    }
}

fn header_dims(buf: &[u8], rec: &mut ProbeRecord) {
    match imagesize::blob_size(buf) {
        Ok(s) if s.width > 0 && s.height > 0 => {
            rec.stored = Some(Dimensions::new(
                u32::try_from(s.width).unwrap_or(u32::MAX),
                u32::try_from(s.height).unwrap_or(u32::MAX),
            ));
        }
        Ok(_) | Err(_) => {
            rec.error.get_or_insert(ProbeError {
                kind: if buf.len() < 32 {
                    ProbeErrorKind::Truncated
                } else {
                    ProbeErrorKind::Corrupt
                },
                message: "image header unreadable".into(),
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Sniffing
// ---------------------------------------------------------------------------------------------------------

/// Identify the format from leading bytes. `Err` carries the reason it is unsupported.
pub fn sniff(b: &[u8]) -> Result<Format, String> {
    if b.is_empty() {
        return Err("empty file".into());
    }
    let starts = |m: &[u8]| b.starts_with(m);
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Ok(Format::Jpeg);
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Ok(Format::Png);
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Ok(Format::Gif);
    }
    if starts(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return Ok(Format::Webp);
    }
    if starts(b"RIFF") && b.get(8..12) == Some(b"AVI ") {
        return Ok(Format::Avi);
    }
    if starts(b"FUJIFILMCCD-RAW") {
        return Ok(Format::Raw(RawFormat::Raf));
    }
    if starts(b"FOVb") {
        return Ok(Format::Raw(RawFormat::X3f));
    }
    if b.get(6..14) == Some(b"HEAPCCDR") {
        return Ok(Format::Raw(RawFormat::Crw));
    }
    if starts(b"8BPS") {
        return Ok(Format::Psd);
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Ok(Format::Mkv);
    }
    if starts(b"IIRO") || starts(b"IIRS") || starts(b"MMOR") {
        return Ok(Format::Raw(RawFormat::Orf));
    }
    if starts(b"IIU\x00") {
        return Ok(Format::Raw(RawFormat::Rw2));
    }
    if starts(b"II*\x00") || starts(b"MM\x00*") {
        return Ok(tiff_flavour(b));
    }
    if is_bigtiff(b) {
        // Recognised, not decoded (`preview-placeholders`): a placeholder says so.
        return Ok(Format::Tiff);
    }
    if let Some(brands) = bmff::ftyp_brands(b) {
        return bmff_flavour(&brands);
    }
    if is_legacy_quicktime(b) {
        return Ok(Format::Mov);
    }
    if starts(b"BM") && is_bmp(b) {
        return Ok(Format::Bmp);
    }
    if is_ico(b) {
        return Ok(Format::Ico);
    }
    if is_mpeg_ts(b) {
        return Ok(Format::Mts);
    }
    if is_svg(b) {
        return Ok(Format::Svg);
    }
    let head: Vec<String> = b.iter().take(8).map(|x| format!("{x:02x}")).collect();
    Err(format!(
        "content not recognised as an image or video (first bytes {})",
        head.join(" ")
    ))
}

fn bmff_flavour(brands: &[[u8; 4]]) -> Result<Format, String> {
    let has = |x: &[u8; 4]| brands.iter().any(|b| b == x);
    let major = brands.first().copied().unwrap_or(*b"    ");
    if has(b"crx ") {
        return Ok(Format::Raw(RawFormat::Cr3));
    }
    if &major == b"avif" || &major == b"avis" {
        return Ok(Format::Avif);
    }
    if [
        b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
    ]
    .contains(&&major)
    {
        return Ok(if has(b"avif") && !has(b"heic") {
            Format::Avif
        } else {
            Format::Heic
        });
    }
    if &major == b"qt  " {
        return Ok(Format::Mov);
    }
    if major.starts_with(b"M4V") {
        return Ok(Format::M4v);
    }
    if major.starts_with(b"3gp") || major.starts_with(b"3g2") {
        return Ok(Format::ThreeGp);
    }
    if [
        b"isom", b"iso2", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"MSNV", b"dash",
        b"mmp4", b"XAVC", b"M4A ",
    ]
    .contains(&&major)
    {
        return Ok(Format::Mp4);
    }
    if has(b"heic") || has(b"mif1") {
        return Ok(Format::Heic);
    }
    if has(b"qt  ") {
        return Ok(Format::Mov);
    }
    if has(b"isom") || has(b"mp41") || has(b"mp42") {
        return Ok(Format::Mp4);
    }
    Err(format!(
        "ISO media file with unsupported brand '{}'",
        String::from_utf8_lossy(&major)
    ))
}

fn is_legacy_quicktime(b: &[u8]) -> bool {
    let Some(size) = be_u32(b, 0) else {
        return false;
    };
    let kind = b.get(4..8).unwrap_or(&[]);
    (size >= 8 || size == 1) && matches!(kind, b"moov" | b"mdat" | b"wide" | b"pnot" | b"skip")
}

fn is_bmp(b: &[u8]) -> bool {
    matches!(
        u32_at(b, 14, crate::tiff::Endian::Little),
        Some(12 | 40 | 52 | 56 | 64 | 108 | 124)
    )
}

fn is_ico(b: &[u8]) -> bool {
    let le = crate::tiff::Endian::Little;
    let (Some(0), Some(t), Some(n)) = (u16_at(b, 0, le), u16_at(b, 2, le), u16_at(b, 4, le)) else {
        return false;
    };
    if !(t == 1 || t == 2) || n == 0 || n > 256 {
        return false;
    }
    // First entry: reserved byte 0 and a data offset after the directory.
    let off = u32_at(b, 6 + 12, le).unwrap_or(0);
    b.get(6 + 3) == Some(&0) && off >= 6 + 16 * u32::from(n)
}

fn is_mpeg_ts(b: &[u8]) -> bool {
    (b.first() == Some(&0x47) && b.get(188) == Some(&0x47) && b.get(376) == Some(&0x47))
        || (b.get(4) == Some(&0x47) && b.get(196) == Some(&0x47) && b.get(388) == Some(&0x47))
}

fn is_svg(b: &[u8]) -> bool {
    let head = &b[..b.len().min(4096)];
    let Ok(text) =
        std::str::from_utf8(head).or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()]))
    else {
        return false;
    };
    let t = text.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<?xml")
        || t.starts_with("<svg")
        || t.starts_with("<!DOCTYPE svg")
        || t.starts_with("<!--"))
        && t.contains("<svg")
}

/// TIFF or a TIFF-based RAW.
fn tiff_flavour(b: &[u8]) -> Format {
    if b.get(8..10) == Some(b"CR") {
        return Format::Raw(RawFormat::Cr2);
    }
    let Some(t) = Tiff::parse(b) else {
        return Format::Tiff;
    };
    let (ifds, _) = t.walk();
    let mut make = None;
    let mut dng = false;
    for ifd in &ifds {
        if ifd.get(0xC612).is_some() {
            dng = true;
        }
        if make.is_none()
            && let Some(e) = ifd.get(0x010F)
        {
            make = t.ascii(e);
        }
    }
    if dng {
        return Format::Raw(RawFormat::Dng);
    }
    let make = make.unwrap_or_default().to_ascii_uppercase();
    let raw = if make.starts_with("NIKON") {
        // NRW is the same container; the extension check reports any mismatch.
        RawFormat::Nef
    } else if make.starts_with("SONY") {
        RawFormat::Arw
    } else if make.starts_with("PENTAX") || make.starts_with("RICOH") {
        RawFormat::Pef
    } else if make.starts_with("SAMSUNG") {
        RawFormat::Srw
    } else if make.starts_with("HASSELBLAD") {
        RawFormat::ThreeFr
    } else if make.starts_with("PHASE ONE") || make.starts_with("LEAF") {
        RawFormat::Iiq
    } else if make.starts_with("CANON") {
        RawFormat::Cr2
    } else {
        return Format::Tiff;
    };
    Format::Raw(raw)
}

/// Extensions that are consistent with a detected format ([`Format::extensions`]).
pub fn expected_extensions(f: Format) -> &'static [&'static str] {
    f.extensions()
}

fn extension_mismatch(path: &Path, f: Format) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    let expected = expected_extensions(f);
    !expected.is_empty() && !expected.contains(&ext.as_str())
}

// ---------------------------------------------------------------------------------------------------------
// EXIF (TIFF block) signals and previews
// ---------------------------------------------------------------------------------------------------------

/// What an EXIF TIFF block contributes to a probe.
#[derive(Debug, Default)]
pub(crate) struct ExifFacts {
    pub camera: CameraSignals,
    pub orientation: Option<u16>,
    pub exif_pixel_dims: Option<Dimensions>,
}

pub(crate) fn exif_facts(t: &Tiff<'_>, ifds: &[IfdView]) -> ExifFacts {
    let mut f = ExifFacts::default();
    let mut dto = None;
    let mut subsec = None;
    let mut offset = None;
    let (mut px, mut py) = (None, None);
    for ifd in ifds {
        match ifd.kind {
            IfdKind::Chain(0) => {
                for e in &ifd.entries {
                    match e.tag {
                        0x010F => f.camera.make = t.ascii(e),
                        0x0110 => f.camera.model = t.ascii(e),
                        0x0131 => f.camera.software = t.ascii(e),
                        0x0112 => f.orientation = t.uint(e).and_then(|v| u16::try_from(v).ok()),
                        _ => {}
                    }
                }
            }
            IfdKind::Exif => {
                for e in &ifd.entries {
                    match e.tag {
                        0x9003 => dto = t.ascii(e),
                        0x9291 => subsec = t.ascii(e),
                        0x9011 => offset = t.ascii(e),
                        0x829A | 0x829D | 0x8827 => f.camera.has_exposure = true,
                        0x927C => f.camera.has_maker_notes = e.count > 0,
                        0xA002 => px = t.uint(e),
                        0xA003 => py = t.uint(e),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    f.camera.date_time_original = dto.as_deref().and_then(parse_exif_datetime).map(|mut c| {
        c.millis = subsec.as_deref().and_then(parse_subsec).unwrap_or(0);
        c.offset_minutes = offset.as_deref().and_then(parse_offset);
        c
    });
    if let (Some(w), Some(h)) = (px, py)
        && w > 0
        && h > 0
    {
        f.exif_pixel_dims = Some(Dimensions::new(w, h));
    }
    f
}

/// JPEG previews referenced by the IFDs of a TIFF block whose start is at absolute offset `base`.
fn tiff_previews(
    t: &Tiff<'_>,
    ifds: &[IfdView],
    base: u64,
    file_size: u64,
    raw_container: bool,
) -> Vec<EmbeddedPreview> {
    let mut out = Vec::new();
    for ifd in ifds {
        if matches!(ifd.kind, IfdKind::Exif | IfdKind::Gps | IfdKind::Interop) {
            continue;
        }
        let get = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e));
        let photometric = get(0x0106);
        if matches!(photometric, Some(32803 | 34892)) || ifd.get(0xC640).is_some() {
            continue; // CFA / linear raw / Canon raw slices
        }
        let (off, len) = if let (Some(o), Some(l)) = (get(0x0201), get(0x0202)) {
            (o, l)
        } else if matches!(get(0x0103), Some(6 | 7 | 99)) {
            let offs = ifd.get(0x0111).map(|e| t.uints(e)).unwrap_or_default();
            let lens = ifd.get(0x0117).map(|e| t.uints(e)).unwrap_or_default();
            if offs.len() != 1 || lens.len() != 1 {
                continue;
            }
            (offs[0], lens[0])
        } else {
            continue;
        };
        if len < 4 {
            continue;
        }
        let abs = base + u64::from(off);
        if abs.saturating_add(u64::from(len)) > file_size {
            continue;
        }
        let local = usize::try_from(off).unwrap_or(usize::MAX);
        let head = t.data.tail(local);
        let mut dims = None;
        if !head.is_empty() {
            if !head.starts_with(&[0xFF, 0xD8]) {
                continue;
            }
            if let Some(sof) = jpeg::sof_of(&head[..head.len().min(len as usize)]) {
                if sof.is_lossless() {
                    continue;
                }
                dims = Some(Dimensions::new(u32::from(sof.width), u32::from(sof.height)));
            }
        }
        if dims.is_none()
            && let (Some(w), Some(h)) = (get(0x0100), get(0x0101))
            && w > 0
            && h > 0
        {
            dims = Some(Dimensions::new(w, h));
        }
        let is_thumb_ifd = matches!(ifd.kind, IfdKind::Chain(1)) && get(0x0201).is_some();
        let source = if raw_container && !is_thumb_ifd {
            PreviewSource::RawPreview
        } else {
            PreviewSource::ExifThumbnail
        };
        out.push(EmbeddedPreview {
            source,
            range: ByteRange {
                offset: abs,
                len: u64::from(len),
            },
            dims,
            decodable: true,
        });
    }
    out
}

/// The best (largest) preview.
fn best_preview(list: Vec<EmbeddedPreview>) -> Option<EmbeddedPreview> {
    list.into_iter()
        .max_by_key(|p| (p.dims.map_or(0, Dimensions::pixels), p.range.len))
}

fn apply_exif(rec: &mut ProbeRecord, facts: &ExifFacts) {
    rec.camera = facts.camera.clone();
    if let Some(o) = facts.orientation
        && (1..=8).contains(&o)
    {
        rec.orientation = Orientation(o as u8);
    }
}

// ---------------------------------------------------------------------------------------------------------
// Per-format probes
// ---------------------------------------------------------------------------------------------------------

/// JPEG at absolute offset `base` (non-zero for previews inside containers).
fn probe_jpeg(buf: &[u8], file_size: u64, base: u64, rec: &mut ProbeRecord) {
    let s = jpeg::scan(buf, file_size);
    if let Some(sof) = s.sof {
        rec.stored = Some(Dimensions::new(u32::from(sof.width), u32::from(sof.height)));
    }
    if let Some((off, len)) = s.exif_tiff {
        let end = (off + len).min(buf.len());
        rec.exif_range = Some(ByteRange {
            offset: base + off as u64,
            len: len as u64,
        });
        if let Some(t) = buf.get(off..end).and_then(Tiff::parse) {
            let (ifds, bad) = t.walk();
            let facts = exif_facts(&t, &ifds);
            apply_exif(rec, &facts);
            if bad {
                rec.warnings
                    .push("EXIF block has an invalid IFD offset".into());
            }
            rec.preview = best_preview(tiff_previews(
                &t,
                &ifds,
                base + off as u64,
                file_size,
                false,
            ));
        } else {
            rec.warnings
                .push("EXIF block is not a valid TIFF structure".into());
        }
    }
    if let Some(issue) = s.issue {
        rec.error = Some(match issue {
            JpegIssue::Truncated(m) => ProbeError {
                kind: ProbeErrorKind::Truncated,
                message: format!("truncated JPEG: {m}"),
            },
            JpegIssue::Corrupt(m) => ProbeError {
                kind: ProbeErrorKind::Corrupt,
                message: format!("corrupt JPEG: {m}"),
            },
        });
    } else if rec.stored.is_none() && s.sos.is_none() && buf.len() as u64 >= file_size {
        rec.error = Some(ProbeError {
            kind: ProbeErrorKind::Truncated,
            message: "truncated JPEG: no frame header".into(),
        });
    }
}

fn probe_png(buf: &[u8], file_size: u64, rec: &mut ProbeRecord) {
    let whole = buf.len() as u64 >= file_size;
    let mut pos = 8usize;
    let mut saw_iend = false;
    let mut chunks = 0;
    while pos + 8 <= buf.len() && chunks < 100_000 {
        chunks += 1;
        let Some(len) = be_u32(buf, pos) else { break };
        let kind = &buf[pos + 4..pos + 8];
        let data_start = pos + 8;
        let end = (data_start as u64) + u64::from(len) + 4;
        if end > file_size {
            rec.error = Some(ProbeError {
                kind: ProbeErrorKind::Truncated,
                message: format!(
                    "truncated PNG: the {} chunk needs {end} bytes, file has {file_size}",
                    String::from_utf8_lossy(kind)
                ),
            });
            break;
        }
        let data = buf
            .get(data_start..(data_start + len as usize).min(buf.len()))
            .unwrap_or(&[]);
        match kind {
            b"IHDR" => {
                if let (Some(w), Some(h), Some(&ct)) =
                    (be_u32(data, 0), be_u32(data, 4), data.get(9))
                {
                    if w == 0 || h == 0 {
                        rec.error = Some(ProbeError {
                            kind: ProbeErrorKind::Corrupt,
                            message: "corrupt PNG: zero dimensions".into(),
                        });
                    } else {
                        rec.stored = Some(Dimensions::new(w, h));
                    }
                    rec.has_palette = ct == 3;
                    rec.has_alpha = ct == 4 || ct == 6;
                }
            }
            b"tRNS" => rec.has_alpha = true,
            b"eXIf" => {
                rec.exif_range = Some(ByteRange {
                    offset: data_start as u64,
                    len: u64::from(len),
                });
                if let Some(t) = Tiff::parse(data) {
                    let (ifds, _) = t.walk();
                    apply_exif(rec, &exif_facts(&t, &ifds));
                }
            }
            b"IEND" => {
                saw_iend = true;
                break;
            }
            _ => {}
        }
        if end > buf.len() as u64 {
            break;
        }
        pos = end as usize;
    }
    if rec.stored.is_none() && rec.error.is_none() {
        rec.error = Some(ProbeError {
            kind: if whole {
                ProbeErrorKind::Truncated
            } else {
                ProbeErrorKind::Corrupt
            },
            message: "PNG header (IHDR) missing".into(),
        });
    }
    if whole && !saw_iend && rec.error.is_none() {
        rec.error = Some(ProbeError {
            kind: ProbeErrorKind::Truncated,
            message: "truncated PNG: no IEND chunk".into(),
        });
    }
}

fn probe_webp(buf: &[u8], rec: &mut ProbeRecord) {
    header_dims(buf, rec);
    // RIFF chunks: VP8X flags and an EXIF chunk.
    let mut pos = 12usize;
    while pos + 8 <= buf.len() {
        let kind = &buf[pos..pos + 4];
        let Some(len) = u32_at(buf, pos + 4, crate::tiff::Endian::Little) else {
            break;
        };
        let data_start = pos + 8;
        let data = buf
            .get(data_start..(data_start + len as usize).min(buf.len()))
            .unwrap_or(&[]);
        match kind {
            b"VP8X" => rec.has_alpha = data.first().is_some_and(|f| f & 0x10 != 0),
            b"VP8L" => rec.has_alpha = data.get(4).is_some_and(|b| b & 0x10 != 0),
            b"ALPH" => rec.has_alpha = true,
            b"EXIF" => {
                let tiff_data = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
                let skip = data.len() - tiff_data.len();
                rec.exif_range = Some(ByteRange {
                    offset: (data_start + skip) as u64,
                    len: tiff_data.len() as u64,
                });
                if let Some(t) = Tiff::parse(tiff_data) {
                    let (ifds, _) = t.walk();
                    apply_exif(rec, &exif_facts(&t, &ifds));
                }
            }
            _ => {}
        }
        let next = data_start as u64 + u64::from(len) + u64::from(len & 1);
        if next > buf.len() as u64 {
            break;
        }
        pos = next as usize;
    }
}

fn probe_tiff(view: View<'_>, file_size: u64, format: Format, rec: &mut ProbeRecord) {
    let Some(t) = Tiff::parse_view(view) else {
        return;
    };
    rec.exif_range = Some(ByteRange {
        offset: 0,
        len: file_size.min(view.head_len() as u64),
    });
    let (ifds, bad) = t.walk();
    if bad {
        if u64::from(t.ifd0) >= file_size {
            rec.error = Some(ProbeError {
                kind: ProbeErrorKind::Corrupt,
                message: format!(
                    "corrupt TIFF: first IFD offset {} is past the end of the file",
                    t.ifd0
                ),
            });
        } else if !view.has(u64::from(t.ifd0), 2) {
            rec.warnings
                .push("first IFD lies beyond the probe window".into());
        } else {
            rec.error = Some(ProbeError {
                kind: ProbeErrorKind::Corrupt,
                message: "corrupt TIFF: first IFD unreadable".into(),
            });
        }
        return;
    }
    let facts = exif_facts(&t, &ifds);
    apply_exif(rec, &facts);
    let dims_of = |ifd: &IfdView| -> Option<Dimensions> {
        let w = t.uint(ifd.get(0x0100)?)?;
        let h = t.uint(ifd.get(0x0101)?)?;
        (w > 0 && h > 0).then(|| Dimensions::new(w, h))
    };
    let largest = |pred: &dyn Fn(&IfdView) -> bool| {
        ifds.iter()
            .filter(|i| pred(i))
            .filter_map(dims_of)
            .max_by_key(|d| d.pixels())
    };
    let is_raw = format.is_raw();
    rec.stored = if !is_raw {
        ifds.iter()
            .find(|i| i.kind == IfdKind::Chain(0))
            .and_then(dims_of)
    } else if format == Format::Raw(RawFormat::Dng) {
        largest(&|i| i.get(0x00FE).and_then(|e| t.uint(e)) == Some(0)).or(largest(&|_| true))
    } else {
        facts.exif_pixel_dims.or_else(|| {
            largest(&|i| {
                !matches!(i.kind, IfdKind::Exif | IfdKind::Gps | IfdKind::Interop)
                    && i.get(0x00FE).and_then(|e| t.uint(e)).unwrap_or(0) == 0
            })
        })
    };
    if !is_raw && let Some(ifd0) = ifds.iter().find(|i| i.kind == IfdKind::Chain(0)) {
        let get = |tag| ifd0.get(tag).and_then(|e| t.uint(e));
        rec.has_palette = get(0x0106) == Some(3);
        rec.has_alpha = ifd0.get(0x0152).is_some();
    }
    rec.preview = best_preview(tiff_previews(&t, &ifds, 0, file_size, is_raw));
}

fn probe_raf(buf: &[u8], file_size: u64, rec: &mut ProbeRecord) {
    let (Some(off), Some(len)) = (be_u32(buf, 84), be_u32(buf, 88)) else {
        rec.error = Some(ProbeError {
            kind: ProbeErrorKind::Truncated,
            message: "truncated RAF header".into(),
        });
        return;
    };
    let abs = u64::from(off);
    if abs + u64::from(len) > file_size || len < 4 {
        rec.error = Some(ProbeError {
            kind: ProbeErrorKind::Corrupt,
            message: "RAF preview offset outside the file".into(),
        });
        return;
    }
    let mut inner = ProbeRecord::empty(u64::from(len));
    if let Some(j) = buf.get(off as usize..) {
        let j = &j[..j.len().min(len as usize)];
        probe_jpeg(j, u64::from(len), abs, &mut inner);
    }
    rec.camera = inner.camera;
    rec.orientation = inner.orientation;
    rec.exif_range = inner.exif_range;
    rec.stored = inner.stored;
    rec.preview = Some(EmbeddedPreview {
        source: PreviewSource::RawPreview,
        range: ByteRange {
            offset: abs,
            len: u64::from(len),
        },
        dims: inner.stored,
        decodable: true,
    });
}

const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
const CANON_PREVIEW_UUID: [u8; 16] = [
    0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16,
];

fn probe_cr3(buf: &[u8], rec: &mut ProbeRecord) {
    let top = bmff::top_level(buf);
    if let Some(moov) = bmff::find(&top, b"moov") {
        for uuid in bmff::children(buf, moov, 0)
            .iter()
            .filter(|b| &b.kind == b"uuid")
        {
            if uuid.body(buf).get(..16) != Some(&CANON_UUID[..]) {
                continue;
            }
            let mut camera = CameraSignals::default();
            let mut orientation = None;
            for c in bmff::children(buf, uuid, 16) {
                let Some(t) = Tiff::parse(c.body(buf)) else {
                    continue;
                };
                let (ifds, _) = t.walk();
                // CMT1 is IFD0, CMT2 the Exif IFD: re-label so exif_facts reads both.
                let relabeled: Vec<IfdView> = ifds
                    .into_iter()
                    .map(|mut i| {
                        if &c.kind == b"CMT2" && i.kind == IfdKind::Chain(0) {
                            i.kind = IfdKind::Exif;
                        }
                        i
                    })
                    .collect();
                let f = exif_facts(&t, &relabeled);
                match &c.kind {
                    b"CMT1" => {
                        camera.make = f.camera.make;
                        camera.model = f.camera.model;
                        camera.software = f.camera.software;
                        orientation = f.orientation;
                    }
                    b"CMT2" => {
                        camera.date_time_original = f.camera.date_time_original;
                        camera.has_exposure = f.camera.has_exposure;
                        camera.has_maker_notes = f.camera.has_maker_notes;
                        if let Some(d) = f.exif_pixel_dims {
                            rec.stored = Some(d);
                        }
                    }
                    _ => {}
                }
            }
            apply_exif(
                rec,
                &ExifFacts {
                    camera,
                    orientation,
                    ..Default::default()
                },
            );
        }
    }
    // Top-level preview uuid holding a PRVW box: u32 0, u16 1, u16 width, u16 height, u16 1, u32 size, JPEG.
    for uuid in top.iter().filter(|b| &b.kind == b"uuid") {
        if buf.get(uuid.body..uuid.body + 16) != Some(&CANON_PREVIEW_UUID[..]) {
            continue;
        }
        let kids = bmff::boxes(buf, uuid.body + 16 + 8, buf.len());
        if let Some(prvw) = bmff::find(&kids, b"PRVW") {
            let b = prvw.body;
            let (Some(w), Some(h), Some(len)) =
                (be_u16(buf, b + 6), be_u16(buf, b + 8), be_u32(buf, b + 12))
            else {
                continue;
            };
            rec.preview = Some(EmbeddedPreview {
                source: PreviewSource::RawPreview,
                range: ByteRange {
                    offset: (b + 16) as u64,
                    len: u64::from(len),
                },
                dims: Some(Dimensions::new(u32::from(w), u32::from(h))),
                decodable: true,
            });
        }
    }
}

fn probe_heif(buf: &[u8], file_size: u64, rec: &mut ProbeRecord) {
    let Some(info) = bmff::parse_heif(buf) else {
        rec.error = Some(ProbeError {
            kind: if (buf.len() as u64) < file_size {
                ProbeErrorKind::Corrupt
            } else {
                ProbeErrorKind::Truncated
            },
            message: "HEIF meta box missing or unreadable".into(),
        });
        return;
    };
    let primary = info.primary.and_then(|id| info.item(id));
    if let Some(p) = primary {
        if let Some((w, h)) = p.dims {
            rec.stored = Some(Dimensions::new(w, h));
        }
        // irot (anti-clockwise) then imir, as an EXIF orientation: 8 = 90 CCW, 3 = 180, 6 = 270 CCW (90 CW).
        rec.orientation = Orientation(p.orientation());
        // A grid's own size is its `ispe`; without one, the grid body would say (decoders read it).
    }
    // EXIF item: 4-byte header offset, then the TIFF block.
    if let Some(ex) = info.exif_items.first().and_then(|id| info.item(*id))
        && let Some(&(off, len)) = ex.extents.first()
        && len > 4
        && off.saturating_add(len) <= file_size
    {
        let local = usize::try_from(off).unwrap_or(usize::MAX);
        let skip = be_u32(buf, local).map_or(0, u64::from);
        let tiff_off = off + 4 + skip;
        rec.exif_range = Some(ByteRange {
            offset: tiff_off,
            len: len.saturating_sub(4 + skip),
        });
        let start = usize::try_from(tiff_off).unwrap_or(usize::MAX);
        let end = usize::try_from(off + len)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        if let Some(t) = buf.get(start..end).and_then(Tiff::parse) {
            let (ifds, _) = t.walk();
            let facts = exif_facts(&t, &ifds);
            let orientation = rec.orientation;
            apply_exif(rec, &facts);
            // HEIF display orientation comes from irot, not the informational EXIF tag.
            rec.orientation = orientation;
        } else if start >= buf.len() {
            rec.warnings
                .push("EXIF item lies beyond the probe window".into());
        }
    }
    let primary_id = info.primary.unwrap_or(0);
    let thumb = info
        .thumbnails
        .iter()
        .filter(|(_, master)| *master == primary_id)
        .filter_map(|(t, _)| info.item(*t))
        .max_by_key(|i| i.dims.map_or(0, |(w, h)| u64::from(w) * u64::from(h)));
    if let Some(t) = thumb
        && let Some(&(off, len)) = t.extents.first()
    {
        rec.preview = Some(EmbeddedPreview {
            source: PreviewSource::HeicThumbnail,
            range: ByteRange { offset: off, len },
            dims: t.dims.map(|(w, h)| Dimensions::new(w, h)),
            // A property of the file: an HEVC-coded thumbnail item. Whether this machine can
            // decode it is the decoder registry's business (ADR-0089).
            decodable: &t.kind == b"hvc1",
        });
    }
}

fn probe_movie(buf: &[u8], rec: &mut ProbeRecord) {
    let Some(moov) = bmff::moov_box(buf) else {
        rec.warnings
            .push("moov atom not found in the probe window".into());
        rec.video = Some(VideoSummary::default());
        return;
    };
    if !moov.complete(buf) {
        rec.warnings
            .push("moov atom extends beyond the probe window".into());
        rec.video = Some(VideoSummary::default());
        return;
    }
    let info = crate::video::movie_info(buf, &moov);
    if let (Some(w), Some(h)) = (info.width, info.height)
        && w > 0
        && h > 0
    {
        rec.stored = Some(Dimensions::new(w, h));
        rec.orientation = Orientation(match info.rotation {
            90 => 6,
            180 => 3,
            270 => 8,
            _ => 1,
        });
    }
    rec.video = Some(VideoSummary {
        duration_ms: info.duration_ms,
        creation: info.creation,
        make: info.make,
        model: info.model,
    });
}

/// QuickTime epoch seconds to a capture time.
pub(crate) fn mac_time(secs: u64) -> Option<ps_model::CaptureTime> {
    (secs > 0).then(|| capture_time_from_unix(secs as i64 - MAC_EPOCH_OFFSET))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_basics() {
        assert_eq!(
            sniff(&ps_fixtures::media::jpeg(8, 8, 1, 80)),
            Ok(Format::Jpeg)
        );
        assert_eq!(sniff(&ps_fixtures::media::svg()), Ok(Format::Svg));
        assert_eq!(sniff(&ps_fixtures::media::ico(32)), Ok(Format::Ico));
        assert!(sniff(b"hello world").is_err());
        assert!(sniff(b"").is_err());
    }
}
