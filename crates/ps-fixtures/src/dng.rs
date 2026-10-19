//! Minimal DNG: IFD0 is a full-size JPEG preview (NewSubFileType 1) with camera tags, an Exif IFD and GPS,
//! plus one SubIFD holding a tiny 16-bit CFA "sensor strip". Enough for format detection, metadata, embedded
//! preview extraction and RAW+JPEG pairing; not a decodable full-resolution raw.

use crate::camera::{ExifSpec, exif_ifds};
use crate::rng::mix64;
use crate::tiff::{Endian, Ifd, Value, write_tiff};

/// Build a DNG for a shot whose camera JPEG (without APP1) is `preview_jpeg` at `width x height`.
pub fn dng(spec: &ExifSpec<'_>, preview_jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    let m = spec.body.model;
    let mut raw = Ifd::new();
    let (rw, rh) = ((width / 16).max(2) & !1, (height / 16).max(2) & !1);
    let mut strip = Vec::with_capacity((rw * rh * 2) as usize);
    let mut s = spec.maker_note.unwrap_or(1);
    for _ in 0..rw * rh {
        s = mix64(s);
        let v = 600 + (s % 14_000) as u16;
        strip.extend_from_slice(&match m.endian {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        });
    }
    raw.long(0x00FE, 0)
        .long(0x0100, rw)
        .long(0x0101, rh)
        .short(0x0102, 16)
        .short(0x0103, 1)
        .short(0x0106, 32803)
        .short(0x0115, 1)
        .long(0x0116, rh)
        .short(0x011C, 1)
        .set(0x828D, Value::Short(vec![2, 2]))
        .set(0x828E, Value::Byte(vec![0, 1, 1, 2]))
        .long(0xC61A, 512)
        .long(0xC61D, 15_000)
        .blob(0x0111, 0x0117, strip);

    let mut ifd0 = Ifd::new();
    ifd0.long(0x00FE, 1)
        .long(0x0100, width)
        .long(0x0101, height)
        .set(0x0102, Value::Short(vec![8, 8, 8]))
        .short(0x0103, 7)
        .short(0x0106, 6)
        .ascii(0x010F, m.make)
        // Like rawler's normalized names (D1): the RAW's model without the make ("EOS 5D Mark III"), while
        // the JPEG twin says "Canon EOS 5D Mark III". Pairing must normalize.
        .ascii(
            0x0110,
            m.model
                .strip_prefix(m.make)
                .map_or(m.model, str::trim_start),
        )
        .short(0x0112, spec.orientation)
        .short(0x0115, 3)
        .long(0x0116, height)
        .ascii(0x0131, "Adobe DNG Converter 12.0")
        .ascii(0x0132, spec.capture.exif())
        .set(0xC612, Value::Byte(vec![1, 4, 0, 0]))
        .set(0xC613, Value::Byte(vec![1, 1, 0, 0]))
        .ascii(0xC614, m.model)
        .blob(0x0111, 0x0117, preview_jpeg.to_vec())
        .child(0x014A, raw);

    let (ex, gps) = exif_ifds(spec);
    ifd0.child(0x8769, ex);
    if let Some(g) = gps {
        ifd0.child(0x8825, g);
    }
    write_tiff(&ifd0, m.endian)
}
