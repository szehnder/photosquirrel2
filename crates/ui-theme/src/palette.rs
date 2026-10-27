//! The two palettes, authored in Oklch (ADR-0028).
//!
//! Graphite: near-neutral greys with a whisper of cool hue on chrome, *zero* chroma behind photos, and one warm
//! amber accent, "acorn", for selection, focus and primary actions. Photos carry the colour; the UI does not.

use gpui_kit::Hsla;

use crate::color::{Oklch, oklch};

/// Hue of the graphite greys (cool, barely there: chroma stays below 0.01).
const GRAPHITE_HUE: f32 = 260.0;

/// The acorn accent ramp, shared by both themes. `ACORN` itself is identical in light and dark: it is the
/// selection and primary-action colour everywhere. Text drawn on acorn uses [`ON_ACORN`].
pub const ACORN: Oklch = oklch(0.79, 0.150, 68.0);
pub const ACORN_HOVER: Oklch = oklch(0.83, 0.140, 72.0);
pub const ACORN_ACTIVE: Oklch = oklch(0.73, 0.150, 64.0);
/// A deeper acorn for glyphs and focus rings on light surfaces, where `ACORN` lacks contrast.
pub const ACORN_DEEP: Oklch = oklch(0.56, 0.125, 58.0);
/// Text and icons on an acorn fill.
pub const ON_ACORN: Oklch = oklch(0.22, 0.035, 60.0);

/// Light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

/// Every colour role of one theme. gpui-kit's theme file is generated from this ([`crate::theme_json`]), and
/// roles gpui-kit has no slot for (photo surround, acorn ramp) reach views through [`Colors`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub mode: Mode,
    /// Title bar and status bar.
    pub chrome: Oklch,
    /// The window background behind the central view.
    pub background: Oklch,
    /// Navigator and inspector.
    pub panel: Oklch,
    /// Controls resting on a panel (secondary buttons, inputs, segmented bars).
    pub raised: Oklch,
    /// Popovers, the palette and the settings window.
    pub overlay: Oklch,
    /// Hover on rows and ghost buttons.
    pub hover: Oklch,
    /// Pressed rows and the active navigator entry's base.
    pub pressed: Oklch,
    /// Hairlines between regions.
    pub border: Oklch,
    /// Input outlines and stronger separators.
    pub border_strong: Oklch,
    /// Behind photos in the grid (neutral: chroma 0).
    pub gallery: Oklch,
    /// Behind a single photo in the loupe (neutral: chroma 0).
    pub loupe: Oklch,
    /// Primary text.
    pub text: Oklch,
    /// Secondary text: labels, metadata keys.
    pub text_muted: Oklch,
    /// Tertiary text: hints, placeholders, disabled.
    pub text_faint: Oklch,
    /// Selection and primary fills.
    pub accent: Oklch,
    pub accent_hover: Oklch,
    pub accent_active: Oklch,
    pub on_accent: Oklch,
    /// The accent as a glyph or text colour on this theme's surfaces (links, active icons, focus ring).
    pub accent_text: Oklch,
    /// Tinted background for selected rows.
    pub accent_soft: Oklch,
    pub danger: Oklch,
    pub warning: Oklch,
    pub success: Oklch,
    pub info: Oklch,
    /// Text on the semantic fills above.
    pub on_semantic: Oklch,
    /// Dim layer behind modal overlays.
    pub scrim: Oklch,
}

impl Palette {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::dark(),
            Mode::Light => Self::light(),
        }
    }

    /// Graphite dark, the default.
    pub fn dark() -> Self {
        let g = |l: f32| oklch(l, 0.006, GRAPHITE_HUE);
        Self {
            mode: Mode::Dark,
            chrome: g(0.175),
            background: g(0.205),
            panel: g(0.225),
            raised: g(0.265),
            overlay: g(0.255),
            hover: g(0.285),
            pressed: g(0.315),
            border: g(0.290),
            border_strong: g(0.360),
            gallery: oklch(0.190, 0.0, 0.0),
            loupe: oklch(0.150, 0.0, 0.0),
            text: oklch(0.935, 0.004, 80.0),
            text_muted: oklch(0.745, 0.006, GRAPHITE_HUE),
            text_faint: oklch(0.600, 0.006, GRAPHITE_HUE),
            accent: ACORN,
            accent_hover: ACORN_HOVER,
            accent_active: ACORN_ACTIVE,
            on_accent: ON_ACORN,
            accent_text: ACORN,
            accent_soft: ACORN.alpha(0.16),
            danger: oklch(0.680, 0.170, 25.0),
            warning: oklch(0.800, 0.140, 85.0),
            success: oklch(0.740, 0.140, 150.0),
            info: oklch(0.730, 0.110, 240.0),
            on_semantic: oklch(0.180, 0.010, 60.0),
            scrim: oklch(0.0, 0.0, 0.0).alpha(0.55),
        }
    }

    /// Graphite light.
    pub fn light() -> Self {
        let g = |l: f32| oklch(l, 0.004, GRAPHITE_HUE);
        Self {
            mode: Mode::Light,
            chrome: g(0.935),
            background: g(0.975),
            panel: g(0.955),
            raised: g(0.995),
            overlay: g(0.995),
            hover: g(0.915),
            pressed: g(0.885),
            border: g(0.885),
            border_strong: g(0.810),
            gallery: oklch(0.910, 0.0, 0.0),
            loupe: oklch(0.860, 0.0, 0.0),
            text: oklch(0.225, 0.006, GRAPHITE_HUE),
            text_muted: oklch(0.460, 0.008, GRAPHITE_HUE),
            text_faint: oklch(0.560, 0.008, GRAPHITE_HUE),
            accent: ACORN,
            accent_hover: ACORN_HOVER,
            accent_active: ACORN_ACTIVE,
            on_accent: ON_ACORN,
            accent_text: ACORN_DEEP,
            accent_soft: ACORN.alpha(0.22),
            danger: oklch(0.680, 0.170, 25.0),
            warning: oklch(0.800, 0.140, 85.0),
            success: oklch(0.740, 0.140, 150.0),
            info: oklch(0.730, 0.110, 240.0),
            on_semantic: oklch(0.180, 0.010, 60.0),
            scrim: oklch(0.0, 0.0, 0.0).alpha(0.30),
        }
    }

    /// Resolve every role to `Hsla` for painting.
    pub fn resolve(&self) -> Colors {
        let c = |o: Oklch| o.to_hsla();
        Colors {
            dark: self.mode == Mode::Dark,
            chrome: c(self.chrome),
            background: c(self.background),
            panel: c(self.panel),
            raised: c(self.raised),
            overlay: c(self.overlay),
            hover: c(self.hover),
            pressed: c(self.pressed),
            border: c(self.border),
            border_strong: c(self.border_strong),
            gallery: c(self.gallery),
            loupe: c(self.loupe),
            text: c(self.text),
            text_muted: c(self.text_muted),
            text_faint: c(self.text_faint),
            accent: c(self.accent),
            accent_hover: c(self.accent_hover),
            accent_active: c(self.accent_active),
            on_accent: c(self.on_accent),
            accent_text: c(self.accent_text),
            accent_soft: c(self.accent_soft),
            selection: c(self.accent),
            danger: c(self.danger),
            warning: c(self.warning),
            success: c(self.success),
            info: c(self.info),
            on_semantic: c(self.on_semantic),
            scrim: c(self.scrim),
        }
    }
}

/// The active theme's colours, resolved for painting. Read with [`crate::colors`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub dark: bool,
    pub chrome: Hsla,
    pub background: Hsla,
    pub panel: Hsla,
    pub raised: Hsla,
    pub overlay: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub gallery: Hsla,
    pub loupe: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_active: Hsla,
    pub on_accent: Hsla,
    pub accent_text: Hsla,
    pub accent_soft: Hsla,
    /// Selection highlight (photos, rows): always the acorn accent.
    pub selection: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub info: Hsla,
    pub on_semantic: Hsla,
    pub scrim: Hsla,
}

impl Colors {
    /// A transparent version of `color` for soft tints (status chips, hover washes).
    pub fn tint(color: Hsla, alpha: f32) -> Hsla {
        color.opacity(alpha)
    }

    /// A recessed well for monospace command blocks inside panels and tinted notices: the chrome in dark (below
    /// the panel), the raised white in light, always with a [`Colors::border_strong`] outline. `raised`, used
    /// before, was barely distinguishable from an info notice in dark.
    pub fn inset(&self) -> Hsla {
        if self.dark { self.chrome } else { self.raised }
    }

    /// The soft surface of a placeholder tile, a photo without a preview (ADR-0109): a step lighter than the
    /// gallery surround in dark, darker in light, never a colour of its own.
    pub fn placeholder_surface(&self) -> Hsla {
        crate::contrast::over(
            Self::tint(self.text, if self.dark { 0.075 } else { 0.065 }),
            self.gallery,
        )
    }

    /// The faint diagonal hatch over [`Colors::placeholder_surface`].
    pub fn placeholder_stroke(&self) -> Hsla {
        Self::tint(self.text, if self.dark { 0.07 } else { 0.08 })
    }

    /// Alpha of the soft background of a status chip ([`Colors::chip_bg`]).
    pub const CHIP_TINT: f32 = 0.14;

    /// The soft background of a status chip in `tone` (success, info, warning, danger).
    pub fn chip_bg(tone: Hsla) -> Hsla {
        Self::tint(tone, Self::CHIP_TINT)
    }

    /// Text in `tone` that reads as normal text (WCAG AA 4.5:1) on its chip background over a panel: the tone
    /// itself where it is already dark (light theme) or light (dark theme) enough, else the tone moved
    /// towards black or white in lightness.
    pub fn tone_text(&self, tone: Hsla) -> Hsla {
        let bg = crate::contrast::over(Self::chip_bg(tone), self.panel);
        let mut fg = tone;
        for _ in 0..40 {
            if crate::contrast::ratio(fg, bg) >= crate::contrast::AA_TEXT {
                break;
            }
            fg.l = if self.dark {
                (fg.l + 0.02).min(1.0)
            } else {
                (fg.l - 0.02).max(0.0)
            };
        }
        fg
    }
}

#[cfg(test)]
mod inset_tests {
    use super::*;

    /// The command block stands out from an info notice (info at 8% over a panel) in both themes, clearly more
    /// than the raised surface did in dark, and its outline more still.
    #[test]
    fn inset_wells_stand_out_from_notices() {
        for mode in [Mode::Dark, Mode::Light] {
            let c = Palette::for_mode(mode).resolve();
            let notice = crate::contrast::over(Colors::tint(c.info, 0.08), c.panel);
            let well = crate::contrast::ratio(c.inset(), notice);
            let outline = crate::contrast::ratio(c.border_strong, notice);
            if c.dark {
                let before = crate::contrast::ratio(c.raised, notice);
                assert!(well > before * 1.1, "{mode:?}: {well} vs raised {before}");
                assert!(well >= 1.2, "{mode:?}: {well}");
            } else {
                assert!(well >= 1.05, "{mode:?}: {well}");
            }
            assert!(outline >= 1.3, "{mode:?}: outline {outline}");
        }
    }
}
