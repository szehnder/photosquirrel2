//! JPEG/PNG encoding and JPEG segment surgery (insert or strip APP0/APP1).

use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

use crate::synth::Rgb;

/// Baseline JPEG of `img` at `quality` (1-100). Output starts `SOI APP0(JFIF) ...` as the `image` encoder writes it.
pub fn encode_jpeg(img: &Rgb, quality: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() / 6 + 1024);
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode(&img.data, img.width, img.height, ExtendedColorType::Rgb8)
        .expect("encoding an in-memory RGB buffer cannot fail");
    out
}

/// RGB or palette PNG encoder settings.
pub enum PngKind<'a> {
    Rgb(&'a Rgb),
    /// Indexed: (width, height, indices, palette RGB triples, optional tRNS alpha per palette entry).
    Indexed {
        width: u32,
        height: u32,
        indices: &'a [u8],
        palette: &'a [u8],
        trns: Option<&'a [u8]>,
    },
    /// RGBA 8-bit.
    Rgba {
        width: u32,
        height: u32,
        data: &'a [u8],
    },
}

/// Encode a PNG (fast deflate; deterministic).
pub fn encode_png(kind: PngKind<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let (w, h) = match &kind {
            PngKind::Rgb(i) => (i.width, i.height),
            PngKind::Indexed { width, height, .. } | PngKind::Rgba { width, height, .. } => {
                (*width, *height)
            }
        };
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let data: &[u8] = match &kind {
            PngKind::Rgb(i) => {
                enc.set_color(png::ColorType::Rgb);
                &i.data
            }
            PngKind::Indexed {
                indices,
                palette,
                trns,
                ..
            } => {
                enc.set_color(png::ColorType::Indexed);
                enc.set_palette(palette.to_vec());
                if let Some(t) = trns {
                    enc.set_trns(t.to_vec());
                }
                indices
            }
            PngKind::Rgba { data, .. } => {
                enc.set_color(png::ColorType::Rgba);
                data
            }
        };
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(data).expect("png data");
        wr.finish().expect("png finish");
    }
    out
}

/// Marker segments before the scan: (marker, start offset, total length incl. marker).
fn segments(jpeg: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let m = jpeg[i + 1];
        if m == 0xDA {
            break;
        }
        let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
        out.push((m, i, len + 2));
        i += len + 2;
    }
    out
}

/// Remove every segment with marker `m` (e.g. `0xE0` JFIF, `0xE1` EXIF/XMP).
pub fn strip_segments(jpeg: &[u8], markers: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&jpeg[..2]);
    let mut pos = 2;
    for (m, start, len) in segments(jpeg) {
        out.extend_from_slice(&jpeg[pos..start]);
        if !markers.contains(&m) {
            out.extend_from_slice(&jpeg[start..start + len]);
        }
        pos = start + len;
    }
    out.extend_from_slice(&jpeg[pos..]);
    out
}

/// Build an APP1 EXIF segment from a TIFF stream.
pub fn app1_exif(tiff: &[u8]) -> Vec<u8> {
    let len = tiff.len() + 6 + 2;
    assert!(len <= 0xFFFF, "EXIF block too large: {len}");
    let mut seg = Vec::with_capacity(len + 2);
    seg.extend_from_slice(&[0xFF, 0xE1]);
    seg.extend_from_slice(&(len as u16).to_be_bytes());
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(tiff);
    seg
}

/// Camera style: `SOI APP1(EXIF) ...` with the encoder's JFIF APP0 removed.
pub fn camera_jpeg(encoded: &[u8], app1: &[u8]) -> Vec<u8> {
    let body = strip_segments(encoded, &[0xE0]);
    let mut out = Vec::with_capacity(body.len() + app1.len());
    out.extend_from_slice(&body[..2]);
    out.extend_from_slice(app1);
    out.extend_from_slice(&body[2..]);
    out
}

/// Editor style: `SOI APP0(JFIF) APP1(EXIF) ...`.
pub fn editor_jpeg(encoded: &[u8], app1: &[u8]) -> Vec<u8> {
    let segs = segments(encoded);
    let insert_at = segs
        .iter()
        .find(|(m, ..)| *m == 0xE0)
        .map(|(_, s, l)| s + l)
        .unwrap_or(2);
    let mut out = Vec::with_capacity(encoded.len() + app1.len());
    out.extend_from_slice(&encoded[..insert_at]);
    out.extend_from_slice(app1);
    out.extend_from_slice(&encoded[insert_at..]);
    out
}

/// True if the JPEG has an APP1 segment starting with `Exif\0\0`.
pub fn has_exif(jpeg: &[u8]) -> bool {
    segments(jpeg)
        .iter()
        .any(|(m, s, l)| *m == 0xE1 && *l > 10 && &jpeg[s + 4..s + 10] == b"Exif\0\0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_strip_exif() {
        let img = Rgb::new(16, 16);
        let enc = encode_jpeg(&img, 90);
        assert!(!has_exif(&enc));
        let app1 = app1_exif(b"II*\0\x08\0\0\0\0\0\0\0\0\0");
        let cam = camera_jpeg(&enc, &app1);
        assert!(has_exif(&cam));
        assert_eq!(&cam[2..4], &[0xFF, 0xE1]);
        let stripped = strip_segments(&cam, &[0xE1]);
        assert!(!has_exif(&stripped));
        assert!(image::load_from_memory(&stripped).is_ok());
        let ed = editor_jpeg(&enc, &app1);
        assert_eq!(&ed[2..4], &[0xFF, 0xE0]);
        assert!(has_exif(&ed));
    }
}
