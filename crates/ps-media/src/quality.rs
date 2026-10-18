//! JPEG quality estimate from quantization tables (ADR-0017): least-squares match of the luminance table
//! against the IJG standard table scaled for every quality 1-100.

/// IJG / Annex K.1 luminance table, natural (row-major) order.
const STD_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// Zigzag position -> natural index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

fn scaled(q: u32, v: u16) -> f64 {
    let scale = if q < 50 { 5000 / q } else { 200 - q * 2 };
    ((u32::from(v) * scale + 50) / 100).clamp(1, 255) as f64
}

/// Estimate quality (1-100) from a luminance table in zigzag order (as stored in DQT).
pub fn estimate_from_luma(zigzag: &[u16; 64]) -> Option<u8> {
    if zigzag.iter().all(|v| *v == 0) {
        return None;
    }
    let mut natural = [0u16; 64];
    for (z, &n) in ZIGZAG.iter().enumerate() {
        natural[n] = zigzag[z];
    }
    let mut best = (f64::MAX, 0u32);
    for q in 1..=100u32 {
        let err: f64 = natural
            .iter()
            .zip(STD_LUMA.iter())
            .map(|(a, r)| {
                let d = f64::from(*a) - scaled(q, *r);
                d * d
            })
            .sum();
        if err < best.0 {
            best = (err, q);
        }
    }
    u8::try_from(best.1).ok()
}

/// Estimate the quality of a JPEG file.
pub fn estimate(jpeg: &[u8]) -> Option<u8> {
    let scan = crate::jpeg::scan(jpeg, u64::MAX);
    scan.dqt[0].as_ref().and_then(estimate_from_luma)
}
