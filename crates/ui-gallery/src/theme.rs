//! Gallery-specific tokens (ADR-0028): cell geometry, chrome sizes and the few colours the grid derives from
//! the active palette. This module is the only place in the crate with pixel literals (`cargo xtask tokens`
//! exempts `theme.rs`).

use gpui_kit::{App, Hsla, Pixels, Rgba, px};
use ui_theme::Colors;

/// Cell size range (gallery spec: 96-512 px) and the default.
pub const MIN_CELL: f32 = 96.0;
pub const MAX_CELL: f32 = 512.0;
pub const DEFAULT_CELL: f32 = 156.0;
/// One zoom step multiplies the cell size by this.
pub const ZOOM_STEP: f32 = 1.18;

/// Gap between cells and the padding around the grid, in logical px.
pub const CELL_GAP: f32 = 6.0;
pub const GRID_PADDING: f32 = 14.0;
/// Inset of the photo inside its tile (fraction of the cell size): the tile frames the picture.
pub const PHOTO_INSET: f32 = 0.055;
pub const TILE_RADIUS: Pixels = px(5.0);
pub const PHOTO_RADIUS: Pixels = px(2.0);
pub const SELECTION_WIDTH: Pixels = px(2.0);
pub const FOCUS_WIDTH: Pixels = px(1.0);

/// Scrollbar and timeline scrubber.
pub const SCROLLBAR_WIDTH: f32 = 6.0;
pub const SCROLLBAR_HOVER_WIDTH: f32 = 10.0;
pub const SCROLLBAR_MIN_THUMB: f32 = 36.0;
pub const SCROLLBAR_MARGIN: f32 = 3.0;
pub const SCRUBBER_WIDTH: f32 = 58.0;
pub const SCRUBBER_LABEL_PX: f32 = 10.5;
pub const SCRUBBER_MIN_LABEL_GAP: f32 = 30.0;
pub const SCRUBBER_TICK: f32 = 8.0;

/// Badges on tiles (stack count, video).
pub const BADGE_PX: f32 = 11.0;
pub const BADGE_HEIGHT: f32 = 18.0;
pub const BADGE_PAD: f32 = 6.0;
pub const BADGE_MARGIN: f32 = 6.0;

/// Loupe and filmstrip.
pub const FILMSTRIP_HEIGHT: f32 = 84.0;
pub const FILMSTRIP_CELL: f32 = 64.0;
pub const FILMSTRIP_GAP: f32 = 4.0;
pub const LOUPE_PADDING: f32 = 24.0;
pub const NOTICE_PX: f32 = 12.0;

/// Line-scroll step for mouse wheels, as a fraction of a cell pitch.
pub const WHEEL_LINE_FRACTION: f32 = 0.34;

/// Logical px as `Pixels` (for computed geometry).
pub fn lpx(v: f32) -> Pixels {
    px(v)
}

/// A colour from an sRGB triple (dominant-colour placeholders).
pub fn dominant_colour(c: [u8; 3]) -> Hsla {
    Rgba {
        r: f32::from(c[0]) / 255.0,
        g: f32::from(c[1]) / 255.0,
        b: f32::from(c[2]) / 255.0,
        a: 1.0,
    }
    .into()
}

/// The colours the grid paints with, resolved once per frame.
#[derive(Clone, Copy, Debug)]
pub struct GridColors {
    pub background: Hsla,
    /// The tile behind each photo: a touch lighter than the surround in dark mode, darker in light mode.
    pub tile: Hsla,
    pub tile_hover: Hsla,
    /// Placeholder when no dominant colour is stored.
    pub placeholder: Hsla,
    pub selection: Hsla,
    pub selection_tile: Hsla,
    pub focus: Hsla,
    pub badge_bg: Hsla,
    pub badge_text: Hsla,
    pub keeper: Hsla,
    pub on_keeper: Hsla,
    pub member_tile: Hsla,
    pub scrollbar: Hsla,
    pub scrollbar_hover: Hsla,
    pub scrubber_text: Hsla,
    pub scrubber_strong: Hsla,
    pub scrubber_tick: Hsla,
    pub notice_bg: Hsla,
    pub notice_text: Hsla,
    pub loupe: Hsla,
    pub missing: Hsla,
}

impl GridColors {
    pub fn from_colors(c: &Colors) -> Self {
        let mix = |a: Hsla, b: Hsla, t: f32| -> Hsla {
            let a = a.to_rgb();
            let b = b.to_rgb();
            Rgba {
                r: a.r + (b.r - a.r) * t,
                g: a.g + (b.g - a.g) * t,
                b: a.b + (b.b - a.b) * t,
                a: 1.0,
            }
            .into()
        };
        let tile = if c.dark {
            mix(c.gallery, c.text, 0.055)
        } else {
            mix(c.gallery, c.text, 0.06)
        };
        Self {
            background: c.gallery,
            tile,
            tile_hover: mix(c.gallery, c.text, if c.dark { 0.10 } else { 0.11 }),
            placeholder: mix(c.gallery, c.text, 0.14),
            selection: c.accent,
            selection_tile: mix(tile, c.accent, if c.dark { 0.22 } else { 0.18 }),
            focus: Colors::tint(c.text, 0.55),
            badge_bg: Colors::tint(mix(c.gallery, c.chrome, 0.5), 0.86),
            badge_text: c.text,
            keeper: c.accent,
            on_keeper: c.on_accent,
            member_tile: mix(tile, c.accent, 0.08),
            scrollbar: Colors::tint(c.text, 0.22),
            scrollbar_hover: Colors::tint(c.text, 0.45),
            scrubber_text: c.text_faint,
            scrubber_strong: c.text_muted,
            scrubber_tick: Colors::tint(c.text, 0.16),
            notice_bg: Colors::tint(c.chrome, 0.9),
            notice_text: c.text,
            loupe: c.loupe,
            missing: c.warning,
        }
    }

    pub fn current(cx: &App) -> Self {
        Self::from_colors(ui_theme::colors(cx))
    }
}
