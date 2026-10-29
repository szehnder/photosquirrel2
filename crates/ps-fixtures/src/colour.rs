//! Colour-space fixtures (`adobe-rgb-color`, ADR-0113): solid patches in a known colour space, recorded the ways
//! cameras and phones record it, so a test can compare a thumbnail with a reference conversion.
//!
//! - [`adobe_rgb_jpeg`]: a Canon-style `_MG_` JPEG, EXIF `ColorSpace` 0xFFFF and `InteropIndex` `R03`, no profile.
//! - [`icc_jpeg`]: a JPEG with an embedded ICC profile in `APP2 ICC_PROFILE` chunks ([`display_p3_icc`],
//!   [`srgb_icc`], [`adobe_rgb_icc`]).
//! - [`canon_adobe_rgb_cr2`]: a CR2-style file whose Canon maker note records `ColorSpace` (0x00B4) = 2 and whose
//!   preview JPEG carries no EXIF of its own.
//! - [`dng_preview_colour`]: a DNG whose preview IFD records `PreviewColorSpace` (0xC71A).
//!
//! Every writer is a pure function of its arguments. The ground-truth corpus does not use them.

use crate::jpeg::{app1_exif, camera_jpeg, encode_jpeg, strip_segments};
use crate::synth::Rgb;
use crate::tiff::{Endian, Ifd, Value, write_tiff};

/// A saturated green whose conversion from Adobe RGB (1998) or Display P3 stays inside sRGB.
pub const GREEN: [u8; 3] = [40, 170, 60];

/// The patch size of the writers: 3:2, above the 320 px a usable embedded preview needs.
pub const PATCH: (u32, u32) = (640, 426);

/// The capture time every writer records.
pub const CAPTURE: &str = "2024:06:01 10:00:00";

/// A `width` x `height` image of one colour.
pub fn patch(width: u32, height: u32, rgb: [u8; 3]) -> Rgb {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..width * height {
        data.extend_from_slice(&rgb);
    }
    Rgb {
        width,
        height,
        data,
    }
}

/// The JPEG of a [`PATCH`]-sized patch of `rgb` at quality 95, without its JFIF APP0 (as cameras write).
fn patch_jpeg(rgb: [u8; 3]) -> Vec<u8> {
    strip_segments(&encode_jpeg(&patch(PATCH.0, PATCH.1, rgb), 95), &[0xE0])
}

/// A Canon maker note (a bare little-endian IFD) with one entry, `ColorSpace` (0x00B4, SHORT) = `value`.
pub fn canon_colour_note(value: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0x00B4u16.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// The Exif IFD of a Canon file: the capture time, `ColorSpace` (0xA001) = `colour_space`, an Interop IFD with
/// `InteropIndex` = `interop` when given, and a maker note when given.
fn exif_ifd(colour_space: u16, interop: Option<&str>, note: Option<Vec<u8>>) -> Ifd {
    let mut ex = Ifd::new();
    ex.ascii(0x9003, CAPTURE).short(0xA001, colour_space);
    if let Some(n) = note {
        ex.set(0x927C, Value::Undefined(n));
    }
    if let Some(index) = interop {
        let mut io = Ifd::new();
        io.ascii(0x0001, index)
            .set(0x0002, Value::Undefined(b"0100".to_vec()));
        ex.child(0xA005, io);
    }
    ex
}

/// A Canon `_MG_` JPEG in Adobe RGB (1998), as an EOS body set to Adobe RGB writes it: a [`PATCH`] of `rgb` (Adobe
/// RGB values), EXIF `ColorSpace` 0xFFFF ("uncalibrated") and `InteropIndex` `R03` (DCF's Adobe RGB option file),
/// no ICC profile.
pub fn adobe_rgb_jpeg(rgb: [u8; 3]) -> Vec<u8> {
    exif_jpeg(rgb, 0xFFFF, Some("R03"), None)
}

/// A camera JPEG of a [`PATCH`] of `rgb` with EXIF `ColorSpace` = `colour_space`, `InteropIndex` = `interop` and a
/// Canon maker note recording `ColorSpace` = `canon_note` when given.
pub fn exif_jpeg(
    rgb: [u8; 3],
    colour_space: u16,
    interop: Option<&str>,
    canon_note: Option<u16>,
) -> Vec<u8> {
    with_exif(
        &encode_jpeg(&patch(PATCH.0, PATCH.1, rgb), 95),
        colour_space,
        interop,
        canon_note,
    )
}

/// `encoded` (a JPEG as an encoder writes it) as a Canon camera JPEG: its JFIF APP0 replaced by an EXIF block with
/// `ColorSpace` = `colour_space`, `InteropIndex` = `interop` and a Canon maker note recording `ColorSpace` =
/// `canon_note` when given.
pub fn with_exif(
    encoded: &[u8],
    colour_space: u16,
    interop: Option<&str>,
    canon_note: Option<u16>,
) -> Vec<u8> {
    let mut ifd0 = Ifd::new();
    ifd0.ascii(0x010F, "Canon")
        .ascii(0x0110, "Canon EOS 5D Mark III")
        .short(0x0112, 1)
        .ascii(0x0132, CAPTURE)
        .child(
            0x8769,
            exif_ifd(colour_space, interop, canon_note.map(canon_colour_note)),
        );
    let tiff = write_tiff(&ifd0, Endian::Little);
    camera_jpeg(encoded, &app1_exif(&tiff))
}

/// Largest ICC payload of one `APP2` segment (65,535 - 2 length bytes - 14 header bytes).
const ICC_CHUNK: usize = 65_519;

/// `APP2 ICC_PROFILE` segments holding `icc`, in sequence order.
pub fn icc_segments(icc: &[u8]) -> Vec<u8> {
    let chunks: Vec<&[u8]> = icc.chunks(ICC_CHUNK).collect();
    let mut out = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        out.extend_from_slice(&[0xFF, 0xE2]);
        out.extend_from_slice(&((c.len() + 16) as u16).to_be_bytes());
        out.extend_from_slice(b"ICC_PROFILE\0");
        out.push((i + 1) as u8);
        out.push(chunks.len() as u8);
        out.extend_from_slice(c);
    }
    out
}

/// A JPEG of a [`PATCH`] of `rgb` with `icc` embedded in `APP2 ICC_PROFILE` chunks after SOI, and no EXIF.
pub fn icc_jpeg(rgb: [u8; 3], icc: &[u8]) -> Vec<u8> {
    with_icc(&patch_jpeg(rgb), icc)
}

/// `jpeg` with `icc` embedded in `APP2 ICC_PROFILE` chunks right after SOI.
pub fn with_icc(jpeg: &[u8], icc: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(jpeg.len() + icc.len() + 32);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&icc_segments(icc));
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// A JPEG of a [`PATCH`] of `rgb` with neither EXIF nor a profile (an untagged JPEG).
pub fn untagged_jpeg(rgb: [u8; 3]) -> Vec<u8> {
    patch_jpeg(rgb)
}

/// The profile encoded by moxcms, with the header's creation date (bytes 24..36: year, month, day, hour, minute,
/// second, big-endian) set to [`CAPTURE`]'s: moxcms writes the current time there, which made two encodings a second
/// apart differ.
fn encoded(p: &moxcms::ColorProfile) -> Vec<u8> {
    let mut v = p.encode().expect("moxcms encodes its built-in profiles");
    if v.len() >= 36 {
        for (i, n) in [2024u16, 6, 1, 10, 0, 0].into_iter().enumerate() {
            v[24 + 2 * i..26 + 2 * i].copy_from_slice(&n.to_be_bytes());
        }
    }
    v
}

/// moxcms' Display P3 profile, encoded (description "Display P3").
pub fn display_p3_icc() -> Vec<u8> {
    encoded(&moxcms::ColorProfile::new_display_p3())
}

/// moxcms' sRGB profile, encoded (description "sRGB IEC61966-2.1").
pub fn srgb_icc() -> Vec<u8> {
    encoded(&moxcms::ColorProfile::new_srgb())
}

/// moxcms' Adobe RGB (1998) profile, encoded (description "Adobe RGB 1998").
pub fn adobe_rgb_icc() -> Vec<u8> {
    encoded(&moxcms::ColorProfile::new_adobe_rgb())
}

/// moxcms' ProPhoto RGB profile, renamed `description`: a wide-gamut profile that is none of the named spaces.
pub fn pro_photo_icc(description: &str) -> Vec<u8> {
    let mut p = moxcms::ColorProfile::new_pro_photo_rgb();
    p.description = Some(moxcms::ProfileText::Localizable(vec![
        moxcms::LocalizableString::new("en".into(), "US".into(), description.into()),
    ]));
    encoded(&p)
}

/// A CR2-style RAW from a Canon EOS 5D Mark III set to Adobe RGB: IFD0 holds a [`PATCH`]-sized preview JPEG of
/// `rgb` (Adobe RGB values) with no EXIF of its own, the Exif IFD `ColorSpace` 0xFFFF without an Interop IFD, and
/// the Canon maker note `ColorSpace` (0x00B4) = `canon_note`, so only the maker note tells the colour space.
pub fn canon_cr2(rgb: [u8; 3], canon_note: u16) -> Vec<u8> {
    canon_cr2_of(&patch_jpeg(rgb), PATCH, canon_note)
}

/// [`canon_cr2`] around any preview JPEG of `dims` (without EXIF of its own).
pub fn canon_cr2_of(preview_jpeg: &[u8], dims: (u32, u32), canon_note: u16) -> Vec<u8> {
    let preview = preview_jpeg.to_vec();
    let mut ifd0 = Ifd::new();
    ifd0.long(0x0100, dims.0)
        .long(0x0101, dims.1)
        .short(0x0103, 6)
        .ascii(0x010F, "Canon")
        .ascii(0x0110, "Canon EOS 5D Mark III")
        .short(0x0112, 1)
        .ascii(0x0132, CAPTURE)
        .blob(0x0111, 0x0117, preview)
        .child(
            0x8769,
            exif_ifd(0xFFFF, None, Some(canon_colour_note(canon_note))),
        );
    write_tiff(&ifd0, Endian::Little)
}

/// [`canon_cr2_of`] with `sensor_bytes` of 16-bit sensor data in a SubIFD strip after the preview, so the file is
/// as much larger than its preview as a camera's is (a test that counts the bytes a preview read takes).
pub fn canon_cr2_with_sensor(
    preview_jpeg: &[u8],
    dims: (u32, u32),
    canon_note: u16,
    sensor_bytes: u32,
) -> Vec<u8> {
    let (rw, rh) = (1024u32, (sensor_bytes / 2048).max(1));
    let mut strip = Vec::with_capacity((rw * rh * 2) as usize);
    for i in 0..rw * rh {
        strip.extend_from_slice(&(600 + (i % 97) as u16 * 100).to_le_bytes());
    }
    let mut raw = Ifd::new();
    raw.long(0x00FE, 0)
        .long(0x0100, rw)
        .long(0x0101, rh)
        .short(0x0102, 16)
        .short(0x0103, 1)
        .short(0x0106, 32803)
        .short(0x0115, 1)
        .long(0x0116, rh)
        .blob(0x0111, 0x0117, strip);
    let mut ifd0 = Ifd::new();
    ifd0.long(0x0100, dims.0)
        .long(0x0101, dims.1)
        .short(0x0103, 6)
        .ascii(0x010F, "Canon")
        .ascii(0x0110, "Canon EOS 5D Mark III")
        .short(0x0112, 1)
        .ascii(0x0132, CAPTURE)
        .blob(0x0111, 0x0117, preview_jpeg.to_vec())
        .child(
            0x8769,
            exif_ifd(0xFFFF, None, Some(canon_colour_note(canon_note))),
        )
        .child(0x014A, raw);
    write_tiff(&ifd0, Endian::Little)
}

/// [`canon_cr2`] recording Adobe RGB (maker-note colour space 2).
pub fn canon_adobe_rgb_cr2(rgb: [u8; 3]) -> Vec<u8> {
    canon_cr2(rgb, 2)
}

/// A DNG whose IFD0 is a [`PATCH`]-sized preview JPEG of `rgb` with `PreviewColorSpace` (0xC71A) = `preview_space`
/// (2 sRGB, 3 Adobe RGB), and a SubIFD holding a tiny 16-bit CFA strip.
pub fn dng_preview_colour(rgb: [u8; 3], preview_space: u32) -> Vec<u8> {
    let preview = patch_jpeg(rgb);
    let (rw, rh) = (16u32, 16u32);
    let mut strip = Vec::with_capacity((rw * rh * 2) as usize);
    for i in 0..rw * rh {
        strip.extend_from_slice(&(600 + (i % 97) as u16 * 100).to_le_bytes());
    }
    let mut raw = Ifd::new();
    raw.long(0x00FE, 0)
        .long(0x0100, rw)
        .long(0x0101, rh)
        .short(0x0102, 16)
        .short(0x0103, 1)
        .short(0x0106, 32803)
        .short(0x0115, 1)
        .long(0x0116, rh)
        .set(0x828D, Value::Short(vec![2, 2]))
        .set(0x828E, Value::Byte(vec![0, 1, 1, 2]))
        .blob(0x0111, 0x0117, strip);
    let mut ifd0 = Ifd::new();
    ifd0.long(0x00FE, 1)
        .long(0x0100, PATCH.0)
        .long(0x0101, PATCH.1)
        .set(0x0102, Value::Short(vec![8, 8, 8]))
        .short(0x0103, 7)
        .short(0x0106, 6)
        .ascii(0x010F, "Canon")
        .ascii(0x0110, "EOS 5D Mark III")
        .short(0x0112, 1)
        .short(0x0115, 3)
        .long(0x0116, PATCH.1)
        .ascii(0x0131, "Adobe DNG Converter 12.0")
        .ascii(0x0132, CAPTURE)
        .set(0xC612, Value::Byte(vec![1, 4, 0, 0]))
        .set(0xC613, Value::Byte(vec![1, 1, 0, 0]))
        .ascii(0xC614, "Canon EOS 5D Mark III")
        .long(0xC71A, preview_space)
        .blob(0x0111, 0x0117, preview)
        .child(0x014A, raw);
    let mut ex = Ifd::new();
    ex.ascii(0x9003, CAPTURE);
    ifd0.child(0x8769, ex);
    write_tiff(&ifd0, Endian::Little)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writers_are_deterministic() {
        assert_eq!(adobe_rgb_jpeg(GREEN), adobe_rgb_jpeg(GREEN));
        assert_eq!(
            icc_jpeg(GREEN, &display_p3_icc()),
            icc_jpeg(GREEN, &display_p3_icc())
        );
        assert_eq!(canon_adobe_rgb_cr2(GREEN), canon_adobe_rgb_cr2(GREEN));
        assert_eq!(dng_preview_colour(GREEN, 3), dng_preview_colour(GREEN, 3));
        // The profile header carries a fixed date, not the time of encoding.
        assert_eq!(
            &display_p3_icc()[24..36],
            &[7, 232, 0, 6, 0, 1, 0, 10, 0, 0, 0, 0]
        );
    }

    #[test]
    fn icc_is_split_into_numbered_chunks() {
        let big = vec![7u8; ICC_CHUNK + 10];
        let segs = icc_segments(&big);
        assert_eq!(&segs[4..16], b"ICC_PROFILE\0");
        assert_eq!((segs[16], segs[17]), (1, 2));
        let second = 2 + 2 + 14 + ICC_CHUNK;
        assert_eq!(&segs[second..second + 2], &[0xFF, 0xE2]);
        assert_eq!((segs[second + 16], segs[second + 17]), (2, 2));
    }
}
