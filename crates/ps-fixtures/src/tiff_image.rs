//! TIFF test images (tiff-support): one picture written in the layouts, sample formats, colour models,
//! compressions and directory placements real TIFF writers produce, so tests can compare every decode with the
//! same 8-bit RGB reference. Uncompressed and JPEG-compressed files use the fixture writer
//! ([`crate::tiff::write_tiff_placed`]), which can put the directories before or after the data; LZW and Deflate
//! files use the `tiff` crate's encoder, which (like libtiff and ImageMagick) writes every directory after its data.
//!
//! None of these is part of the ground-truth corpus.

use std::io::Cursor;

use crate::media::{encode_jpeg, photo_rgb};
use crate::tiff::{Endian, Ifd, Placement, Value, write_tiff_placed};

/// Strips of `rows` rows, or square tiles of `size` pixels (a multiple of 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Strips { rows: u32 },
    Tiles { size: u32 },
}

/// The colour model written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    /// BlackIsZero luminance (BT.601 weights).
    Gray,
    Rgb,
    /// RGB plus unassociated alpha.
    Rgba(Alpha),
    /// Naive CMYK (K = 255 - max(R, G, B)).
    Cmyk,
    /// YCbCr data in JPEG-compressed strips or tiles (photometric 6, compression 7), 8-bit only.
    YCbCrJpeg,
}

/// The alpha channel of [`Colour::Rgba`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Opaque,
    /// Alpha 0 in the left half of the image, 255 in the right half.
    LeftClear,
}

/// Sample type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    U8,
    U16,
    /// IEEE float, 0.0 black to 1.0 white.
    F32,
}

/// Compression of non-JPEG files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Lzw,
    Deflate,
}

/// Camera EXIF written into IFD0 and an Exif IFD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiffExif {
    pub make: String,
    pub model: String,
    /// `YYYY:MM:DD HH:MM:SS`.
    pub date_time_original: String,
}

/// One TIFF test image.
#[derive(Clone, Debug)]
pub struct TiffSpec {
    pub width: u32,
    pub height: u32,
    /// Seed of the picture ([`photo_rgb`]); further pages use `seed + 1`, `seed + 2`, ...
    pub seed: u64,
    /// The stored pixels (RGB8, `width x height`) instead of [`photo_rgb`].
    pub pixels: Option<Vec<u8>>,
    pub layout: Layout,
    pub colour: Colour,
    pub sample: Sample,
    /// PlanarConfiguration 2 (one plane per sample); uncompressed files only.
    pub planar: bool,
    pub compression: Compression,
    /// Horizontal differencing (LZW/Deflate integer samples).
    pub predictor: bool,
    pub placement: Placement,
    pub endian: Endian,
    /// Orientation tag 0x0112.
    pub orientation: Option<u16>,
    pub exif: Option<TiffExif>,
    /// Further pages (NewSubfileType 2), each a different picture.
    pub pages: Vec<(u32, u32)>,
    /// Reduced-resolution levels of the picture (NewSubfileType 1), after the first image in the chain.
    pub levels: Vec<(u32, u32)>,
}

impl TiffSpec {
    /// An 8-bit RGB, uncompressed, 16-row-strip, little-endian TIFF with its directories after the data.
    pub fn new(width: u32, height: u32, seed: u64) -> Self {
        Self {
            width,
            height,
            seed,
            pixels: None,
            layout: Layout::Strips { rows: 16 },
            colour: Colour::Rgb,
            sample: Sample::U8,
            planar: false,
            compression: Compression::None,
            predictor: false,
            placement: Placement::DataFirst,
            endian: Endian::Little,
            orientation: None,
            exif: None,
            pages: Vec::new(),
            levels: Vec::new(),
        }
    }

    /// The stored picture as RGB8 (`width x height`).
    pub fn reference(&self) -> Vec<u8> {
        self.pixels
            .clone()
            .unwrap_or_else(|| photo_rgb(self.width, self.height, self.seed))
    }
}

/// Box-downsample (or nearest-upsample) an RGB8 image to `w x h`.
pub fn resize_rgb(rgb: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        let (y0, y1) = (y * sh / h, ((y + 1) * sh / h).max(y * sh / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * sw / w, ((x + 1) * sw / w).max(x * sw / w + 1));
            let mut acc = [0u32; 3];
            for yy in y0..y1.min(sh) {
                for xx in x0..x1.min(sw) {
                    let i = (yy as usize * sw as usize + xx as usize) * 3;
                    for c in 0..3 {
                        acc[c] += u32::from(rgb[i + c]);
                    }
                }
            }
            let n = (y1.min(sh) - y0) * (x1.min(sw) - x0);
            out.extend(acc.map(|v| ((v + n / 2) / n.max(1)) as u8));
        }
    }
    out
}

/// BT.601 luminance, as the fixtures write greyscale.
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) + 500) / 1000) as u8
}

/// Write the TIFF of `spec`.
pub fn tiff_image(spec: &TiffSpec) -> Vec<u8> {
    let mut images = vec![(spec.width, spec.height, spec.reference(), 0u32)];
    for &(w, h) in &spec.levels {
        images.push((
            w,
            h,
            resize_rgb(&images[0].2, spec.width, spec.height, w, h),
            1,
        ));
    }
    for (i, &(w, h)) in spec.pages.iter().enumerate() {
        images.push((w, h, photo_rgb(w, h, spec.seed + 1 + i as u64), 2));
    }
    match spec.compression {
        Compression::None => own_writer(spec, &images),
        Compression::Lzw | Compression::Deflate => tiff_encoder(spec, &images),
    }
}

/// Samples per pixel and photometric interpretation of a colour model.
fn model(colour: Colour) -> (usize, u16) {
    match colour {
        Colour::Gray => (1, 1),
        Colour::Rgb => (3, 2),
        Colour::Rgba(_) => (4, 2),
        Colour::Cmyk => (4, 5),
        Colour::YCbCrJpeg => (3, 6),
    }
}

/// Append the pixel's samples (8-bit) in the colour model to `out`.
fn push_samples(out: &mut Vec<u8>, colour: Colour, rgb: [u8; 3], x: u32, w: u32) {
    let [r, g, b] = rgb;
    match colour {
        Colour::Gray => out.push(luma(r, g, b)),
        Colour::Rgb | Colour::YCbCrJpeg => out.extend_from_slice(&rgb),
        Colour::Rgba(a) => {
            let alpha = match a {
                Alpha::Opaque => 255,
                Alpha::LeftClear if x < w / 2 => 0,
                Alpha::LeftClear => 255,
            };
            out.extend_from_slice(&[r, g, b, alpha]);
        }
        Colour::Cmyk => {
            let k = 255 - r.max(g).max(b);
            let ink = |v: u8| {
                if k == 255 {
                    0
                } else {
                    ((255 - u32::from(v) - u32::from(k)) * 255 / (255 - u32::from(k))) as u8
                }
            };
            out.extend_from_slice(&[ink(r), ink(g), ink(b), k]);
        }
    }
}

/// Encode one 8-bit sample value in the sample type and byte order.
fn put_sample(out: &mut Vec<u8>, v: u8, sample: Sample, e: Endian) {
    match (sample, e) {
        (Sample::U8, _) => out.push(v),
        (Sample::U16, Endian::Little) => out.extend_from_slice(&(u16::from(v) * 257).to_le_bytes()),
        (Sample::U16, Endian::Big) => out.extend_from_slice(&(u16::from(v) * 257).to_be_bytes()),
        (Sample::F32, Endian::Little) => {
            out.extend_from_slice(&(f32::from(v) / 255.0).to_le_bytes())
        }
        (Sample::F32, Endian::Big) => out.extend_from_slice(&(f32::from(v) / 255.0).to_be_bytes()),
    }
}

fn bits(sample: Sample) -> u16 {
    match sample {
        Sample::U8 => 8,
        Sample::U16 => 16,
        Sample::F32 => 32,
    }
}

/// The image's chunks (strips or tiles; planar: plane by plane), uncompressed or JPEG.
fn chunks(spec: &TiffSpec, w: u32, h: u32, rgb: &[u8]) -> Vec<Vec<u8>> {
    let (spp, _) = model(spec.colour);
    let px = |x: u32, y: u32| {
        // Tiles past the image edge repeat the edge pixel, as encoders pad.
        let (x, y) = (x.min(w - 1), y.min(h - 1));
        let i = (y as usize * w as usize + x as usize) * 3;
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    };
    let rects: Vec<(u32, u32, u32, u32)> = match spec.layout {
        Layout::Strips { rows } => (0..h.div_ceil(rows))
            .map(|i| (0, i * rows, w, rows.min(h - i * rows)))
            .collect(),
        Layout::Tiles { size } => (0..h.div_ceil(size))
            .flat_map(|ty| (0..w.div_ceil(size)).map(move |tx| (tx * size, ty * size, size, size)))
            .collect(),
    };
    if spec.colour == Colour::YCbCrJpeg {
        return rects
            .iter()
            .map(|&(x0, y0, cw, ch)| {
                let mut tile = Vec::with_capacity(cw as usize * ch as usize * 3);
                for y in y0..y0 + ch {
                    for x in x0..x0 + cw {
                        tile.extend_from_slice(&px(x, y));
                    }
                }
                encode_jpeg(&tile, cw, ch, 92)
            })
            .collect();
    }
    let planes: Vec<Option<usize>> = if spec.planar {
        (0..spp).map(Some).collect()
    } else {
        vec![None]
    };
    let mut out = Vec::new();
    for plane in planes {
        for &(x0, y0, cw, ch) in &rects {
            let mut c = Vec::new();
            let mut s = Vec::with_capacity(spp);
            for y in y0..y0 + ch {
                for x in x0..x0 + cw {
                    s.clear();
                    push_samples(&mut s, spec.colour, px(x, y), x, w);
                    match plane {
                        Some(p) => put_sample(&mut c, s[p], spec.sample, spec.endian),
                        None => s
                            .iter()
                            .for_each(|v| put_sample(&mut c, *v, spec.sample, spec.endian)),
                    }
                }
            }
            out.push(c);
        }
    }
    out
}

fn own_writer(spec: &TiffSpec, images: &[(u32, u32, Vec<u8>, u32)]) -> Vec<u8> {
    let (spp, photometric) = model(spec.colour);
    let mut ifds: Vec<Ifd> = images
        .iter()
        .enumerate()
        .map(|(i, (w, h, rgb, subfile))| {
            let mut ifd = Ifd::new();
            if *subfile != 0 {
                ifd.long(0x00FE, *subfile);
            }
            ifd.long(0x0100, *w).long(0x0101, *h);
            let sample = if spec.colour == Colour::YCbCrJpeg {
                Sample::U8
            } else {
                spec.sample
            };
            ifd.set(0x0102, Value::Short(vec![bits(sample); spp]));
            ifd.short(
                0x0103,
                if spec.colour == Colour::YCbCrJpeg {
                    7
                } else {
                    1
                },
            );
            ifd.short(0x0106, photometric);
            ifd.short(0x0115, spp as u16);
            ifd.short(0x011C, if spec.planar { 2 } else { 1 });
            if let Colour::Rgba(_) = spec.colour {
                ifd.short(0x0152, 2);
            }
            if sample == Sample::F32 {
                ifd.set(0x0153, Value::Short(vec![3; spp]));
            }
            if spec.colour == Colour::YCbCrJpeg {
                ifd.set(0x0212, Value::Short(vec![2, 2]));
            }
            if let Some(o) = spec.orientation {
                ifd.short(0x0112, o);
            }
            let data = chunks(spec, *w, *h, rgb);
            match spec.layout {
                Layout::Strips { rows } => {
                    ifd.long(0x0116, rows);
                    ifd.chunks(0x0111, 0x0117, data);
                }
                Layout::Tiles { size } => {
                    ifd.long(0x0142, size).long(0x0143, size);
                    ifd.chunks(0x0144, 0x0145, data);
                }
            }
            if i == 0
                && let Some(x) = &spec.exif
            {
                ifd.ascii(0x010F, x.make.clone())
                    .ascii(0x0110, x.model.clone());
                let mut exif = Ifd::new();
                exif.ascii(0x9003, x.date_time_original.clone());
                ifd.child(0x8769, exif);
            }
            ifd
        })
        .collect();
    let mut chain = ifds.pop().expect("at least one image");
    while let Some(mut prev) = ifds.pop() {
        prev.set_next(chain);
        chain = prev;
    }
    write_tiff_placed(&chain, spec.endian, spec.placement)
}

/// LZW or Deflate through the `tiff` crate's encoder (strips, chunky, directories after the data).
fn tiff_encoder(spec: &TiffSpec, images: &[(u32, u32, Vec<u8>, u32)]) -> Vec<u8> {
    use tiff::encoder::{TiffEncoder, colortype};
    use tiff::tags::Tag;
    assert!(
        !spec.planar && matches!(spec.layout, Layout::Strips { .. }) && spec.exif.is_none(),
        "the tiff encoder writes chunky strips without EXIF"
    );
    assert!(
        spec.endian == Endian::Little,
        "the tiff encoder writes little-endian"
    );
    let rows = match spec.layout {
        Layout::Strips { rows } => rows,
        Layout::Tiles { .. } => unreachable!(),
    };
    let compression = match spec.compression {
        Compression::Lzw => tiff::encoder::Compression::Lzw,
        _ => tiff::encoder::Compression::Deflate(tiff::encoder::DeflateLevel::Balanced),
    };
    let predictor = if spec.predictor && spec.sample != Sample::F32 {
        tiff::tags::Predictor::Horizontal
    } else {
        tiff::tags::Predictor::None
    };
    let mut out = Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut out)
            .expect("encoder")
            .with_compression(compression)
            .with_predictor(predictor);
        for (w, h, rgb, subfile) in images {
            let (w, h) = (*w, *h);
            let mut samples: Vec<u8> = Vec::with_capacity(w as usize * h as usize * 4);
            for y in 0..h {
                for x in 0..w {
                    let i = (y as usize * w as usize + x as usize) * 3;
                    push_samples(
                        &mut samples,
                        spec.colour,
                        [rgb[i], rgb[i + 1], rgb[i + 2]],
                        x,
                        w,
                    );
                }
            }
            macro_rules! write {
                ($ct:ty, $data:expr) => {{
                    let mut img = enc.new_image::<$ct>(w, h).expect("image");
                    img.rows_per_strip(rows).expect("rows per strip");
                    if *subfile != 0 {
                        img.encoder()
                            .write_tag(Tag::NewSubfileType, *subfile)
                            .expect("subfile type");
                    }
                    if let Some(o) = spec.orientation {
                        img.encoder()
                            .write_tag(Tag::Orientation, o)
                            .expect("orientation");
                    }
                    img.write_data(&$data).expect("data");
                }};
            }
            let u16s = || {
                samples
                    .iter()
                    .map(|v| u16::from(*v) * 257)
                    .collect::<Vec<u16>>()
            };
            let f32s = || {
                samples
                    .iter()
                    .map(|v| f32::from(*v) / 255.0)
                    .collect::<Vec<f32>>()
            };
            match (spec.colour, spec.sample) {
                (Colour::Gray, Sample::U8) => write!(colortype::Gray8, samples),
                (Colour::Gray, Sample::U16) => write!(colortype::Gray16, u16s()),
                (Colour::Gray, Sample::F32) => write!(colortype::Gray32Float, f32s()),
                (Colour::Rgb, Sample::U8) => write!(colortype::RGB8, samples),
                (Colour::Rgb, Sample::U16) => write!(colortype::RGB16, u16s()),
                (Colour::Rgb, Sample::F32) => write!(colortype::RGB32Float, f32s()),
                (Colour::Rgba(_), Sample::U8) => write!(colortype::RGBA8, samples),
                (Colour::Rgba(_), Sample::U16) => write!(colortype::RGBA16, u16s()),
                (Colour::Rgba(_), Sample::F32) => write!(colortype::RGBA32Float, f32s()),
                (Colour::Cmyk, Sample::U8) => write!(colortype::CMYK8, samples),
                (Colour::Cmyk, Sample::U16) => write!(colortype::CMYK16, u16s()),
                (Colour::Cmyk, Sample::F32) => write!(colortype::CMYK32Float, f32s()),
                (Colour::YCbCrJpeg, _) => panic!("JPEG TIFFs use the fixture writer"),
            }
        }
    }
    out.into_inner()
}

/// A little-endian BigTIFF (version 43, 8-byte offsets) holding one uncompressed 8-bit RGB strip of
/// [`photo_rgb`]`(width, height, seed)`, its directory right after the header (`preview-placeholders`: a format
/// variant PhotoSquirrel recognises and does not decode).
pub fn bigtiff_rgb(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let pixels = photo_rgb(width, height, seed);
    // (tag, type, count, value): SHORT = 3, LONG8 = 16; values inline (at most 8 bytes).
    let entries: [(u16, u16, u64, u64); 10] = [
        (256, 16, 1, u64::from(width)),
        (257, 16, 1, u64::from(height)),
        // BitsPerSample: three SHORTs of 8, inline.
        (258, 3, 3, 8 | (8 << 16) | (8 << 32)),
        (259, 3, 1, 1),
        (262, 3, 1, 2),
        (273, 16, 1, 0), // StripOffsets, patched below
        (277, 3, 1, 3),
        (278, 16, 1, u64::from(height)),
        (279, 16, 1, pixels.len() as u64),
        (284, 3, 1, 1),
    ];
    let ifd_at = 16u64;
    let ifd_len = 8 + entries.len() as u64 * 20 + 8;
    let data_at = ifd_at + ifd_len;
    let mut out = Vec::with_capacity(data_at as usize + pixels.len());
    out.extend_from_slice(b"II");
    out.extend_from_slice(&43u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&ifd_at.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (tag, ty, count, value) in entries {
        let value = if tag == 273 { data_at } else { value };
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&pixels);
    out
}
