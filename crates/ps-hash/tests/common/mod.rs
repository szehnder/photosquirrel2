//! Shared test helpers: synthetic photos, JPEG round trips and instrumented readers.
#![allow(dead_code, clippy::unwrap_used)]

use std::io::{self, Read, Seek, SeekFrom};

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, RgbImage};
use ps_hash::{ImageView, PixelLayout};
use ps_model::{Orientation, PerceptualHash};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// A deterministic procedural "photo" (RGB8): a sky-to-ground gradient, soft blobs of colour, oriented
/// texture and a little sensor noise. Different seeds give unrelated layouts.
pub fn photo(seed: u64, w: u32, h: u32) -> RgbImage {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let c = |rng: &mut ChaCha8Rng| [rng.random::<f32>(), rng.random(), rng.random()];
    let (top, bottom) = (c(&mut rng), c(&mut rng));
    let angle: f32 = rng.random_range(0.0..std::f32::consts::TAU);
    let (gx, gy) = (angle.cos(), angle.sin());
    let blobs: Vec<([f32; 3], f32, f32, f32, f32, f32)> = (0..rng.random_range(5..10))
        .map(|_| {
            (
                c(&mut rng),
                rng.random_range(-0.1..1.1),
                rng.random_range(-0.1..1.1),
                rng.random_range(0.05..0.35),
                rng.random_range(0.05..0.35),
                rng.random_range(0.5..1.0),
            )
        })
        .collect();
    let (freq, phase, tex_angle): (f32, f32, f32) = (
        rng.random_range(4.0..20.0),
        rng.random_range(0.0..std::f32::consts::TAU),
        rng.random_range(0.0..std::f32::consts::PI),
    );
    let (tx, ty) = (tex_angle.cos(), tex_angle.sin());
    let mut img = RgbImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let u = x as f32 / w as f32;
        let v = y as f32 / h as f32;
        let t = ((u - 0.5) * gx + (v - 0.5) * gy + 0.5).clamp(0.0, 1.0);
        let mut px = [0f32; 3];
        for k in 0..3 {
            px[k] = top[k] * (1.0 - t) + bottom[k] * t;
        }
        for (col, cx, cy, rx, ry, a) in &blobs {
            let d = ((u - cx) / rx).powi(2) + ((v - cy) / ry).powi(2);
            let wgt = a * (-d).exp();
            for k in 0..3 {
                px[k] = px[k] * (1.0 - wgt) + col[k] * wgt;
            }
        }
        let tex = 0.06 * ((u * tx + v * ty) * freq * std::f32::consts::TAU + phase).sin();
        let noise = (rng.random::<f32>() - 0.5) * 0.04;
        *p = image::Rgb(px.map(|c| ((c + tex + noise).clamp(0.0, 1.0) * 255.0).round() as u8));
    }
    img
}

pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    out
}

pub fn decode_jpeg(bytes: &[u8]) -> RgbImage {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .unwrap()
        .to_rgb8()
}

pub fn hash_rgb(img: &RgbImage, orientation: u8) -> PerceptualHash {
    let view = ImageView::new(img.width(), img.height(), PixelLayout::Rgb8, img.as_raw())
        .unwrap()
        .with_orientation(Orientation(orientation));
    ps_hash::perceptual_hash(&view)
}

/// Counts bytes read and the largest single read request.
pub struct CountingReader<R> {
    pub inner: R,
    pub bytes_read: u64,
    pub max_request: usize,
}

impl<R> CountingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            bytes_read: 0,
            max_request: 0,
        }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.max_request = self.max_request.max(buf.len());
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}
