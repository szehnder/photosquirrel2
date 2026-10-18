//! WCAG 2.1 contrast (relative luminance) for theme checks.

use gpui_kit::{Hsla, Rgba};

/// Normal text needs 4.5:1.
pub const AA_TEXT: f32 = 4.5;
/// Secondary text, large text and UI glyphs (focus rings, icons) need 3:1.
pub const AA_SECONDARY: f32 = 3.0;

pub fn relative_luminance(color: Hsla) -> f32 {
    let rgb = color.to_rgb();
    let channel = |c: f32| {
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

/// Composite `fg` over an opaque `bg` (sRGB alpha blending, as the renderer does).
pub fn over(fg: Hsla, bg: Hsla) -> Hsla {
    let (f, b) = (fg.to_rgb(), bg.to_rgb());
    let a = f.a;
    Hsla::from(Rgba {
        r: f.r * a + b.r * (1.0 - a),
        g: f.g * a + b.g * (1.0 - a),
        b: f.b * a + b.b * (1.0 - a),
        a: 1.0,
    })
}

/// Contrast ratio of `fg` drawn on `bg` (1..=21).
pub fn ratio(fg: Hsla, bg: Hsla) -> f32 {
    let fg = over(fg, bg);
    let (la, lb) = (relative_luminance(fg), relative_luminance(bg));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_wcag_reference_values() {
        assert!((ratio(Hsla::black(), Hsla::white()) - 21.0).abs() < 0.01);
        assert!((ratio(Hsla::white(), Hsla::white()) - 1.0).abs() < 0.001);
        // #777777 on white is the classic 4.48:1.
        let grey = Hsla::from(Rgba {
            r: 0x77 as f32 / 255.0,
            g: 0x77 as f32 / 255.0,
            b: 0x77 as f32 / 255.0,
            a: 1.0,
        });
        assert!((ratio(grey, Hsla::white()) - 4.48).abs() < 0.02);
    }
}
