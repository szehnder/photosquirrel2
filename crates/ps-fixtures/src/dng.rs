//! Minimal DNG: IFD0 is a full-size JPEG preview (NewSubFileType 1) with camera tags, an Exif IFD and GPS,
//! plus one SubIFD holding a tiny 16-bit CFA "sensor strip". Enough for format detection, metadata, embedded
//! preview extraction and RAW+JPEG pairing; not a decodable full-resolution raw.

use crate::camera::{ExifSpec, exif_ifds};
use crate::rng::mix64;
use crate::tiff::{Endian, Ifd, Value, write_tiff};

/// Build a DNG for a shot whose camera JPEG (without APP1) is `preview_jpeg` at `width x height`.
pub fn dng(spec: &ExifSpec<'_>, preview_jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    raw_tiff(spec, preview_jpeg, width, height, &Flavour::Dng)
}

/// A camera RAW in the CR2 manner for the converted-burst sets (ADR-0105): the same layout as [`dng`] without the
/// DNG tags, so the probe takes it for a Canon CR2 by its make; camera firmware as software, the maker note in the
/// Exif IFD. Stored dimensions are the Exif pixel dimensions (`width x height`).
pub fn cr2_like(spec: &ExifSpec<'_>, preview_jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    raw_tiff(spec, preview_jpeg, width, height, &Flavour::Cr2Like)
}

/// A DNG as Adobe's converter writes it from a camera RAW (ADR-0105): `OriginalRawFileName` (0xC68B), the
/// camera's maker note moved into `DNGPrivateData` (0xC634, `Adobe\0MakN`, rebased on `original_offset`), none in
/// the Exif IFD (pass `spec.maker_note: None`), and a raw SubIFD of `sensor` dimensions (larger than the camera's
/// JPEG, as 5568x3708 is for a 5472x3648 CR2). The strip itself stays tiny: it is never decoded.
pub fn converted_dng(
    spec: &ExifSpec<'_>,
    preview_jpeg: &[u8],
    width: u32,
    height: u32,
    original_name: &str,
    maker_note: &[u8],
    sensor: (u32, u32),
) -> Vec<u8> {
    raw_tiff(
        spec,
        preview_jpeg,
        width,
        height,
        &Flavour::Converted {
            original_name,
            maker_note,
            sensor,
        },
    )
}

/// `DNGPrivateData` holding a maker note the way Adobe writes it: `Adobe\0`, `MakN`, a big-endian length, the
/// note's byte order, its offset in the original file (big-endian), then the note.
pub fn dng_private_maker_note(note: &[u8], little_endian: bool, original_offset: u32) -> Vec<u8> {
    let mut v = b"Adobe\0MakN".to_vec();
    v.extend_from_slice(&((note.len() + 6) as u32).to_be_bytes());
    v.extend_from_slice(if little_endian { b"II" } else { b"MM" });
    v.extend_from_slice(&original_offset.to_be_bytes());
    v.extend_from_slice(note);
    v
}

enum Flavour<'a> {
    Dng,
    Cr2Like,
    Converted {
        original_name: &'a str,
        maker_note: &'a [u8],
        sensor: (u32, u32),
    },
}

fn raw_tiff(
    spec: &ExifSpec<'_>,
    preview_jpeg: &[u8],
    width: u32,
    height: u32,
    flavour: &Flavour<'_>,
) -> Vec<u8> {
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
    let (sw, sh) = match flavour {
        Flavour::Converted { sensor, .. } => *sensor,
        _ => (rw, rh),
    };
    raw.long(0x00FE, 0)
        .long(0x0100, sw)
        .long(0x0101, sh)
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
        .ascii(
            0x0131,
            match flavour {
                Flavour::Cr2Like => m.firmware,
                _ => "Adobe DNG Converter 12.0",
            },
        )
        .ascii(0x0132, spec.capture.exif());
    if !matches!(flavour, Flavour::Cr2Like) {
        ifd0.set(0xC612, Value::Byte(vec![1, 4, 0, 0]))
            .set(0xC613, Value::Byte(vec![1, 1, 0, 0]))
            .ascii(0xC614, m.model);
    }
    if let Flavour::Converted {
        original_name,
        maker_note,
        ..
    } = flavour
    {
        ifd0.set(
            0xC634,
            Value::Byte(dng_private_maker_note(
                maker_note,
                m.endian == Endian::Little,
                0x0360,
            )),
        )
        .ascii(0xC68B, *original_name);
    }
    ifd0.blob(0x0111, 0x0117, preview_jpeg.to_vec())
        .child(0x014A, raw);

    let (ex, gps) = exif_ifds(spec);
    ifd0.child(0x8769, ex);
    if let Some(g) = gps {
        ifd0.child(0x8825, g);
    }
    write_tiff(&ifd0, m.endian)
}

/// The three files of one frame of a converted Canon burst (ADR-0105), for tests outside the corpus.
#[derive(Clone, Debug)]
pub struct BurstFrameFiles {
    /// `IMG_<n>.CR2`: the camera RAW (CR2-like), maker note in the Exif IFD.
    pub cr2: Vec<u8>,
    /// `IMG_<n>.JPG`: the camera JPEG.
    pub jpeg: Vec<u8>,
    /// `IMG_<n>.DNG`: converted, `OriginalRawFileName` `IMG_<n>.CR2`, the maker note in `DNGPrivateData`, no
    /// sub-second when the camera's was `00`.
    pub dng: Vec<u8>,
    /// The camera's file number, `101 * 10000 + n` (`101-0034`).
    pub file_number: u32,
    /// `IMG_<n>`.
    pub stem: String,
}

/// Frame `frame` (0-based) of a burst of a Canon EOS 5D Mark III shot at 2019-07-04 12:00:05 and `millis` ms
/// (two-digit sub-seconds, `00` below 10 ms), file `IMG_<34 + frame>`; `seed` picks the scene (equal seeds, one
/// burst) and `long` the long edge of the 3:2 picture. Frames differ like burst frames (pHash distance about 2).
pub fn converted_burst_frame(seed: u64, frame: u32, millis: u16, long: u32) -> BurstFrameFiles {
    converted_burst_frame_with(seed, frame, millis, long, frame)
}

/// [`converted_burst_frame`] whose converted DNG shows frame `rendered_as` of the burst instead of its own (the
/// converter's rendering of frame N closer to frame N+1 than to frame N's camera JPEG, which is what made frame N's
/// DNG join frame N+1 on the owner's card).
pub fn converted_burst_frame_with(
    seed: u64,
    frame: u32,
    millis: u16,
    long: u32,
    rendered_as: u32,
) -> BurstFrameFiles {
    use crate::camera::{Body, Capture, Exposure, MODELS, spec_maker_note};
    use crate::jpeg::{app1_exif, camera_jpeg, encode_jpeg, strip_segments};
    use crate::synth::{Perturb, Scene, render};
    let model = &MODELS[0];
    let body = Body::new(model, &mut crate::rng::derive(seed, "burst-body", 0));
    let scene = Scene::random(&mut crate::rng::derive(seed, "burst-scene", 0));
    let mut rng = crate::rng::derive(seed, "burst-frame", u64::from(frame));
    let perturb = Perturb::near(&mut rng, 1.0);
    let (w, h) = (long & !1, (long * 2 / 3) & !1);
    let capture = Capture {
        local: ps_model::CaptureTime::from_parts(2019, 7, 4, 12, 0, 5).local_seconds,
        millis,
        offset_min: 0,
    };
    let exposure = Exposure::random(model, &mut rng);
    let n = 34 + frame;
    let file_number = 101 * 10_000 + n;
    let spec = ExifSpec {
        body: &body,
        capture: &capture,
        exposure: &exposure,
        gps: None,
        orientation: 1,
        width: w,
        height: h,
        maker_note: Some(u64::from(frame) + seed),
        software: None,
        thumbnail: None,
        subsec: true,
        file_number: Some(file_number),
    };
    let img = render(&scene, &perturb, w, h);
    let encoded = encode_jpeg(&img, 92);
    let jpeg = camera_jpeg(&encoded, &app1_exif(&crate::camera::exif_tiff(&spec)));
    let preview = strip_segments(&encoded, &[0xE0]);
    let cr2 = cr2_like(&spec, &preview, w, h);
    let mut render_p = if rendered_as == frame {
        perturb
    } else {
        Perturb::near(
            &mut crate::rng::derive(seed, "burst-frame", u64::from(rendered_as)),
            1.0,
        )
    };
    render_p.light *= 1.02;
    render_p.grain_seed ^= 0x5eed;
    let dng_preview = strip_segments(&encode_jpeg(&render(&scene, &render_p, w, h), 90), &[0xE0]);
    let note = spec_maker_note(&spec).unwrap_or_default();
    let dng_spec = ExifSpec {
        maker_note: None,
        subsec: millis / 10 != 0,
        ..spec.clone()
    };
    let stem = format!("IMG_{n:04}");
    let dng = converted_dng(
        &dng_spec,
        &dng_preview,
        w,
        h,
        &format!("{stem}.CR2"),
        &note,
        ((w + w / 64) & !1, (h + h / 64) & !1),
    );
    BurstFrameFiles {
        cr2,
        jpeg,
        dng,
        file_number,
        stem,
    }
}
