//! Camera models and bodies, and the camera-like EXIF block (IFD0, Exif IFD, GPS IFD, IFD1 thumbnail,
//! opaque MakerNote) written into originals.

use ps_model::civil_from_days;

use crate::rng::{CorpusRng, Rng, mix64};
use crate::tiff::{Endian, Ifd, Value, write_tiff};

/// A camera model as the corpus knows it.
#[derive(Debug)]
pub struct CameraModel {
    pub make: &'static str,
    pub model: &'static str,
    pub endian: Endian,
    /// Stored (sensor) dimensions for the small/medium presets (landscape).
    pub dims: (u32, u32),
    pub phone: bool,
    /// Vendor maker-note header (the rest of the blob is opaque).
    pub maker_header: &'static [u8],
    pub firmware: &'static str,
    pub lens: &'static str,
    /// Focal length range in tenths of mm, and 35 mm crop factor x10.
    pub focal: (u32, u32),
    pub crop_x10: u32,
    /// Digits written to SubSecTimeOriginal; 0 = the camera writes none.
    pub subsec_digits: u8,
    /// Writes OffsetTimeOriginal.
    pub offset_time: bool,
    pub years: (i32, i32),
    pub naming: Naming,
    pub dcim: &'static str,
    pub can_raw: bool,
    pub can_video: bool,
}

/// File naming convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    /// `<prefix>1234.<ext>` with a 4-digit counter.
    Counter(&'static str, &'static str),
    /// `20210704_183005.jpg` (Samsung).
    DateTime,
    /// `PXL_20210704_183005123.jpg` (Pixel).
    Pixel,
}

pub static MODELS: &[CameraModel] = &[
    CameraModel {
        make: "Canon",
        model: "Canon EOS 5D Mark III",
        endian: Endian::Little,
        dims: (2400, 1600),
        phone: false,
        maker_header: b"\x1d\x00\x01\x00\x03\x00\x31\x00\x00\x00",
        firmware: "Firmware Version 1.3.5",
        lens: "EF24-105mm f/4L IS USM",
        focal: (240, 1050),
        crop_x10: 10,
        subsec_digits: 2,
        offset_time: false,
        years: (2012, 2020),
        naming: Naming::Counter("IMG_", "JPG"),
        dcim: "100CANON",
        can_raw: true,
        can_video: false,
    },
    CameraModel {
        make: "NIKON CORPORATION",
        model: "NIKON D750",
        endian: Endian::Big,
        dims: (2304, 1536),
        phone: false,
        maker_header: b"Nikon\x00\x02\x11\x00\x00MM\x00\x2a\x00\x00\x00\x08",
        firmware: "Ver.1.10",
        lens: "24.0-120.0 mm f/4.0",
        focal: (240, 1200),
        crop_x10: 10,
        subsec_digits: 2,
        offset_time: true,
        years: (2014, 2024),
        naming: Naming::Counter("DSC_", "JPG"),
        dcim: "100NCD750",
        can_raw: true,
        can_video: false,
    },
    CameraModel {
        make: "Apple",
        model: "iPhone 12",
        endian: Endian::Big,
        dims: (2016, 1512),
        phone: true,
        maker_header: b"Apple iOS\x00\x00\x01MM",
        firmware: "14.4",
        lens: "iPhone 12 back dual wide camera 4.2mm f/1.6",
        focal: (42, 42),
        crop_x10: 62,
        subsec_digits: 3,
        offset_time: true,
        years: (2020, 2024),
        naming: Naming::Counter("IMG_", "JPG"),
        dcim: "100APPLE",
        can_raw: false,
        can_video: true,
    },
    CameraModel {
        make: "SONY",
        model: "ILCE-7M3",
        endian: Endian::Little,
        dims: (2208, 1472),
        phone: false,
        maker_header: b"SONY DSC \x00\x00\x00",
        firmware: "ILCE-7M3 v3.01",
        lens: "FE 28-70mm F3.5-5.6 OSS",
        focal: (280, 700),
        crop_x10: 10,
        subsec_digits: 3,
        offset_time: true,
        years: (2018, 2024),
        naming: Naming::Counter("DSC0", "JPG"),
        dcim: "100MSDCF",
        can_raw: true,
        can_video: true,
    },
    CameraModel {
        make: "FUJIFILM",
        model: "X-T3",
        endian: Endian::Little,
        dims: (2160, 1440),
        phone: false,
        maker_header: b"FUJIFILM\x0c\x00\x00\x00",
        firmware: "Digital Camera X-T3 Ver4.00",
        lens: "XF18-55mmF2.8-4 R LM OIS",
        focal: (180, 550),
        crop_x10: 15,
        subsec_digits: 2,
        offset_time: true,
        years: (2018, 2024),
        naming: Naming::Counter("DSCF", "JPG"),
        dcim: "100_FUJI",
        can_raw: true,
        can_video: false,
    },
    CameraModel {
        make: "samsung",
        model: "SM-G991B",
        endian: Endian::Little,
        dims: (2000, 1500),
        phone: true,
        maker_header: b"\x07\x00\x01\x00\x07\x00\x04\x00\x00\x00\x30\x31\x30\x30",
        firmware: "G991BXXU3AUE1",
        lens: "",
        focal: (54, 54),
        crop_x10: 48,
        subsec_digits: 3,
        offset_time: true,
        years: (2021, 2024),
        naming: Naming::DateTime,
        dcim: "Camera",
        can_raw: false,
        can_video: true,
    },
    CameraModel {
        make: "Canon",
        model: "Canon PowerShot SD1000",
        endian: Endian::Little,
        dims: (1632, 1224),
        phone: false,
        maker_header: b"\x1a\x00\x01\x00\x03\x00\x2e\x00\x00\x00",
        firmware: "Firmware Version 1.00",
        lens: "",
        focal: (58, 174),
        crop_x10: 60,
        subsec_digits: 0,
        offset_time: false,
        years: (2007, 2013),
        naming: Naming::Counter("IMG_", "JPG"),
        dcim: "100CANON",
        can_raw: false,
        can_video: false,
    },
    CameraModel {
        make: "OLYMPUS IMAGING CORP.",
        model: "E-M10",
        endian: Endian::Little,
        dims: (1840, 1380),
        phone: false,
        maker_header: b"OLYMPUS\x00II\x03\x00",
        firmware: "Version 1.2",
        lens: "OLYMPUS M.14-42mm F3.5-5.6 EZ",
        focal: (140, 420),
        crop_x10: 20,
        subsec_digits: 0,
        offset_time: false,
        years: (2014, 2020),
        naming: Naming::Counter("P", "JPG"),
        dcim: "100OLYMP",
        can_raw: false,
        can_video: false,
    },
    CameraModel {
        make: "Google",
        model: "Pixel 4a",
        endian: Endian::Little,
        dims: (2048, 1536),
        phone: true,
        maker_header: b"HDRP\x02\x00",
        firmware: "HDR+ 1.0.320048813zd",
        lens: "Google Pixel 4a back camera 4.38mm f/1.73",
        focal: (44, 44),
        crop_x10: 62,
        subsec_digits: 3,
        offset_time: true,
        years: (2020, 2023),
        naming: Naming::Pixel,
        dcim: "Camera",
        can_raw: false,
        can_video: true,
    },
];

/// A physical camera (model + serial) owned by someone.
#[derive(Clone, Debug)]
pub struct Body {
    pub model: &'static CameraModel,
    pub serial: String,
    pub owner: &'static str,
    /// Per-body maker-note seed (constant part of the blob).
    pub maker_seed: u64,
    pub counter_start: u32,
}

pub const OWNERS: &[&str] = &["sam", "alex", "mom", "dad", "grandma"];

impl Body {
    pub fn new(model: &'static CameraModel, rng: &mut CorpusRng) -> Body {
        let serial = if model.phone {
            // Phones do not write a body serial in EXIF; keep a unique id for truth/debugging only.
            String::new()
        } else {
            format!("{:0>10}", rng.random_range(100_000_000u64..9_999_999_999))
        };
        Body {
            model,
            serial,
            owner: OWNERS[rng.random_range(0..OWNERS.len())],
            maker_seed: rng.random(),
            counter_start: rng.random_range(100..7000),
        }
    }
}

/// When and how a shot was exposed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capture {
    /// Wall-clock seconds (`CaptureTime::local_seconds` convention).
    pub local: i64,
    /// Sub-second milliseconds actually elapsed (written only if the camera writes sub-seconds).
    pub millis: u16,
    pub offset_min: i16,
}

impl Capture {
    pub fn utc(&self) -> i64 {
        self.local - i64::from(self.offset_min) * 60
    }

    /// `YYYY:MM:DD HH:MM:SS`.
    pub fn exif(&self) -> String {
        exif_datetime(self.local)
    }

    pub fn ymd(&self) -> (i32, u8, u8) {
        civil_from_days(self.local.div_euclid(86_400))
    }

    pub fn hms(&self) -> (u8, u8, u8) {
        let s = self.local.rem_euclid(86_400);
        ((s / 3600) as u8, ((s / 60) % 60) as u8, (s % 60) as u8)
    }

    pub fn plus_millis(&self, ms: i64) -> Capture {
        let total = self.local * 1000 + i64::from(self.millis) + ms;
        Capture {
            local: total.div_euclid(1000),
            millis: total.rem_euclid(1000) as u16,
            offset_min: self.offset_min,
        }
    }

    pub fn offset_string(&self) -> String {
        let o = self.offset_min;
        let sign = if o < 0 { '-' } else { '+' };
        format!(
            "{sign}{:02}:{:02}",
            o.unsigned_abs() / 60,
            o.unsigned_abs() % 60
        )
    }
}

pub fn exif_datetime(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}:{m:02}:{d:02} {:02}:{:02}:{:02}",
        s / 3600,
        (s / 60) % 60,
        s % 60
    )
}

/// GPS position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gps {
    /// Micro-degrees (integers keep the corpus platform-independent).
    pub lat_e6: i64,
    pub lon_e6: i64,
    pub alt_dm: u32,
}

/// Exposure settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Exposure {
    pub time: (u32, u32),
    pub fnumber_x10: u32,
    pub iso: u16,
    pub focal_x10: u32,
    pub flash: bool,
}

impl Exposure {
    pub fn random(model: &CameraModel, rng: &mut CorpusRng) -> Exposure {
        const TIMES: &[(u32, u32)] = &[
            (1, 2000),
            (1, 1000),
            (1, 500),
            (1, 250),
            (1, 125),
            (1, 60),
            (1, 30),
            (1, 15),
        ];
        const F: &[u32] = &[16, 18, 20, 28, 40, 56, 80, 110];
        const ISO: &[u16] = &[50, 100, 200, 400, 800, 1600, 3200];
        let fnum = if model.phone {
            16 + rng.random_range(0..2)
        } else {
            F[rng.random_range(2..F.len())]
        };
        Exposure {
            time: TIMES[rng.random_range(0..TIMES.len())],
            fnumber_x10: fnum,
            iso: ISO[rng.random_range(0..ISO.len())],
            focal_x10: rng.random_range(model.focal.0..=model.focal.1),
            flash: rng.random_bool(0.1),
        }
    }
}

/// Everything written into a shot's EXIF block.
#[derive(Clone, Debug)]
pub struct ExifSpec<'a> {
    pub body: &'a Body,
    pub capture: &'a Capture,
    pub exposure: &'a Exposure,
    pub gps: Option<Gps>,
    pub orientation: u16,
    /// Stored pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// Per-shot maker-note variation (shutter count etc.); `None` drops the MakerNote (edited copies).
    pub maker_note: Option<u64>,
    /// Software tag override (editors); `None` writes the camera firmware.
    pub software: Option<&'a str>,
    /// IFD1 thumbnail JPEG.
    pub thumbnail: Option<Vec<u8>>,
    /// Write SubSecTimeOriginal (if the camera does).
    pub subsec: bool,
    /// Canon file number (`directory * 10000 + index`, tag 0x0008): the maker note is then a well-formed
    /// Canon IFD recording it (converted RAW bursts, ADR-0105) instead of the opaque blob.
    pub file_number: Option<u32>,
}

/// The opaque MakerNote blob: vendor header, body-constant block, per-shot block.
pub fn maker_note(body: &Body, shot_seed: u64) -> Vec<u8> {
    let mut out = body.model.maker_header.to_vec();
    let const_len = 900 + (body.maker_seed % 1400) as usize;
    let var_len = 300 + (shot_seed % 500) as usize;
    let mut s = body.maker_seed;
    for _ in 0..const_len / 8 {
        s = mix64(s);
        out.extend_from_slice(&s.to_le_bytes());
    }
    // Readable firmware/serial strings, as many maker notes carry.
    out.extend_from_slice(body.model.firmware.as_bytes());
    out.push(0);
    out.extend_from_slice(body.serial.as_bytes());
    out.push(0);
    let mut s = shot_seed;
    for _ in 0..var_len / 8 {
        s = mix64(s);
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// A well-formed Canon maker note: an IFD whose values are all inline (so it reads the same wherever it is
/// placed, also inside `DNGPrivateData`) recording `FileNumber` (0x0008), the body's serial (0x000C) and a
/// per-shot word, followed by opaque per-shot bytes.
pub fn canon_maker_note(body: &Body, shot_seed: u64, file_number: u32) -> Vec<u8> {
    let e = body.model.endian;
    let u16b = |v: u16| match e {
        Endian::Little => v.to_le_bytes(),
        Endian::Big => v.to_be_bytes(),
    };
    let u32b = |v: u32| match e {
        Endian::Little => v.to_le_bytes(),
        Endian::Big => v.to_be_bytes(),
    };
    let serial = body.serial.parse::<u64>().unwrap_or(0) as u32;
    let entries: [(u16, u16, u32, u32); 3] = [
        (0x0008, 4, 1, file_number),
        (0x000C, 4, 1, serial),
        (0x0013, 4, 1, (mix64(shot_seed) & 0xFFFF_FFFF) as u32),
    ];
    let mut out = Vec::new();
    out.extend_from_slice(&u16b(entries.len() as u16));
    for (tag, typ, count, value) in entries {
        out.extend_from_slice(&u16b(tag));
        out.extend_from_slice(&u16b(typ));
        out.extend_from_slice(&u32b(count));
        out.extend_from_slice(&u32b(value));
    }
    out.extend_from_slice(&u32b(0));
    let mut s = shot_seed ^ body.maker_seed;
    for _ in 0..32 {
        s = mix64(s);
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// The maker note `spec` asks for, if any.
pub fn spec_maker_note(spec: &ExifSpec<'_>) -> Option<Vec<u8>> {
    let seed = spec.maker_note?;
    Some(match spec.file_number {
        Some(n) => canon_maker_note(spec.body, seed, n),
        None => maker_note(spec.body, seed),
    })
}

fn dms(e6: i64) -> Value {
    let a = e6.unsigned_abs();
    let deg = (a / 1_000_000) as u32;
    let min_e6 = (a % 1_000_000) * 60;
    let min = (min_e6 / 1_000_000) as u32;
    let sec_e4 = ((min_e6 % 1_000_000) * 60 / 100) as u32; // seconds * 10^4
    Value::Rational(vec![(deg, 1), (min, 1), (sec_e4, 10_000)])
}

/// The Exif IFD and (if the shot has GPS) the GPS IFD.
pub fn exif_ifds(spec: &ExifSpec<'_>) -> (Ifd, Option<Ifd>) {
    let m = spec.body.model;
    let c = spec.capture;
    let mut ex = Ifd::new();
    let e = spec.exposure;
    ex.rational(0x829A, e.time.0, e.time.1)
        .rational(0x829D, e.fnumber_x10, 10)
        .short(0x8822, 2)
        .short(0x8827, e.iso)
        .set(0x9000, Value::Undefined(b"0232".to_vec()))
        .ascii(0x9003, c.exif())
        .ascii(0x9004, c.exif())
        .set(0x9204, Value::SRational(vec![(0, 3)]))
        .short(0x9207, 5)
        .short(0x9209, if e.flash { 0x19 } else { 0x10 })
        .rational(0x920A, e.focal_x10, 10)
        .set(0xA000, Value::Undefined(b"0100".to_vec()))
        .short(0xA001, 1)
        .long(0xA002, spec.width)
        .long(0xA003, spec.height)
        .short(0xA405, ((e.focal_x10 * m.crop_x10 + 50) / 100) as u16);
    if m.offset_time {
        ex.ascii(0x9010, c.offset_string())
            .ascii(0x9011, c.offset_string());
    }
    if spec.subsec && m.subsec_digits > 0 {
        let s = match m.subsec_digits {
            2 => format!("{:02}", c.millis / 10),
            _ => format!("{:03}", c.millis),
        };
        ex.ascii(0x9291, s.clone()).ascii(0x9292, s);
    }
    if let Some(note) = spec_maker_note(spec) {
        ex.set(0x927C, Value::Undefined(note));
    }
    if !spec.body.serial.is_empty() {
        ex.ascii(0xA431, spec.body.serial.clone());
    }
    if !m.lens.is_empty() {
        ex.ascii(0xA433, m.make).ascii(0xA434, m.lens);
    }

    let gps = spec.gps.map(|g| {
        let mut gps = Ifd::new();
        let utc = c.utc();
        let s = utc.rem_euclid(86_400) as u32;
        gps.set(0x0000, Value::Byte(vec![2, 3, 0, 0]))
            .ascii(0x0001, if g.lat_e6 >= 0 { "N" } else { "S" })
            .set(0x0002, dms(g.lat_e6))
            .ascii(0x0003, if g.lon_e6 >= 0 { "E" } else { "W" })
            .set(0x0004, dms(g.lon_e6))
            .set(0x0005, Value::Byte(vec![0]))
            .rational(0x0006, g.alt_dm, 10)
            .set(
                0x0007,
                Value::Rational(vec![(s / 3600, 1), ((s / 60) % 60, 1), (s % 60, 1)]),
            )
            .ascii(0x001D, exif_datetime(utc)[..10].to_string());
        gps
    });
    (ex, gps)
}

/// The EXIF TIFF stream (to wrap with [`crate::jpeg::app1_exif`]).
pub fn exif_tiff(spec: &ExifSpec<'_>) -> Vec<u8> {
    let m = spec.body.model;
    let c = spec.capture;
    let mut ifd0 = Ifd::new();
    ifd0.ascii(0x010F, m.make)
        .ascii(0x0110, m.model)
        .short(0x0112, spec.orientation)
        .rational(0x011A, 72, 1)
        .rational(0x011B, 72, 1)
        .short(0x0128, 2)
        .ascii(0x0131, spec.software.unwrap_or(m.firmware))
        .ascii(0x0132, c.exif())
        .short(0x0213, 2);
    if !spec.body.serial.is_empty() {
        ifd0.ascii(0x013B, format!("{OWNER_TITLE} {}", spec.body.owner));
    }
    let (ex, gps) = exif_ifds(spec);
    ifd0.child(0x8769, ex);
    if let Some(g) = gps {
        ifd0.child(0x8825, g);
    }
    if let Some(thumb) = &spec.thumbnail {
        let mut ifd1 = Ifd::new();
        ifd1.short(0x0103, 6)
            .rational(0x011A, 72, 1)
            .rational(0x011B, 72, 1)
            .short(0x0128, 2)
            .blob(0x0201, 0x0202, thumb.clone());
        ifd0.set_next(ifd1);
    }
    write_tiff(&ifd0, m.endian)
}

const OWNER_TITLE: &str = "Owner:";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_arithmetic() {
        let c = Capture {
            local: 1_404_498_605,
            millis: 900,
            offset_min: -420,
        };
        let d = c.plus_millis(300);
        assert_eq!(d.local, c.local + 1);
        assert_eq!(d.millis, 200);
        assert_eq!(c.offset_string(), "-07:00");
        assert_eq!(c.exif(), "2014:07:04 18:30:05");
    }

    #[test]
    fn models_span_years_and_have_subsec_variety() {
        let min = MODELS.iter().map(|m| m.years.0).min().unwrap_or(0);
        let max = MODELS.iter().map(|m| m.years.1).max().unwrap_or(0);
        assert!(max - min >= 5);
        assert!(MODELS.iter().any(|m| m.subsec_digits == 0));
        assert!(MODELS.iter().any(|m| m.subsec_digits > 0));
    }
}
