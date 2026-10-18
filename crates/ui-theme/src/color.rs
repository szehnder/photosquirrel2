//! Colour tokens are authored in Oklch (perceptual lightness, chroma, hue) and converted to GPUI's `Hsla`
//! (through sRGB) at the edge. Oklch makes "neutral" measurable (chroma) and keeps lightness steps even.
//!
//! Conversions follow Björn Ottosson's reference Oklab matrices.

use gpui_kit::{Hsla, Rgba};

/// A colour in Oklch: `l` in 0..=1, `c` (chroma) >= 0, `h` (hue) in degrees, `a` alpha in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub a: f32,
}

/// Shorthand constructor for opaque tokens.
pub const fn oklch(l: f32, c: f32, h: f32) -> Oklch {
    Oklch { l, c, h, a: 1.0 }
}

impl Oklch {
    /// The same colour with alpha `a`.
    pub const fn alpha(self, a: f32) -> Self {
        Oklch { a, ..self }
    }

    /// The same hue and chroma at lightness `l`.
    pub const fn with_l(self, l: f32) -> Self {
        Oklch { l, ..self }
    }

    /// Straight-alpha sRGB in 0..=1 (out-of-gamut channels are clamped).
    pub fn to_rgba(self) -> Rgba {
        let (a, b) = {
            let h = self.h.to_radians();
            (self.c * h.cos(), self.c * h.sin())
        };
        let l_ = self.l + 0.396_337_78 * a + 0.215_803_76 * b;
        let m_ = self.l - 0.105_561_346 * a - 0.063_854_17 * b;
        let s_ = self.l - 0.089_484_18 * a - 1.291_485_5 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
        let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
        let bl = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
        Rgba {
            r: encode(r),
            g: encode(g),
            b: encode(bl),
            a: self.a.clamp(0.0, 1.0),
        }
    }

    pub fn to_hsla(self) -> Hsla {
        Hsla::from(self.to_rgba())
    }

    /// `#rrggbb` or `#rrggbbaa` (when not opaque), as gpui-kit theme files expect.
    pub fn to_hex(self) -> String {
        let c = self.to_rgba();
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut s = format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b));
        if c.a < 1.0 {
            s.push_str(&format!("{:02x}", byte(c.a)));
        }
        s
    }

    /// Back from sRGB (used by tests to check what is actually painted).
    pub fn from_rgba(c: Rgba) -> Self {
        let (r, g, b) = (decode(c.r), decode(c.g), decode(c.b));
        let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
        let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
        let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
        let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
        let ll = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
        let aa = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
        let bb = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;
        let c_ = (aa * aa + bb * bb).sqrt();
        let mut h = bb.atan2(aa).to_degrees();
        if h < 0.0 {
            h += 360.0;
        }
        Oklch {
            l: ll,
            c: c_,
            h,
            a: c.a,
        }
    }

    pub fn from_hsla(c: Hsla) -> Self {
        Self::from_rgba(c.to_rgb())
    }
}

/// Linear light to gamma-encoded sRGB.
fn encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn decode(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_black_and_grey_round_trip() {
        assert_eq!(oklch(1.0, 0.0, 0.0).to_hex(), "#ffffff");
        assert_eq!(oklch(0.0, 0.0, 0.0).to_hex(), "#000000");
        let grey = Oklch::from_rgba(oklch(0.5, 0.0, 0.0).to_rgba());
        assert!((grey.l - 0.5).abs() < 0.002, "{grey:?}");
        assert!(grey.c < 0.001, "{grey:?}");
    }

    #[test]
    fn known_reference_colour() {
        // CSS Color 4: oklch(0.628 0.2577 29.23) is sRGB red.
        assert_eq!(oklch(0.627_955, 0.257_683, 29.234).to_hex(), "#ff0000");
        let red = Oklch::from_rgba(Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        assert!((red.h - 29.23).abs() < 0.1 && (red.c - 0.2577).abs() < 0.002);
    }

    #[test]
    fn alpha_is_written_when_translucent() {
        assert_eq!(oklch(1.0, 0.0, 0.0).alpha(0.5).to_hex(), "#ffffff80");
    }
}
