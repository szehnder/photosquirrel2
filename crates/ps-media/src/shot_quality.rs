//! How sharp and how well exposed a photo is, from its thumbnail (ADR-0135 decision 5, shot-quality version 1).
//!
//! Only shots of one moment, framed alike, are ever compared, so a small image is enough: the thumbnail the scan
//! already decodes (384 px long edge). Sharpness is the variance of the 3x3 Laplacian of the Rec. 709 luminance,
//! averaged per pixel; clipping is the fraction of pixels whose luminance is at least 250 (blown highlights) or at
//! most 5 (crushed shadows).

use ps_model::ShotQuality;

use crate::decode::RgbImage;

/// Luminance at or above this is a blown highlight.
pub const CLIPPED_HIGH: u8 = 250;
/// Luminance at or below this is a crushed shadow.
pub const CLIPPED_LOW: u8 = 5;

/// The shot quality of an RGB image; `None` for one smaller than 3x3 or with a short buffer.
pub fn shot_quality(img: &RgbImage) -> Option<ShotQuality> {
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 3 || h < 3 || img.data.len() < w * h * 3 {
        return None;
    }
    let luma: Vec<f32> = img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .take(w * h)
        .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
        .collect();
    let (mut high, mut low) = (0usize, 0usize);
    for &y in &luma {
        if y >= f32::from(CLIPPED_HIGH) {
            high += 1;
        } else if y <= f32::from(CLIPPED_LOW) {
            low += 1;
        }
    }
    // Laplacian over the interior: 4·centre − the four neighbours.
    let (mut sum, mut sum_sq, mut n) = (0f64, 0f64, 0f64);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let l = 4.0 * luma[i] - luma[i - 1] - luma[i + 1] - luma[i - w] - luma[i + w];
            let l = f64::from(l);
            sum += l;
            sum_sq += l * l;
            n += 1.0;
        }
    }
    let mean = sum / n;
    let total = (w * h) as f32;
    Some(ShotQuality {
        sharpness: (sum_sq / n - mean * mean).max(0.0) as f32,
        clipped_high: high as f32 / total,
        clipped_low: low as f32 / total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo-like test card: fine stripes and a soft gradient.
    fn card(w: u32, h: u32) -> RgbImage {
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let stripe = if (x / 2 + y / 3) % 2 == 0 { 60 } else { 0 };
                let v = (40 + (x * 120 / w) + stripe) as u8;
                data.extend_from_slice(&[v, v, v]);
            }
        }
        RgbImage {
            width: w,
            height: h,
            data,
        }
    }

    /// A 3x3 box blur, `n` times.
    fn blurred(img: &RgbImage, n: usize) -> RgbImage {
        let (w, h) = (img.width as usize, img.height as usize);
        let mut cur = img.data.clone();
        for _ in 0..n {
            let mut next = cur.clone();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    for c in 0..3 {
                        let mut s = 0u32;
                        for dy in [-1i32, 0, 1] {
                            for dx in [-1i32, 0, 1] {
                                let i = ((y as i32 + dy) as usize * w + (x as i32 + dx) as usize)
                                    * 3
                                    + c;
                                s += u32::from(cur[i]);
                            }
                        }
                        next[(y * w + x) * 3 + c] = (s / 9) as u8;
                    }
                }
            }
            cur = next;
        }
        RgbImage {
            width: img.width,
            height: img.height,
            data: cur,
        }
    }

    #[test]
    fn a_blurred_copy_scores_lower() {
        let sharp = shot_quality(&card(96, 64)).unwrap();
        let soft = shot_quality(&blurred(&card(96, 64), 2)).unwrap();
        assert!(
            sharp.sharpness > 2.0 * soft.sharpness,
            "{sharp:?} vs {soft:?}"
        );
        assert_eq!(sharp.clipped_high, 0.0);
    }

    #[test]
    fn blown_and_crushed_pixels_are_counted() {
        let mut img = card(10, 10);
        for p in img.data.as_chunks_mut::<3>().0.iter_mut().take(20) {
            p.copy_from_slice(&[255, 255, 255]);
        }
        for p in img.data.as_chunks_mut::<3>().0.iter_mut().skip(90) {
            p.copy_from_slice(&[0, 0, 0]);
        }
        let q = shot_quality(&img).unwrap();
        assert!((q.clipped_high - 0.20).abs() < 1e-6, "{q:?}");
        assert!((q.clipped_low - 0.10).abs() < 1e-6, "{q:?}");
    }

    #[test]
    fn tiny_or_short_images_have_no_score() {
        assert!(shot_quality(&card(2, 10)).is_none());
        let mut img = card(10, 10);
        img.data.truncate(10);
        assert!(shot_quality(&img).is_none());
    }
}
