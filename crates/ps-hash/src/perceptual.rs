//! Perceptual hashes, algorithm version 1 (ADR-0012).
//!
//! 1. **Luminance 32x32.** The stored pixels are area-resampled (box filter with exact fractional coverage) to
//!    32x32 using integer arithmetic, so the result is bit-identical on every platform. Luminance is BT.601
//!    (`0.299 R + 0.587 G + 0.114 B`), applied to the channel sums (it is linear). The EXIF orientation is then
//!    applied to the 32x32 grid, which equals resampling the physically rotated image exactly.
//! 2. **pHash.** Orthonormal 2-D DCT-II of the 32x32 luminance (separable, `f32`, precomputed cosine table, fixed
//!    loop order); the top-left 8x8 coefficients *including DC* are compared with their median (the mean of the
//!    32nd and 33rd smallest). Bit `u * 8 + v` is set when coefficient `(u, v)` (`u` vertical frequency) is
//!    greater than the median.
//! 3. **dHash.** The 32x32 luminance is area-resampled to 9 columns x 8 rows; bit `r * 8 + c` is set when cell
//!    `(r, c)` is darker than its right neighbour `(r, c + 1)`.
//!
//! Changing any step changes hash values: bump [`PERCEPTUAL_HASH_VERSION`] and the golden values together.

use std::fmt;
use std::sync::OnceLock;

use ps_model::PerceptualHash;

use crate::image::{ImageView, PixelLayout};

/// Current perceptual hash algorithm version.
pub const PERCEPTUAL_HASH_VERSION: u8 = 1;

/// Side of the luminance image.
pub const LUMA_SIDE: usize = 32;

/// The oriented 32x32 luminance image (row-major, values 0-255).
pub type Luma32 = [f32; LUMA_SIDE * LUMA_SIDE];

/// Hamming distance of two 64-bit hashes (hardware `popcnt` on the x86-64-v2 baseline, ADR-0032).
#[inline]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Both Hamming distances between two hashes of the same version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HashDistance {
    pub phash: u32,
    pub dhash: u32,
}

/// Two hashes were computed by different algorithm versions and cannot be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VersionMismatch {
    pub left: u8,
    pub right: u8,
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "perceptual hash versions {} and {} are incompatible",
            self.left, self.right
        )
    }
}

impl std::error::Error for VersionMismatch {}

/// Distances between two hashes, or [`VersionMismatch`] when their algorithm versions differ.
pub fn compare(a: &PerceptualHash, b: &PerceptualHash) -> Result<HashDistance, VersionMismatch> {
    if a.version != b.version {
        return Err(VersionMismatch {
            left: a.version,
            right: b.version,
        });
    }
    Ok(HashDistance {
        phash: hamming(a.phash, b.phash),
        dhash: hamming(a.dhash, b.dhash),
    })
}

/// pHash and dHash of an image, version [`PERCEPTUAL_HASH_VERSION`].
pub fn perceptual_hash(img: &ImageView<'_>) -> PerceptualHash {
    let luma = luminance32(img);
    PerceptualHash {
        phash: phash_from_luma(&luma),
        dhash: dhash_from_luma(&luma),
        version: PERCEPTUAL_HASH_VERSION,
    }
}

/// Integer area-resampling weights along one axis. Source pixel `x` covers `[x * dst, (x + 1) * dst)` and
/// destination bin `j` covers `[j * src, (j + 1) * src)`, so every overlap is an integer, each bin's weights
/// sum to `src` and each source pixel's weights sum to `dst`. Bin `j`'s `(source index, weight)` pairs are
/// `pairs[starts[j]..starts[j + 1]]`.
struct AxisWeights {
    starts: Vec<usize>,
    pairs: Vec<(u32, u32)>,
}

impl AxisWeights {
    fn new(src: usize, dst: usize) -> Self {
        let mut starts = Vec::with_capacity(dst + 1);
        let mut pairs = Vec::with_capacity(src + dst);
        for j in 0..dst {
            starts.push(pairs.len());
            let (b0, b1) = (j * src, (j + 1) * src);
            for x in b0 / dst..b1.div_ceil(dst).min(src) {
                let (p0, p1) = (x * dst, (x + 1) * dst);
                let w = p1.min(b1).saturating_sub(p0.max(b0));
                if w > 0 {
                    pairs.push((x as u32, w as u32));
                }
            }
        }
        starts.push(pairs.len());
        Self { starts, pairs }
    }

    #[inline]
    fn bin(&self, j: usize) -> &[(u32, u32)] {
        &self.pairs[self.starts[j]..self.starts[j + 1]]
    }

    /// Bin `j` as an optional run `x0..x1` of full-weight pixels (weight `dst`) and up to two partially
    /// covered pixels. A bin's pixels are consecutive and only its first and last can be partial.
    #[inline]
    fn split_bin(&self, j: usize) -> (Option<(usize, usize)>, [(u32, u32); 2]) {
        let bin = self.bin(j);
        let dst = (self.starts.len() - 1) as u32;
        let mut partial = [(0, 0); 2];
        let mut lo = 0;
        let mut hi = bin.len();
        if let Some(&first) = bin.first()
            && first.1 != dst
        {
            partial[0] = first;
            lo = 1;
        }
        if hi > lo && bin[hi - 1].1 != dst {
            partial[1] = bin[hi - 1];
            hi -= 1;
        }
        let middle = &bin[lo..hi];
        debug_assert!(middle.iter().all(|p| p.1 == dst));
        let run = middle
            .first()
            .map(|f| (f.0 as usize, f.0 as usize + middle.len()));
        (run, partial)
    }
}

// Row kernels. Kept out of line so LLVM sees non-aliasing slice arguments and vectorizes them (inlined into
// the resampler they stayed scalar).
#[inline(never)]
fn add_row(acc: &mut [u16], row: &[u8]) {
    for (a, &p) in acc.iter_mut().zip(row) {
        *a += u16::from(p);
    }
}

#[inline(never)]
fn add_rows4(acc: &mut [u16], r0: &[u8], r1: &[u8], r2: &[u8], r3: &[u8]) {
    let n = acc
        .len()
        .min(r0.len())
        .min(r1.len())
        .min(r2.len())
        .min(r3.len());
    let (acc, r0, r1, r2, r3) = (&mut acc[..n], &r0[..n], &r1[..n], &r2[..n], &r3[..n]);
    for i in 0..n {
        acc[i] += (u16::from(r0[i]) + u16::from(r1[i])) + (u16::from(r2[i]) + u16::from(r3[i]));
    }
}

#[inline(never)]
fn add_row_weighted(acc: &mut [u32], row: &[u8], w: u32) {
    for (a, &p) in acc.iter_mut().zip(row) {
        *a += u32::from(p) * w;
    }
}

#[inline(never)]
fn flush_u16(acc: &mut [u32], acc16: &mut [u16], w: u32) {
    for (a, b) in acc.iter_mut().zip(acc16.iter_mut()) {
        *a += u32::from(*b) * w;
        *b = 0;
    }
}

/// Full-weight rows are summed into `u16` lanes and flushed to `u32` before they could overflow.
const U16_FLUSH_ROWS: u32 = 256; // 255 rows of 255 fit in u16

/// The oriented 32x32 luminance image of `img`.
pub fn luminance32(img: &ImageView<'_>) -> Luma32 {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data = img.data();
    let grid = match img.layout() {
        PixelLayout::Luma8 => grid_sums::<1>(data, w, h),
        PixelLayout::Rgb8 => grid_sums::<3>(data, w, h),
        PixelLayout::Rgba8 => grid_sums::<4>(data, w, h),
    };
    let luma_scale = if img.layout() == PixelLayout::Luma8 {
        1.0
    } else {
        1000.0
    };
    let scale = 1.0 / (luma_scale * w as f64 * h as f64);
    let n = LUMA_SIDE - 1;
    let o = img.orientation().0;
    let mut luma = [0f32; LUMA_SIDE * LUMA_SIDE];
    for r in 0..LUMA_SIDE {
        for c in 0..LUMA_SIDE {
            let (sr, sc) = match o {
                2 => (r, n - c),
                3 => (n - r, n - c),
                4 => (n - r, c),
                5 => (c, r),
                6 => (n - c, r),
                7 => (n - c, n - r),
                8 => (c, n - r),
                _ => (r, c),
            };
            luma[r * LUMA_SIDE + c] = (grid[sr * LUMA_SIDE + sc] as f64 * scale) as f32;
        }
    }
    luma
}

/// Area-resampled luminance sums of a `CH`-channel image in stored orientation: cell `(i, j)` is
/// `sum(weight * Y)` with `Y` = BT.601 x 1000 for colour (the raw value for `CH == 1`).
fn grid_sums<const CH: usize>(data: &[u8], w: usize, h: usize) -> [u64; LUMA_SIDE * LUMA_SIDE] {
    let ch = CH;
    let row_len = w * ch;
    let rows = AxisWeights::new(h, LUMA_SIDE);
    let cols = AxisWeights::new(w, LUMA_SIDE);
    // Weight of a source pixel lying entirely inside a bin, along either axis.
    let full = LUMA_SIDE as u32;
    let full_w = full;

    // Stored-orientation grid of luminance sums (scaled; see `scale` below).
    let mut grid = [0u64; LUMA_SIDE * LUMA_SIDE];
    let mut acc16 = vec![0u16; row_len];
    let mut acc = vec![0u32; row_len];
    for i in 0..LUMA_SIDE {
        // Vertical pass over the rows of bin `i`. `acc` ends up holding the weighted column sums.
        acc.fill(0);
        acc16.fill(0);
        let (full_run, partial) = rows.split_bin(i);
        if let Some((y0, y1)) = full_run {
            let row = |y: usize| &data[y * row_len..(y + 1) * row_len];
            let mut pending = 0u32;
            let mut y = y0;
            while y < y1 {
                let n = (y1 - y).min(4);
                if pending + n as u32 > U16_FLUSH_ROWS - 1 {
                    flush_u16(&mut acc, &mut acc16, full);
                    pending = 0;
                }
                if n == 4 {
                    add_rows4(&mut acc16, row(y), row(y + 1), row(y + 2), row(y + 3));
                } else {
                    for k in 0..n {
                        add_row(&mut acc16, row(y + k));
                    }
                }
                pending += n as u32;
                y += n;
            }
            flush_u16(&mut acc, &mut acc16, full);
        }
        for (y, wy) in partial {
            if wy > 0 {
                let y = y as usize;
                add_row_weighted(&mut acc, &data[y * row_len..(y + 1) * row_len], wy);
            }
        }
        // Horizontal pass: channel sums per column bin (full-weight columns unweighted, then scaled), then
        // BT.601 luminance x 1000 (linear, so it commutes with the sums).
        for (j, o) in grid[i * LUMA_SIDE..(i + 1) * LUMA_SIDE]
            .iter_mut()
            .enumerate()
        {
            let (full_run, partial) = cols.split_bin(j);
            let mut sum = [0u64; 3];
            let mut weighted = [0u64; 3];
            if let Some((x0, x1)) = full_run {
                for p in acc[x0 * CH..x1 * CH].as_chunks::<CH>().0 {
                    for c in 0..CH.min(3) {
                        sum[c] += u64::from(p[c]);
                    }
                }
            }
            for (x, wx) in partial {
                let p = &acc[x as usize * ch..x as usize * ch + ch];
                for c in 0..CH.min(3) {
                    weighted[c] += u64::from(p[c]) * u64::from(wx);
                }
            }
            let cell: [u64; 3] = std::array::from_fn(|c| sum[c] * u64::from(full_w) + weighted[c]);
            *o = if CH == 1 {
                cell[0]
            } else {
                299 * cell[0] + 587 * cell[1] + 114 * cell[2]
            };
        }
    }

    grid
}

/// `alpha(u) * cos(pi * (2x + 1) * u / 64)` for the 8 lowest frequencies.
fn dct_table() -> &'static [[f32; LUMA_SIDE]; 8] {
    static TABLE: OnceLock<[[f32; LUMA_SIDE]; 8]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let n = LUMA_SIDE as f64;
        let mut t = [[0f32; LUMA_SIDE]; 8];
        for (u, row) in t.iter_mut().enumerate() {
            let alpha = if u == 0 {
                (1.0 / n).sqrt()
            } else {
                (2.0 / n).sqrt()
            };
            for (x, v) in row.iter_mut().enumerate() {
                let angle = std::f64::consts::PI * (2 * x + 1) as f64 * u as f64 / (2.0 * n);
                *v = (alpha * angle.cos()) as f32;
            }
        }
        t
    })
}

/// The low 8x8 orthonormal DCT-II coefficients of the luminance image, `[u * 8 + v]`.
pub fn dct8x8(luma: &Luma32) -> [f32; 64] {
    let t = dct_table();
    // Horizontal: h[y][v] = sum_x luma[y][x] * t[v][x].
    let mut h = [[0f32; 8]; LUMA_SIDE];
    for (y, hrow) in h.iter_mut().enumerate() {
        let row = &luma[y * LUMA_SIDE..(y + 1) * LUMA_SIDE];
        for (v, out) in hrow.iter_mut().enumerate() {
            let mut s = 0f32;
            for (a, b) in row.iter().zip(&t[v]) {
                s += a * b;
            }
            *out = s;
        }
    }
    // Vertical: p[u][v] = sum_y t[u][y] * h[y][v].
    let mut p = [0f32; 64];
    for u in 0..8 {
        let mut s = [0f32; 8];
        for (y, hrow) in h.iter().enumerate() {
            let c = t[u][y];
            for v in 0..8 {
                s[v] += c * hrow[v];
            }
        }
        p[u * 8..u * 8 + 8].copy_from_slice(&s);
    }
    p
}

/// DCT pHash of a 32x32 luminance image.
pub fn phash_from_luma(luma: &Luma32) -> u64 {
    let coeffs = dct8x8(luma);
    let mut sorted = coeffs;
    sorted.sort_unstable_by(f32::total_cmp);
    let median = (sorted[31] + sorted[32]) * 0.5;
    coeffs
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, &c)| acc | (u64::from(c > median) << i))
}

/// Gradient dHash of a 32x32 luminance image.
pub fn dhash_from_luma(luma: &Luma32) -> u64 {
    // 32 -> 9 columns with integer coverage weights; 32 -> 8 rows is exactly 4 rows per cell.
    let cols = AxisWeights::new(LUMA_SIDE, 9);
    let mut cells = [[0f32; 9]; 8];
    for (r, cell_row) in cells.iter_mut().enumerate() {
        let mut rowsum = [0f32; LUMA_SIDE];
        for y in r * 4..r * 4 + 4 {
            for (s, v) in rowsum
                .iter_mut()
                .zip(&luma[y * LUMA_SIDE..(y + 1) * LUMA_SIDE])
            {
                *s += v;
            }
        }
        for (c, cell) in cell_row.iter_mut().enumerate() {
            let mut s = 0f32;
            for &(x, wx) in cols.bin(c) {
                s += rowsum[x as usize] * wx as f32;
            }
            *cell = s;
        }
    }
    let mut bits = 0u64;
    for (r, row) in cells.iter().enumerate() {
        for c in 0..8 {
            bits |= u64::from(row[c] < row[c + 1]) << (r * 8 + c);
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps_model::Orientation;

    #[test]
    fn axis_weights_cover_each_bin_exactly() {
        for (src, dst) in [
            (512, 32),
            (32, 32),
            (7, 32),
            (1, 32),
            (1000, 32),
            (32, 9),
            (33, 32),
        ] {
            let w = AxisWeights::new(src, dst);
            assert_eq!(w.starts.len(), dst + 1);
            for j in 0..dst {
                assert_eq!(
                    w.bin(j).iter().map(|&(_, w)| w as usize).sum::<usize>(),
                    src
                );
            }
            // Every source pixel contributes its full weight `dst` in total.
            let mut per_src = vec![0usize; src];
            for j in 0..dst {
                for &(x, wx) in w.bin(j) {
                    per_src[x as usize] += wx as usize;
                }
            }
            assert!(per_src.iter().all(|&s| s == dst), "{src}->{dst}");
        }
    }

    fn rect_rgb(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x * y) % 251) as u8,
                ]);
            }
        }
        v
    }

    /// Physically apply EXIF orientation `o` to a stored image, giving the displayed pixels.
    fn orient(src: &[u8], w: usize, h: usize, o: u8) -> (Vec<u8>, usize, usize) {
        let (dw, dh) = if o >= 5 { (h, w) } else { (w, h) };
        let mut out = vec![0u8; src.len()];
        for r in 0..dh {
            for c in 0..dw {
                let (sr, sc) = match o {
                    2 => (r, w - 1 - c),
                    3 => (h - 1 - r, w - 1 - c),
                    4 => (h - 1 - r, c),
                    5 => (c, r),
                    6 => (h - 1 - c, r),
                    7 => (h - 1 - c, w - 1 - r),
                    8 => (c, w - 1 - r),
                    _ => (r, c),
                };
                let s = (sr * w + sc) * 3;
                let d = (r * dw + c) * 3;
                out[d..d + 3].copy_from_slice(&src[s..s + 3]);
            }
        }
        (out, dw, dh)
    }

    #[test]
    fn orientation_on_grid_equals_rotated_pixels_exactly() {
        let (w, h) = (97, 61);
        let px = rect_rgb(w, h);
        for o in 1..=8u8 {
            let tagged = ImageView::new(w as u32, h as u32, PixelLayout::Rgb8, &px)
                .unwrap()
                .with_orientation(Orientation(o));
            let (rot, rw, rh) = orient(&px, w, h, o);
            let physical = ImageView::new(rw as u32, rh as u32, PixelLayout::Rgb8, &rot).unwrap();
            assert_eq!(
                luminance32(&tagged),
                luminance32(&physical),
                "orientation {o}"
            );
        }
    }

    #[test]
    fn flat_image_has_flat_luminance() {
        let px = vec![200u8; 50 * 40];
        let img = ImageView::new(50, 40, PixelLayout::Luma8, &px).unwrap();
        assert!(luminance32(&img).iter().all(|&v| (v - 200.0).abs() < 1e-4));
        let rgb = vec![100u8; 3 * 3 * 5];
        let img = ImageView::new(3, 5, PixelLayout::Rgb8, &rgb).unwrap();
        assert!(luminance32(&img).iter().all(|&v| (v - 100.0).abs() < 1e-3));
    }

    #[test]
    fn rgba_ignores_alpha_and_matches_rgb() {
        let rgb = rect_rgb(40, 30);
        let rgba: Vec<u8> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], (p[0] ^ 0x5a)])
            .collect();
        let a = perceptual_hash(&ImageView::new(40, 30, PixelLayout::Rgb8, &rgb).unwrap());
        let b = perceptual_hash(&ImageView::new(40, 30, PixelLayout::Rgba8, &rgba).unwrap());
        assert_eq!(a, b);
    }

    #[test]
    fn tall_white_image_does_not_overflow_u16_lanes() {
        // 600 full rows per bin forces several u16 flushes.
        let px = vec![255u8; 2 * 32 * 600 * 3];
        let img = ImageView::new(2, 32 * 600, PixelLayout::Rgb8, &px).unwrap();
        assert!(luminance32(&img).iter().all(|&v| (v - 255.0).abs() < 1e-3));
    }

    #[test]
    fn dct_of_flat_image_is_dc_only() {
        let luma = [128f32; 1024];
        let p = dct8x8(&luma);
        assert!((p[0] - 128.0 * 32.0).abs() < 0.1);
        assert!(p[1..].iter().all(|c| c.abs() < 1e-2));
    }

    #[test]
    fn rejects_bad_buffers() {
        assert!(ImageView::new(0, 4, PixelLayout::Luma8, &[]).is_err());
        assert!(ImageView::new(2, 2, PixelLayout::Rgb8, &[0; 11]).is_err());
    }
}
