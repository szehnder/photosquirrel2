//! Non-photo files: icons (ICO with PNG payloads), palette UI graphics with transparency (PNG, GIF) and tiny
//! web JPEGs. Pixel content is simple flat-shaded vector-like art, as real UI assets are.

use image::codecs::gif::GifEncoder;
use image::{Frame, RgbaImage};

use crate::jpeg::{PngKind, encode_jpeg, encode_png};
use crate::rng::{CorpusRng, Rng};
use crate::synth::{Perturb, Rgb, Scene, render};

/// A flat-colour glyph (rounded square / circle / bar chart) as palette indices; 0 is transparent.
fn glyph(rng: &mut CorpusRng, w: u32, h: u32, colours: u8) -> Vec<u8> {
    let kind = rng.random_range(0..3);
    let mut idx = vec![0u8; (w * h) as usize];
    let bars: Vec<u32> = (0..4)
        .map(|_| rng.random_range(h / 5..h.max(6) - 1))
        .collect();
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = ((2 * x + 1) as i64 - w as i64, (2 * y + 1) as i64 - h as i64);
            let (rw, rh) = (w as i64, h as i64);
            let v = match kind {
                0 => {
                    // Circle with a ring.
                    let d = fx * fx * rh * rh + fy * fy * rw * rw;
                    let r2 = rw * rw * rh * rh;
                    if d < r2 * 81 / 100 {
                        if d > r2 * 49 / 100 { 1 } else { 2 }
                    } else {
                        0
                    }
                }
                1 => {
                    // Rounded square with a stripe.
                    let inside = fx.abs() < rw * 85 / 100 && fy.abs() < rh * 85 / 100;
                    if inside {
                        if fy.abs() < rh / 6 { 2 } else { 1 }
                    } else {
                        0
                    }
                }
                _ => {
                    // Bar chart.
                    let bar = (x * 4 / w.max(1)) as usize;
                    if x % (w / 4).max(1) < (w / 6).max(1) && h - y < bars[bar.min(3)] {
                        1 + (bar % usize::from(colours.max(2) - 1)) as u8
                    } else {
                        0
                    }
                }
            };
            idx[(y * w + x) as usize] = v;
        }
    }
    idx
}

fn palette(rng: &mut CorpusRng, n: usize) -> Vec<u8> {
    let mut p = vec![255, 255, 255];
    for _ in 1..n {
        p.extend_from_slice(&[rng.random(), rng.random(), rng.random()]);
    }
    p
}

/// A palette PNG with a transparent background.
pub fn ui_png(rng: &mut CorpusRng, w: u32, h: u32) -> Vec<u8> {
    let idx = glyph(rng, w, h, 3);
    let pal = palette(rng, 3);
    encode_png(PngKind::Indexed {
        width: w,
        height: h,
        indices: &idx,
        palette: &pal,
        trns: Some(&[0]),
    })
}

/// A small animated-looking (single frame) GIF.
pub fn ui_gif(rng: &mut CorpusRng, w: u32, h: u32) -> Vec<u8> {
    let idx = glyph(rng, w, h, 3);
    let pal = palette(rng, 3);
    let mut rgba = RgbaImage::new(w, h);
    for (i, p) in rgba.pixels_mut().enumerate() {
        let k = usize::from(idx[i]);
        let a = if k == 0 { 0 } else { 255 };
        *p = image::Rgba([pal[k * 3], pal[k * 3 + 1], pal[k * 3 + 2], a]);
    }
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        enc.encode_frame(Frame::new(rgba)).expect("gif frame");
    }
    out
}

/// An ICO file holding PNG images at the given square sizes.
pub fn ico(rng: &mut CorpusRng, sizes: &[u32]) -> Vec<u8> {
    let pal = palette(rng, 3);
    let seed: u64 = rng.random();
    let images: Vec<(u32, Vec<u8>)> = sizes
        .iter()
        .map(|&s| {
            let mut r = crate::rng::derive(seed, "ico", u64::from(s));
            let idx = glyph(&mut r, s, s, 3);
            let mut rgba = Vec::with_capacity((s * s * 4) as usize);
            for &k in &idx {
                let k = usize::from(k);
                rgba.extend_from_slice(&[
                    pal[k * 3],
                    pal[k * 3 + 1],
                    pal[k * 3 + 2],
                    if k == 0 { 0 } else { 255 },
                ]);
            }
            (
                s,
                encode_png(PngKind::Rgba {
                    width: s,
                    height: s,
                    data: &rgba,
                }),
            )
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, png) in &images {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend_from_slice(&png);
    }
    out
}

/// A tiny web JPEG (banner, avatar, product thumbnail) without EXIF.
pub fn tiny_web(rng: &mut CorpusRng, w: u32, h: u32) -> (Rgb, Vec<u8>) {
    let scene = Scene::random(rng);
    let img = render(&scene, &Perturb::identity(rng.random()), w, h);
    let q = rng.random_range(60..85);
    let bytes = encode_jpeg(&img, q);
    (img, bytes)
}
