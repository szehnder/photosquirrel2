//! Apple Live Photo pairing (export change, task 1.2): the ContentIdentifier an iPhone writes into both
//! halves of a Live Photo, so the still (HEIC or JPEG) and its video (MOV) can be exported together.
//!
//! - Stills: the Apple maker note (EXIF 0x927C, `Apple iOS` header, tag 0x0011 ContentIdentifier), or an XMP
//!   property named `ContentIdentifier` (written by some tools and converters).
//! - Movies: the `com.apple.quicktime.content.identifier` key of `moov/meta` (`keys` + `ilst`).
//!
//! All parsing is bounded and panic-free; anything malformed yields `None`.

use crate::bmff::{self, BoxRef};
use crate::tiff::be_u32;

/// Maker-note tag of the ContentIdentifier (exiftool `Apple::Main` 0x0011).
const TAG_CONTENT_IDENTIFIER: u16 = 0x0011;
/// QuickTime metadata key of the ContentIdentifier.
pub const QUICKTIME_KEY: &str = "com.apple.quicktime.content.identifier";

/// A plausible identifier: printable ASCII, 1-128 chars (iPhones write an upper-case UUID).
fn clean(s: &[u8]) -> Option<String> {
    let end = s.iter().position(|b| *b == 0).unwrap_or(s.len());
    let s = std::str::from_utf8(&s[..end]).ok()?.trim();
    (!s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| s.to_owned())
}

/// The ContentIdentifier in an Apple maker note (the EXIF MakerNote value).
///
/// Layout: `Apple iOS\0`, a 2-byte version, a 2-byte byte-order mark (`MM`/`II`), then an IFD at offset 14
/// whose value offsets are relative to the start of the maker note.
pub fn maker_note_content_identifier(note: &[u8]) -> Option<String> {
    if !note.starts_with(b"Apple iOS\0") || note.len() < 16 {
        return None;
    }
    let big = match note.get(12..14)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = note.get(o..o + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = note.get(o..o + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let count = usize::from(u16_at(14)?).min(512);
    for i in 0..count {
        let e = 16 + i * 12;
        if u16_at(e)? != TAG_CONTENT_IDENTIFIER {
            continue;
        }
        // ASCII (type 2) or undefined bytes (type 7).
        if !matches!(u16_at(e + 2)?, 2 | 7) {
            return None;
        }
        let n = usize::try_from(u32_at(e + 4)?).ok()?.min(256);
        let value = if n <= 4 {
            note.get(e + 8..e + 8 + n)?
        } else {
            let off = usize::try_from(u32_at(e + 8)?).ok()?;
            note.get(off..off.checked_add(n)?)?
        };
        return clean(value);
    }
    None
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// A `ContentIdentifier` property in an XMP packet anywhere in `bytes` (attribute `x:ContentIdentifier="…"`
/// or element `<x:ContentIdentifier>…</x:ContentIdentifier>`).
pub fn xmp_content_identifier(bytes: &[u8]) -> Option<String> {
    let start = find(bytes, b"<x:xmpmeta", 0)?;
    let end = find(bytes, b"</x:xmpmeta>", start).unwrap_or(bytes.len());
    let xmp = &bytes[start..end];
    let mut from = 0;
    while let Some(p) = find(xmp, b"ContentIdentifier", from) {
        from = p + 1;
        // A property name: preceded by a namespace prefix (`apple-fi:`, `MakerApple:`, ...).
        if p == 0 || xmp[p - 1] != b':' {
            continue;
        }
        let rest = &xmp[p + b"ContentIdentifier".len()..];
        let value = match rest.first() {
            Some(b'=') => {
                let q = *rest.get(1)?;
                if q != b'"' && q != b'\'' {
                    continue;
                }
                let v = &rest[2..];
                &v[..v.iter().position(|b| *b == q)?]
            }
            Some(b'>') => {
                let v = &rest[1..];
                &v[..v.iter().position(|b| *b == b'<')?]
            }
            _ => continue,
        };
        if let Some(s) = clean(value) {
            return Some(s);
        }
    }
    None
}

/// The `com.apple.quicktime.content.identifier` value of `moov/meta` (QuickTime `meta` boxes have no
/// version/flags; ISO-BMFF ones do: both are accepted).
pub fn quicktime_content_identifier(buf: &[u8], moov: &BoxRef) -> Option<String> {
    quicktime_key_value(buf, moov, QUICKTIME_KEY).and_then(|v| clean(&v))
}

/// The keyed QuickTime creation date (`com.apple.quicktime.creationdate`, an ISO 8601 local time with its
/// offset), as text.
pub fn quicktime_creation_date(buf: &[u8], moov: &BoxRef) -> Option<String> {
    let v = quicktime_key_value(buf, moov, QUICKTIME_CREATION_KEY)?;
    let end = v.iter().position(|b| *b == 0).unwrap_or(v.len());
    let s = std::str::from_utf8(&v[..end]).ok()?.trim();
    (!s.is_empty() && s.len() <= 64).then(|| s.to_owned())
}

/// The key of the keyed QuickTime creation date.
pub const QUICKTIME_CREATION_KEY: &str = "com.apple.quicktime.creationdate";

/// The raw value of the `mdta` key `key` in `moov/meta`.
fn quicktime_key_value(buf: &[u8], moov: &BoxRef, key: &str) -> Option<Vec<u8>> {
    let kids = bmff::children(buf, moov, 0);
    let meta = bmff::find(&kids, b"meta")?;
    let items = [0usize, 4]
        .into_iter()
        .map(|skip| bmff::children(buf, meta, skip))
        .find(|c| bmff::find(c, b"keys").is_some())?;
    let keys = bmff::find(&items, b"keys")?.body(buf);
    // keys: version/flags (4), entry count (4), then (size u32, namespace 4cc, name) per key, 1-based.
    let count = be_u32(keys, 4)?.min(4096);
    let mut pos = 8usize;
    let mut index = None;
    for i in 1..=count {
        let size = usize::try_from(be_u32(keys, pos)?).ok()?;
        if size < 8 {
            return None;
        }
        let name = keys.get(pos + 8..pos + size)?;
        if name == key.as_bytes() {
            index = Some(i);
            break;
        }
        pos += size;
    }
    let index = index?;
    let ilst = bmff::find(&items, b"ilst")?;
    for item in bmff::children(buf, ilst, 0) {
        if u32::from_be_bytes(item.kind) != index {
            continue;
        }
        let data = bmff::children(buf, &item, 0);
        let data = bmff::find(&data, b"data")?.body(buf);
        // data: type indicator (4), locale (4), value.
        return data.get(8..).map(<[u8]>::to_vec);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A big-endian Apple maker note with the given tags (ASCII values stored after the IFD).
    pub(crate) fn apple_note(tags: &[(u16, &str)]) -> Vec<u8> {
        let mut note = b"Apple iOS\0\x00\x01MM".to_vec();
        note.extend_from_slice(&(tags.len() as u16).to_be_bytes());
        let mut data_off = 16 + tags.len() * 12 + 4;
        let mut data = Vec::new();
        for (tag, value) in tags {
            let mut v = value.as_bytes().to_vec();
            v.push(0);
            note.extend_from_slice(&tag.to_be_bytes());
            note.extend_from_slice(&2u16.to_be_bytes());
            note.extend_from_slice(&(v.len() as u32).to_be_bytes());
            note.extend_from_slice(&(data_off as u32).to_be_bytes());
            data_off += v.len();
            data.extend_from_slice(&v);
        }
        note.extend_from_slice(&0u32.to_be_bytes());
        note.extend_from_slice(&data);
        note
    }

    #[test]
    fn maker_note_identifier() {
        let id = "8F3B2C1A-0D4E-4F5A-9B6C-7D8E9F0A1B2C";
        let note = apple_note(&[(0x0008, "ignored"), (0x0011, id)]);
        assert_eq!(maker_note_content_identifier(&note).as_deref(), Some(id));
        assert_eq!(
            maker_note_content_identifier(&apple_note(&[(0x0008, "x")])),
            None
        );
        assert_eq!(
            maker_note_content_identifier(b"Canon\0\0\0\0\0\0\0\0\0\0\0"),
            None
        );
        // Truncated notes never panic.
        for n in 0..note.len() {
            let _ = maker_note_content_identifier(&note[..n]);
        }
    }

    #[test]
    fn xmp_identifier_attribute_and_element() {
        let id = "A1B2C3D4-0000-4000-8000-000000000001";
        let attr = format!(
            r#"junk<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:Description apple-fi:ContentIdentifier="{id}"/></x:xmpmeta>"#
        );
        assert_eq!(xmp_content_identifier(attr.as_bytes()).as_deref(), Some(id));
        let elem = format!(
            "<x:xmpmeta><MakerApple:ContentIdentifier>{id}</MakerApple:ContentIdentifier></x:xmpmeta>"
        );
        assert_eq!(xmp_content_identifier(elem.as_bytes()).as_deref(), Some(id));
        assert_eq!(
            xmp_content_identifier(b"ContentIdentifier=\"x\""),
            None,
            "no XMP packet"
        );
    }
}
