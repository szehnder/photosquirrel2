//! Small, hand-built media fixtures (orientation JPEGs, DNG with preview, MOV/MP4 metadata, ICO, palette PNG,
//! truncated files) used by the media-analysis tests.
//!
//! Everything is generated from code and deterministic for its inputs (and seed), so no binary blobs are
//! committed. The TIFF/EXIF and ISO-BMFF writers here are deliberately tiny: they emit exactly the structures
//! the tests need, little-endian TIFF and big-endian boxes.

use image::ImageEncoder;

/// Deterministic xorshift64* generator (fixtures must not depend on `rand`'s algorithm choices).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_u8(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

/// `len` random bytes.
pub fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed);
    (0..len).map(|_| r.next_u8()).collect()
}

/// A photo-like RGB8 image: smooth gradients, a few soft shapes and mild noise (compresses like a photo).
pub fn photo_rgb(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let (cx, cy) = (
        r.below(u64::from(w.max(1))) as f32,
        r.below(u64::from(h.max(1))) as f32,
    );
    let base = [r.next_u8(), r.next_u8(), r.next_u8()];
    let mut out = Vec::with_capacity(w as usize * h as usize * 3);
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / wf;
            let fy = y as f32 / hf;
            let d = (((x as f32 - cx) / wf).powi(2) + ((y as f32 - cy) / hf).powi(2)).sqrt();
            let blob = (1.0 - (d * 3.0).min(1.0)) * 90.0;
            let n = (r.next_u8() % 9) as f32 - 4.0;
            let px = [
                f32::from(base[0]) * 0.4 + fx * 120.0 + blob + n,
                f32::from(base[1]) * 0.4 + fy * 110.0 + blob * 0.5 + n,
                f32::from(base[2]) * 0.4 + (1.0 - fx) * 100.0 + n,
            ];
            out.extend(px.iter().map(|v| v.clamp(0.0, 255.0) as u8));
        }
    }
    out
}

/// Encode RGB8 pixels as a baseline JPEG with the standard IJG tables scaled to `quality`.
pub fn encode_jpeg(rgb: &[u8], w: u32, h: u32, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(rgb, w, h, image::ExtendedColorType::Rgb8)
        .expect("in-memory JPEG encode");
    out
}

/// A photo-like JPEG without metadata.
pub fn jpeg(w: u32, h: u32, seed: u64, quality: u8) -> Vec<u8> {
    encode_jpeg(&photo_rgb(w, h, seed), w, h, quality)
}

/// The same fixture image at quality 60 and 92 (quality-estimate scenario).
pub fn quality_pair(seed: u64) -> (Vec<u8>, Vec<u8>) {
    let rgb = photo_rgb(640, 480, seed);
    (
        encode_jpeg(&rgb, 640, 480, 60),
        encode_jpeg(&rgb, 640, 480, 92),
    )
}

/// Insert an `APP1 Exif` segment holding `tiff` right after SOI.
pub fn insert_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(tiff);
    insert_segment(jpeg, 0xE1, &payload)
}

/// Insert an APPn/COM segment right after SOI.
pub fn insert_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() + 2 <= 0xFFFF, "segment too large");
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Cut a file to its first `len` bytes.
pub fn truncate(bytes: &[u8], len: usize) -> Vec<u8> {
    bytes[..len.min(bytes.len())].to_vec()
}

/// Pad a file with trailing zero bytes to `total` bytes (data after EOI, as cameras and editors often leave).
pub fn pad_to(bytes: &[u8], total: usize) -> Vec<u8> {
    let mut v = bytes.to_vec();
    if v.len() < total {
        v.resize(total, 0);
    }
    v
}

// ---------------------------------------------------------------------------------------------------------
// TIFF / EXIF writer
// ---------------------------------------------------------------------------------------------------------

pub const BYTE: u16 = 1;
pub const ASCII: u16 = 2;
pub const SHORT: u16 = 3;
pub const LONG: u16 = 4;
pub const RATIONAL: u16 = 5;
pub const UNDEFINED: u16 = 7;
pub const SRATIONAL: u16 = 10;

/// One IFD entry with its value bytes (little-endian).
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    pub data: Vec<u8>,
}

impl Tag {
    pub fn ascii(tag: u16, s: &str) -> Self {
        let mut data = s.as_bytes().to_vec();
        data.push(0);
        Self {
            tag,
            typ: ASCII,
            count: data.len() as u32,
            data,
        }
    }

    pub fn short(tag: u16, v: u16) -> Self {
        Self::shorts(tag, &[v])
    }

    pub fn shorts(tag: u16, v: &[u16]) -> Self {
        Self {
            tag,
            typ: SHORT,
            count: v.len() as u32,
            data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }

    pub fn long(tag: u16, v: u32) -> Self {
        Self {
            tag,
            typ: LONG,
            count: 1,
            data: v.to_le_bytes().to_vec(),
        }
    }

    pub fn bytes(tag: u16, v: &[u8]) -> Self {
        Self {
            tag,
            typ: BYTE,
            count: v.len() as u32,
            data: v.to_vec(),
        }
    }

    pub fn rational(tag: u16, v: &[(u32, u32)]) -> Self {
        Self {
            tag,
            typ: RATIONAL,
            count: v.len() as u32,
            data: v
                .iter()
                .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
                .collect(),
        }
    }

    pub fn srational(tag: u16, v: &[(i32, i32)]) -> Self {
        Self {
            tag,
            typ: SRATIONAL,
            count: v.len() as u32,
            data: v
                .iter()
                .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
                .collect(),
        }
    }

    pub fn undefined(tag: u16, v: &[u8]) -> Self {
        Self {
            tag,
            typ: UNDEFINED,
            count: v.len() as u32,
            data: v.to_vec(),
        }
    }
}

/// A data blob referenced from an IFD by an offset tag (and optionally a length tag).
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub offset_tag: u16,
    pub len_tag: Option<u16>,
    pub data: Vec<u8>,
}

/// One IFD with its sub-IFDs (pointer tag -> IFD), blobs and the next IFD in the chain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ifd {
    pub tags: Vec<Tag>,
    pub subs: Vec<(u16, Ifd)>,
    pub blobs: Vec<Blob>,
    pub next: Option<Box<Ifd>>,
}

/// Serialize a little-endian TIFF ("II*\0") whose first IFD is `ifd0`.
pub fn write_tiff(ifd0: &Ifd) -> Vec<u8> {
    let mut buf = b"II\x2A\x00\x08\x00\x00\x00".to_vec();
    write_ifd(&mut buf, ifd0);
    buf
}

fn align2(buf: &mut Vec<u8>) {
    if buf.len() % 2 == 1 {
        buf.push(0);
    }
}

fn write_ifd(buf: &mut Vec<u8>, ifd: &Ifd) -> u32 {
    align2(buf);
    let start = buf.len();
    let n = ifd.tags.len()
        + ifd.subs.len()
        + ifd
            .blobs
            .iter()
            .map(|b| 1 + usize::from(b.len_tag.is_some()))
            .sum::<usize>();
    buf.resize(start + 2 + 12 * n + 4, 0);
    // (tag, type, count, 4-byte value field)
    let mut entries: Vec<(u16, u16, u32, [u8; 4])> = Vec::with_capacity(n);
    for t in &ifd.tags {
        let mut field = [0u8; 4];
        if t.data.len() <= 4 {
            field[..t.data.len()].copy_from_slice(&t.data);
        } else {
            align2(buf);
            field = (buf.len() as u32).to_le_bytes();
            buf.extend_from_slice(&t.data);
        }
        entries.push((t.tag, t.typ, t.count, field));
    }
    for b in &ifd.blobs {
        align2(buf);
        let off = buf.len() as u32;
        buf.extend_from_slice(&b.data);
        entries.push((b.offset_tag, LONG, 1, off.to_le_bytes()));
        if let Some(lt) = b.len_tag {
            entries.push((lt, LONG, 1, (b.data.len() as u32).to_le_bytes()));
        }
    }
    for (ptr_tag, sub) in &ifd.subs {
        let off = write_ifd(buf, sub);
        entries.push((*ptr_tag, LONG, 1, off.to_le_bytes()));
    }
    let next = ifd.next.as_ref().map_or(0, |nx| write_ifd(buf, nx));
    entries.sort_by_key(|e| e.0);
    let mut pos = start;
    buf[pos..pos + 2].copy_from_slice(&(n as u16).to_le_bytes());
    pos += 2;
    for (tag, typ, count, field) in entries {
        buf[pos..pos + 2].copy_from_slice(&tag.to_le_bytes());
        buf[pos + 2..pos + 4].copy_from_slice(&typ.to_le_bytes());
        buf[pos + 4..pos + 8].copy_from_slice(&count.to_le_bytes());
        buf[pos + 8..pos + 12].copy_from_slice(&field);
        pos += 12;
    }
    buf[pos..pos + 4].copy_from_slice(&next.to_le_bytes());
    start as u32
}

/// GPS position as EXIF degrees/minutes/seconds rationals.
#[derive(Clone, Debug, PartialEq)]
pub struct GpsSpec {
    pub lat: [(u32, u32); 3],
    pub lat_ref: char,
    pub lon: [(u32, u32); 3],
    pub lon_ref: char,
    pub altitude_m: Option<(u32, u32)>,
}

impl GpsSpec {
    /// 48 deg 51' 29.6" N, 2 deg 17' 40.2" E (the Eiffel tower; the metadata spec scenario).
    pub fn eiffel() -> Self {
        Self {
            lat: [(48, 1), (51, 1), (296, 10)],
            lat_ref: 'N',
            lon: [(2, 1), (17, 1), (402, 10)],
            lon_ref: 'E',
            altitude_m: Some((35, 1)),
        }
    }
}

/// What to put in a fixture's EXIF block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExifSpec {
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub orientation: Option<u16>,
    /// `YYYY:MM:DD HH:MM:SS`
    pub date_time_original: Option<String>,
    pub create_date: Option<String>,
    pub sub_sec_original: Option<String>,
    pub offset_time_original: Option<String>,
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<(u32, u32)>,
    pub iso: Option<u16>,
    pub focal_length: Option<(u32, u32)>,
    pub flash: Option<u16>,
    pub lens_model: Option<String>,
    pub body_serial: Option<String>,
    pub maker_note: Option<Vec<u8>>,
    pub gps: Option<GpsSpec>,
    /// An IFD1 JPEG thumbnail.
    pub thumbnail: Option<Vec<u8>>,
    pub extra_ifd0: Vec<Tag>,
    pub extra_exif: Vec<Tag>,
}

impl ExifSpec {
    /// The camera scenario: Canon EOS 5D, 1/250 s, f/2.8, ISO 400, 50 mm, with Canon maker notes.
    pub fn canon_5d() -> Self {
        Self {
            make: Some("Canon".into()),
            model: Some("Canon EOS 5D".into()),
            software: Some("Firmware Version 1.1.1".into()),
            orientation: Some(1),
            date_time_original: Some("2010:05:01 10:00:00".into()),
            create_date: Some("2010:05:01 10:00:00".into()),
            exposure: Some((1, 250)),
            f_number: Some((28, 10)),
            iso: Some(400),
            focal_length: Some((50, 1)),
            flash: Some(16),
            lens_model: Some("EF50mm f/1.4 USM".into()),
            body_serial: Some("0123456789".into()),
            maker_note: Some(canon_maker_note()),
            ..Default::default()
        }
    }

    /// Number of non-pointer tags this spec writes (IFD0 + Exif + GPS; IFD1 thumbnail tags excluded).
    pub fn tag_count(&self) -> usize {
        let ifd0 = [
            self.make.is_some(),
            self.model.is_some(),
            self.software.is_some(),
            self.orientation.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count()
            + self.extra_ifd0.len();
        let exif = [
            self.date_time_original.is_some(),
            self.create_date.is_some(),
            self.sub_sec_original.is_some(),
            self.offset_time_original.is_some(),
            self.exposure.is_some(),
            self.f_number.is_some(),
            self.iso.is_some(),
            self.focal_length.is_some(),
            self.flash.is_some(),
            self.lens_model.is_some(),
            self.body_serial.is_some(),
            self.maker_note.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count()
            + self.extra_exif.len();
        let gps = self
            .gps
            .as_ref()
            .map_or(0, |g| 4 + usize::from(g.altitude_m.is_some()) * 2);
        ifd0 + exif + gps
    }
}

/// A plausible Canon maker-note blob (a small IFD with a few entries).
pub fn canon_maker_note() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&2u16.to_le_bytes());
    for (tag, val) in [(0x0006u16, 0x1111u32), (0x0007, 0x2222)] {
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&LONG.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&val.to_le_bytes());
    }
    v.extend_from_slice(&[0, 0, 0, 0]);
    v
}

/// The TIFF block for `spec`.
pub fn exif_tiff(spec: &ExifSpec) -> Vec<u8> {
    write_tiff(&exif_ifd0(spec))
}

/// The IFD tree for `spec` (IFD0 with Exif/GPS sub-IFDs and an optional IFD1 thumbnail).
pub fn exif_ifd0(spec: &ExifSpec) -> Ifd {
    let mut ifd0 = Ifd::default();
    if let Some(s) = &spec.make {
        ifd0.tags.push(Tag::ascii(0x010F, s));
    }
    if let Some(s) = &spec.model {
        ifd0.tags.push(Tag::ascii(0x0110, s));
    }
    if let Some(o) = spec.orientation {
        ifd0.tags.push(Tag::short(0x0112, o));
    }
    if let Some(s) = &spec.software {
        ifd0.tags.push(Tag::ascii(0x0131, s));
    }
    ifd0.tags.extend(spec.extra_ifd0.iter().cloned());

    let mut exif = Ifd::default();
    if let Some(e) = spec.exposure {
        exif.tags.push(Tag::rational(0x829A, &[e]));
    }
    if let Some(f) = spec.f_number {
        exif.tags.push(Tag::rational(0x829D, &[f]));
    }
    if let Some(i) = spec.iso {
        exif.tags.push(Tag::short(0x8827, i));
    }
    if let Some(s) = &spec.date_time_original {
        exif.tags.push(Tag::ascii(0x9003, s));
    }
    if let Some(s) = &spec.create_date {
        exif.tags.push(Tag::ascii(0x9004, s));
    }
    if let Some(s) = &spec.offset_time_original {
        exif.tags.push(Tag::ascii(0x9011, s));
    }
    if let Some(f) = spec.flash {
        exif.tags.push(Tag::short(0x9209, f));
    }
    if let Some(f) = spec.focal_length {
        exif.tags.push(Tag::rational(0x920A, &[f]));
    }
    if let Some(m) = &spec.maker_note {
        exif.tags.push(Tag::undefined(0x927C, m));
    }
    if let Some(s) = &spec.sub_sec_original {
        exif.tags.push(Tag::ascii(0x9291, s));
    }
    if let Some(s) = &spec.body_serial {
        exif.tags.push(Tag::ascii(0xA431, s));
    }
    if let Some(s) = &spec.lens_model {
        exif.tags.push(Tag::ascii(0xA434, s));
    }
    exif.tags.extend(spec.extra_exif.iter().cloned());
    if !exif.tags.is_empty() {
        ifd0.subs.push((0x8769, exif));
    }

    if let Some(g) = &spec.gps {
        let mut gps = Ifd::default();
        gps.tags.push(Tag::ascii(0x0001, &g.lat_ref.to_string()));
        gps.tags.push(Tag::rational(0x0002, &g.lat));
        gps.tags.push(Tag::ascii(0x0003, &g.lon_ref.to_string()));
        gps.tags.push(Tag::rational(0x0004, &g.lon));
        if let Some(a) = g.altitude_m {
            gps.tags.push(Tag::bytes(0x0005, &[0]));
            gps.tags.push(Tag::rational(0x0006, &[a]));
        }
        ifd0.subs.push((0x8825, gps));
    }

    if let Some(thumb) = &spec.thumbnail {
        let ifd1 = Ifd {
            tags: vec![Tag::short(0x0103, 6)],
            blobs: vec![Blob {
                offset_tag: 0x0201,
                len_tag: Some(0x0202),
                data: thumb.clone(),
            }],
            ..Default::default()
        };
        ifd0.next = Some(Box::new(ifd1));
    }
    ifd0
}

/// A photo-like JPEG carrying `spec` as EXIF.
pub fn jpeg_with_exif(w: u32, h: u32, seed: u64, quality: u8, spec: &ExifSpec) -> Vec<u8> {
    insert_exif(&jpeg(w, h, seed, quality), &exif_tiff(spec))
}

/// A camera-original JPEG (Canon maker notes) and the same picture re-saved by an editor: maker notes gone,
/// `Software` says GIMP. Returns `(original, edited)`.
pub fn camera_original_and_edited(seed: u64) -> (Vec<u8>, Vec<u8>) {
    let rgb = photo_rgb(800, 600, seed);
    let original = insert_exif(
        &encode_jpeg(&rgb, 800, 600, 92),
        &exif_tiff(&ExifSpec::canon_5d()),
    );
    let edited_spec = ExifSpec {
        software: Some("GIMP 2.10.36".into()),
        maker_note: None,
        ..ExifSpec::canon_5d()
    };
    let edited = insert_exif(&encode_jpeg(&rgb, 800, 600, 85), &exif_tiff(&edited_spec));
    (original, edited)
}

/// A JPEG with exactly 40 EXIF tags (IFD0 + Exif IFD), including the tag 0xC4A5 that EXIF libraries do not
/// name.
pub fn jpeg_with_40_tags(seed: u64) -> (Vec<u8>, usize) {
    let mut spec = ExifSpec::canon_5d();
    spec.extra_ifd0
        .push(Tag::undefined(0xC4A5, b"PrintIM\x000300\x00\x00"));
    spec.extra_ifd0.push(Tag::ascii(0x010E, "fixture"));
    spec.extra_ifd0.push(Tag::rational(0x011A, &[(72, 1)]));
    spec.extra_ifd0.push(Tag::rational(0x011B, &[(72, 1)]));
    spec.extra_ifd0.push(Tag::short(0x0128, 2));
    spec.extra_ifd0
        .push(Tag::ascii(0x0132, "2010:05:01 10:00:00"));
    spec.extra_ifd0.push(Tag::ascii(0x013B, "Fixture Artist"));
    spec.extra_ifd0.push(Tag::short(0x0213, 2));
    spec.extra_ifd0.push(Tag::ascii(0x8298, "Public domain"));
    for code in [
        0x8822u16, 0x8830, 0x9207, 0x9208, 0xA001, 0xA217, 0xA401, 0xA402, 0xA403, 0xA406, 0xA407,
        0xA408, 0xA409, 0xA40A, 0xA40C,
    ] {
        spec.extra_exif.push(Tag::short(code, 1));
    }
    spec.extra_exif.push(Tag::long(0xA002, 640));
    spec.extra_exif.push(Tag::long(0xA003, 480));
    assert_eq!(spec.tag_count(), 40, "fixture must carry exactly 40 tags");
    let count = spec.tag_count();
    (jpeg_with_exif(640, 480, seed, 85, &spec), count)
}

/// A JPEG whose EXIF block's first IFD offset points far outside the block.
pub fn jpeg_with_corrupt_exif(seed: u64) -> Vec<u8> {
    let mut tiff = b"II\x2A\x00".to_vec();
    tiff.extend_from_slice(&0x7FFF_FF00u32.to_le_bytes());
    tiff.extend_from_slice(&[0u8; 32]);
    insert_exif(&jpeg(640, 480, seed, 85), &tiff)
}

// ---------------------------------------------------------------------------------------------------------
// Orientation
// ---------------------------------------------------------------------------------------------------------

/// Display position of stored pixel `(x, y)` in a `w` x `h` stored image under EXIF orientation `o`.
pub fn oriented_position(o: u16, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
    match o {
        2 => (w - 1 - x, y),
        3 => (w - 1 - x, h - 1 - y),
        4 => (x, h - 1 - y),
        5 => (y, x),
        6 => (h - 1 - y, x),
        7 => (h - 1 - y, w - 1 - x),
        8 => (y, w - 1 - x),
        _ => (x, y),
    }
}

/// Quadrant colours of the upright test card: top-left red, top-right green, bottom-left blue, bottom-right
/// white.
pub const CARD_COLOURS: [[u8; 3]; 4] =
    [[220, 30, 30], [30, 200, 30], [30, 30, 220], [235, 235, 235]];

/// The colour of the upright test card at display position `(x, y)`.
pub fn card_colour(x: u32, y: u32, dw: u32, dh: u32) -> [u8; 3] {
    let right = x >= dw / 2;
    let bottom = y >= dh / 2;
    CARD_COLOURS[usize::from(bottom) * 2 + usize::from(right)]
}

/// Stored RGB pixels and stored dimensions of the test card that displays as `dw` x `dh` once EXIF
/// orientation `o` is applied.
pub fn oriented_card_rgb(dw: u32, dh: u32, o: u16) -> (Vec<u8>, u32, u32) {
    let (sw, sh) = if (5..=8).contains(&o) {
        (dh, dw)
    } else {
        (dw, dh)
    };
    let mut rgb = vec![0u8; sw as usize * sh as usize * 3];
    for y in 0..sh {
        for x in 0..sw {
            let (dx, dy) = oriented_position(o, x, y, sw, sh);
            let c = card_colour(dx, dy, dw, dh);
            let i = (y as usize * sw as usize + x as usize) * 3;
            rgb[i..i + 3].copy_from_slice(&c);
        }
    }
    (rgb, sw, sh)
}

/// A JPEG of the test card stored so that it displays upright as `dw` x `dh` with EXIF orientation `o`.
pub fn orientation_jpeg(dw: u32, dh: u32, o: u16) -> Vec<u8> {
    let (rgb, sw, sh) = oriented_card_rgb(dw, dh, o);
    let spec = ExifSpec {
        orientation: Some(o),
        ..Default::default()
    };
    insert_exif(&encode_jpeg(&rgb, sw, sh, 90), &exif_tiff(&spec))
}

/// Orientation JPEGs 1-8, each displaying as 320x240.
pub fn orientation_set() -> Vec<(u16, Vec<u8>)> {
    (1..=8)
        .map(|o| (o, orientation_jpeg(320, 240, o)))
        .collect()
}

/// A JPEG with a small (160x120) EXIF thumbnail; the main image is `w` x `h`.
pub fn jpeg_with_small_exif_thumbnail(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let thumb = jpeg(160, 120, seed ^ 0xABCD, 80);
    let spec = ExifSpec {
        thumbnail: Some(thumb),
        ..ExifSpec::canon_5d()
    };
    jpeg_with_exif(w, h, seed, 85, &spec)
}

// ---------------------------------------------------------------------------------------------------------
// DNG
// ---------------------------------------------------------------------------------------------------------

/// A minimal DNG: IFD0 is a `pw` x `ph` JPEG preview (NewSubfileType 1), a SubIFD holds `raw_w` x `raw_h`
/// 16-bit uncompressed CFA data. Returns the file bytes.
pub fn dng_with_preview(pw: u32, ph: u32, raw_w: u32, raw_h: u32, seed: u64) -> Vec<u8> {
    let preview = jpeg(pw, ph, seed, 80);
    let raw_data = vec![0u8; raw_w as usize * raw_h as usize * 2];
    let raw = Ifd {
        tags: vec![
            Tag::long(0x00FE, 0),
            Tag::long(0x0100, raw_w),
            Tag::long(0x0101, raw_h),
            Tag::short(0x0102, 16),
            Tag::short(0x0103, 1),
            Tag::short(0x0106, 32803),
            Tag::short(0x0115, 1),
            Tag::long(0x0116, raw_h),
            Tag::shorts(0x828D, &[2, 2]),
            Tag::bytes(0x828E, &[0, 1, 1, 2]),
        ],
        blobs: vec![Blob {
            offset_tag: 0x0111,
            len_tag: Some(0x0117),
            data: raw_data,
        }],
        ..Default::default()
    };
    let exif = Ifd {
        tags: vec![
            Tag::rational(0x829A, &[(1, 125)]),
            Tag::short(0x8827, 200),
            Tag::ascii(0x9003, "2016:08:20 14:05:09"),
        ],
        ..Default::default()
    };
    let ifd0 = Ifd {
        tags: vec![
            Tag::long(0x00FE, 1),
            Tag::long(0x0100, pw),
            Tag::long(0x0101, ph),
            Tag::shorts(0x0102, &[8, 8, 8]),
            Tag::short(0x0103, 7),
            Tag::short(0x0106, 6),
            Tag::ascii(0x010F, "FixtureCam"),
            Tag::ascii(0x0110, "FixtureCam DNG-1"),
            Tag::short(0x0112, 1),
            Tag::short(0x0115, 3),
            Tag::long(0x0116, ph),
            Tag::bytes(0xC612, &[1, 4, 0, 0]),
            Tag::ascii(0xC614, "FixtureCam DNG-1"),
        ],
        blobs: vec![Blob {
            offset_tag: 0x0111,
            len_tag: Some(0x0117),
            data: preview,
        }],
        subs: vec![(0x014A, raw), (0x8769, exif)],
        next: None,
    };
    write_tiff(&ifd0)
}

// ---------------------------------------------------------------------------------------------------------
// QuickTime / MP4
// ---------------------------------------------------------------------------------------------------------

/// Container flavour of [`movie`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovieBrand {
    QuickTime,
    Mp4,
}

/// What to put in a movie fixture.
#[derive(Clone, Debug, PartialEq)]
pub struct MovieSpec {
    pub brand: MovieBrand,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    /// Seconds since 1970-01-01T00:00:00Z.
    pub creation_unix: Option<i64>,
    pub make: Option<String>,
    pub model: Option<String>,
    /// Clockwise rotation in degrees (0, 90, 180, 270) in the track matrix.
    pub rotation: u16,
    pub fps: u32,
    /// Put `moov` after `mdat` (as cameras do without fast-start) and pad `mdat` to this many bytes.
    pub moov_at_end_with_mdat: Option<usize>,
}

impl MovieSpec {
    /// The phone-video scenario: 1920x1080, 12.5 s, Apple, 2019-07-04T18:30:00Z.
    pub fn iphone() -> Self {
        Self {
            brand: MovieBrand::QuickTime,
            width: 1920,
            height: 1080,
            duration_ms: 12_500,
            creation_unix: Some(1_562_265_000),
            make: Some("Apple".into()),
            model: Some("iPhone XS".into()),
            rotation: 0,
            fps: 30,
            moov_at_end_with_mdat: None,
        }
    }
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn matrix(rotation: u16, w: u32, h: u32) -> [u8; 36] {
    // 16.16 a, b, u(2.30), c, d, v(2.30), tx, ty, w(2.30)
    let one = 0x0001_0000i32;
    let neg = -one;
    let (a, b, c, d, tx, ty) = match rotation {
        90 => (0, one, neg, 0, (h as i32) << 16, 0),
        180 => (neg, 0, 0, neg, (w as i32) << 16, (h as i32) << 16),
        270 => (0, neg, one, 0, 0, (w as i32) << 16),
        _ => (one, 0, 0, one, 0, 0),
    };
    let vals = [a, b, 0, c, d, 0, tx, ty, 0x4000_0000];
    let mut out = [0u8; 36];
    for (i, v) in vals.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// A QuickTime or MP4 file with one video track and (QuickTime) `mdta` make/model/creation keys.
pub fn movie(spec: &MovieSpec) -> Vec<u8> {
    const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;
    let timescale: u32 = 600;
    let duration = (spec.duration_ms * u64::from(timescale) / 1000) as u32;
    let created = spec
        .creation_unix
        .map_or(0, |t| (t + MAC_EPOCH_OFFSET) as u32);

    let ftyp = match spec.brand {
        MovieBrand::QuickTime => bx(b"ftyp", b"qt  \x00\x00\x02\x00qt  "),
        MovieBrand::Mp4 => bx(b"ftyp", b"isom\x00\x00\x02\x00isomiso2avc1mp41"),
    };

    let mut mvhd = vec![0u8; 4];
    mvhd.extend_from_slice(&created.to_be_bytes());
    mvhd.extend_from_slice(&created.to_be_bytes());
    mvhd.extend_from_slice(&timescale.to_be_bytes());
    mvhd.extend_from_slice(&duration.to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0u8; 10]);
    mvhd.extend_from_slice(&matrix(0, 0, 0));
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend_from_slice(&2u32.to_be_bytes());

    let mut tkhd = vec![0, 0, 0, 3];
    tkhd.extend_from_slice(&created.to_be_bytes());
    tkhd.extend_from_slice(&created.to_be_bytes());
    tkhd.extend_from_slice(&1u32.to_be_bytes());
    tkhd.extend_from_slice(&[0u8; 4]);
    tkhd.extend_from_slice(&duration.to_be_bytes());
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.extend_from_slice(&[0u8; 4]); // layer, alternate group
    tkhd.extend_from_slice(&[0u8; 4]); // volume, reserved
    tkhd.extend_from_slice(&matrix(spec.rotation, spec.width, spec.height));
    tkhd.extend_from_slice(&(spec.width << 16).to_be_bytes());
    tkhd.extend_from_slice(&(spec.height << 16).to_be_bytes());

    let mut mdhd = vec![0u8; 4];
    mdhd.extend_from_slice(&created.to_be_bytes());
    mdhd.extend_from_slice(&created.to_be_bytes());
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&duration.to_be_bytes());
    mdhd.extend_from_slice(&[0x55, 0xC4, 0, 0]);

    let mut hdlr = vec![0u8; 4];
    hdlr.extend_from_slice(b"mhlr");
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0u8; 12]);
    hdlr.push(0);

    let mut avc1 = vec![0u8; 6];
    avc1.extend_from_slice(&1u16.to_be_bytes());
    avc1.extend_from_slice(&[0u8; 16]);
    avc1.extend_from_slice(&(spec.width as u16).to_be_bytes());
    avc1.extend_from_slice(&(spec.height as u16).to_be_bytes());
    avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1.extend_from_slice(&[0u8; 4]);
    avc1.extend_from_slice(&1u16.to_be_bytes());
    avc1.extend_from_slice(&[0u8; 32]);
    avc1.extend_from_slice(&0x0018u16.to_be_bytes());
    avc1.extend_from_slice(&0xFFFFu16.to_be_bytes());
    let mut stsd = vec![0u8; 4];
    stsd.extend_from_slice(&1u32.to_be_bytes());
    stsd.extend_from_slice(&bx(b"avc1", &avc1));

    let fps = spec.fps.max(1);
    let frames = (spec.duration_ms * u64::from(fps) / 1000) as u32;
    let mut stts = vec![0u8; 4];
    stts.extend_from_slice(&1u32.to_be_bytes());
    stts.extend_from_slice(&frames.to_be_bytes());
    stts.extend_from_slice(&(timescale / fps).to_be_bytes());

    let stbl = [bx(b"stsd", &stsd), bx(b"stts", &stts)].concat();
    let minf = [
        bx(b"vmhd", &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        bx(b"stbl", &stbl),
    ]
    .concat();
    let mdia = [bx(b"mdhd", &mdhd), bx(b"hdlr", &hdlr), bx(b"minf", &minf)].concat();
    let trak = [bx(b"tkhd", &tkhd), bx(b"mdia", &mdia)].concat();

    let mut moov_body = [bx(b"mvhd", &mvhd), bx(b"trak", &trak)].concat();

    let mut keys: Vec<(&str, String)> = Vec::new();
    if let Some(m) = &spec.make {
        keys.push(("com.apple.quicktime.make", m.clone()));
    }
    if let Some(m) = &spec.model {
        keys.push(("com.apple.quicktime.model", m.clone()));
    }
    if let Some(t) = spec.creation_unix {
        keys.push(("com.apple.quicktime.creationdate", iso8601_utc(t)));
    }
    if spec.brand == MovieBrand::QuickTime && !keys.is_empty() {
        let mut mhdlr = vec![0u8; 4];
        mhdlr.extend_from_slice(&[0u8; 4]);
        mhdlr.extend_from_slice(b"mdta");
        mhdlr.extend_from_slice(&[0u8; 12]);
        mhdlr.push(0);
        let mut k = vec![0u8; 4];
        k.extend_from_slice(&(keys.len() as u32).to_be_bytes());
        for (name, _) in &keys {
            k.extend_from_slice(&((name.len() + 8) as u32).to_be_bytes());
            k.extend_from_slice(b"mdta");
            k.extend_from_slice(name.as_bytes());
        }
        let mut ilst = Vec::new();
        for (i, (_, value)) in keys.iter().enumerate() {
            let mut data = 1u32.to_be_bytes().to_vec();
            data.extend_from_slice(&[0u8; 4]);
            data.extend_from_slice(value.as_bytes());
            let data_box = bx(b"data", &data);
            ilst.extend_from_slice(&((data_box.len() + 8) as u32).to_be_bytes());
            ilst.extend_from_slice(&((i + 1) as u32).to_be_bytes());
            ilst.extend_from_slice(&data_box);
        }
        let meta = [bx(b"hdlr", &mhdlr), bx(b"keys", &k), bx(b"ilst", &ilst)].concat();
        moov_body.extend_from_slice(&bx(b"meta", &meta));
    }
    let moov = bx(b"moov", &moov_body);
    match spec.moov_at_end_with_mdat {
        None => [ftyp, moov, bx(b"mdat", &[0u8; 64])].concat(),
        Some(n) => [ftyp, bx(b"mdat", &vec![0u8; n]), moov].concat(),
    }
}

/// `YYYY-MM-DDTHH:MM:SS+00:00` for a Unix time.
pub fn iso8601_utc(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = ps_model::civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

// ---------------------------------------------------------------------------------------------------------
// PNG / ICO
// ---------------------------------------------------------------------------------------------------------

/// A palette (indexed) PNG with a transparent palette entry (`tRNS`).
pub fn palette_png(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let palette: Vec<u8> = (0..16 * 3).map(|_| r.next_u8()).collect();
    let mut trns = vec![255u8; 16];
    trns[0] = 0;
    let data: Vec<u8> = (0..w * h)
        .map(|i| ((i / 37 + i % w / 64) % 16) as u8)
        .collect();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(palette);
        enc.set_trns(trns);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(&data).expect("png data");
    }
    out
}

/// An opaque RGB PNG without metadata.
pub fn rgb_png(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let rgb = photo_rgb(w, h, seed);
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .expect("png encode");
    out
}

/// An RGBA PNG with an alpha gradient.
pub fn rgba_png(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128, ((x + y) % 256) as u8]);
        }
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&px, w, h, image::ExtendedColorType::Rgba8)
        .expect("png encode");
    out
}

/// An ICO with one PNG-compressed `size` x `size` entry (256 is stored as 0 in the directory).
pub fn ico(size: u32) -> Vec<u8> {
    let png = rgba_png(size, size);
    let mut v = vec![0, 0, 1, 0, 1, 0];
    let dim = if size >= 256 { 0 } else { size as u8 };
    v.extend_from_slice(&[dim, dim, 0, 0]);
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(&(png.len() as u32).to_le_bytes());
    v.extend_from_slice(&22u32.to_le_bytes());
    v.extend_from_slice(&png);
    v
}

/// A small GIF (palette image).
pub fn gif(w: u16, h: u16) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        let px: Vec<u8> = (0..u32::from(w) * u32::from(h))
            .flat_map(|i| [(i % 251) as u8, 40, 90, 255])
            .collect();
        let img = image::RgbaImage::from_raw(u32::from(w), u32::from(h), px).expect("gif buffer");
        enc.encode_frame(image::Frame::new(img))
            .expect("gif encode");
    }
    out
}

/// A tiny SVG document.
pub fn svg() -> Vec<u8> {
    br#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48" viewBox="0 0 64 48"><rect width="64" height="48" fill="red"/></svg>
"#
    .to_vec()
}

/// One named fixture of the media set.
#[derive(Clone, Debug)]
pub struct MediaFixture {
    /// Relative path (may include folders, e.g. cache locations).
    pub path: String,
    pub bytes: Vec<u8>,
}

/// The media fixture set used by the classification scenarios: one of each kind.
pub fn media_set(seed: u64) -> Vec<MediaFixture> {
    let f = |p: &str, b: Vec<u8>| MediaFixture {
        path: p.to_owned(),
        bytes: b,
    };
    let mut v = vec![
        f(
            "camera/IMG_0001.JPG",
            jpeg_with_exif(1200, 800, seed, 90, &ExifSpec::canon_5d()),
        ),
        f("misc/plain.jpg", jpeg(1500, 1000, seed + 1, 85)),
        f("misc/tiny.png", rgb_png(180, 500, seed + 2)),
        f("misc/palette.png", palette_png(1024, 768, seed + 3)),
        f("misc/favicon.ico", ico(256)),
        f("misc/anim.gif", gif(64, 64)),
        f("misc/logo.svg", svg()),
        f(
            "raw/IMG_0002.DNG",
            dng_with_preview(1024, 683, 96, 64, seed + 4),
        ),
        f("video/IMG_0003.MOV", movie(&MovieSpec::iphone())),
        f(
            "video/clip.mp4",
            movie(&MovieSpec {
                brand: MovieBrand::Mp4,
                width: 1280,
                height: 720,
                duration_ms: 1_500,
                creation_unix: None,
                make: None,
                model: None,
                rotation: 0,
                fps: 30,
                moov_at_end_with_mdat: None,
            }),
        ),
        f("misc/random.jpg", random_bytes(4096, seed + 5)),
        f(
            "misc/truncated.jpg",
            truncate(&jpeg(800, 600, seed + 6, 85), 1000),
        ),
        f("misc/corrupt_exif.jpg", jpeg_with_corrupt_exif(seed + 7)),
        f(
            "Lightroom/Catalog Previews.lrdata/0/A1/abc.jpg",
            jpeg_with_exif(1024, 683, seed + 8, 80, &ExifSpec::canon_5d()),
        ),
        f(
            "Pictures/.thumbnails/normal/x.png",
            rgb_png(256, 256, seed + 9),
        ),
    ];
    for (o, b) in orientation_set() {
        v.push(f(&format!("orient/o{o}.jpg"), b));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oriented_positions_are_bijective() {
        for o in 1..=8u16 {
            let (w, h) = (5u32, 3u32);
            let (dw, dh) = if (5..=8).contains(&o) { (h, w) } else { (w, h) };
            let mut seen = std::collections::HashSet::new();
            for y in 0..h {
                for x in 0..w {
                    let (dx, dy) = oriented_position(o, x, y, w, h);
                    assert!(dx < dw && dy < dh, "o{o}");
                    assert!(seen.insert((dx, dy)));
                }
            }
        }
    }

    #[test]
    fn tiff_writer_layout() {
        let t = exif_tiff(&ExifSpec::canon_5d());
        assert_eq!(&t[..4], b"II\x2A\x00");
        let n = u16::from_le_bytes([t[8], t[9]]);
        assert_eq!(usize::from(n), 4 + 1); // make, model, orientation, software + Exif pointer
    }
}
