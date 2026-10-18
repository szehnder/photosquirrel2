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
}
