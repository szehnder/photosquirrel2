//! Size, type and motion tokens (ADR-0028). UI crates take every spacing, radius, font size, component size
//! and duration from here; `cargo xtask tokens` rejects raw `px(..)` literals and colours elsewhere.
//!
//! Spacing and type are in rems, so they scale with the UI font size (the zoom); hairlines and radii are
//! fixed pixels.

use std::time::Duration;

use gpui_kit::{App, Pixels, Rems, px, rems};

/// UI text size in logical pixels; also the rem size. Inter at 13 px is dense and crisp, like a pro tool.
pub const DEFAULT_UI_FONT_SIZE: f32 = 13.0;
/// Technical metadata (EXIF, hashes, paths) in JetBrains Mono.
pub const DEFAULT_MONO_FONT_SIZE: f32 = 12.0;
pub const MIN_UI_FONT_SIZE: f32 = 11.0;
pub const MAX_UI_FONT_SIZE: f32 = 20.0;

pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS_LG: f32 = 10.0;

/// Spacing, radii, type and component sizes.
pub struct Tokens;

impl Tokens {
    /// `n` steps of the 4 px grid (at the default UI size).
    pub fn space(n: f32) -> Rems {
        rems(n * 4.0 / DEFAULT_UI_FONT_SIZE)
    }

    /// A component size given in default-size pixels; scales with zoom like [`Self::space`].
    pub fn size(default_px: f32) -> Rems {
        rems(default_px / DEFAULT_UI_FONT_SIZE)
    }

    /// Hairline borders do not scale.
    pub const BORDER: Pixels = px(1.0);
    /// Selection and focus outlines.
    pub const OUTLINE: Pixels = px(2.0);
    pub const RADIUS_SM: Pixels = px(RADIUS_SM);
    pub const RADIUS_MD: Pixels = px(RADIUS_MD);
    pub const RADIUS_LG: Pixels = px(RADIUS_LG);

    // Type scale (rems of the UI font size).
    pub fn text_2xs() -> Rems {
        rems(10.0 / DEFAULT_UI_FONT_SIZE)
    }
    pub fn text_xs() -> Rems {
        rems(11.0 / DEFAULT_UI_FONT_SIZE)
    }
    pub fn text_sm() -> Rems {
        rems(12.0 / DEFAULT_UI_FONT_SIZE)
    }
    pub fn text_md() -> Rems {
        rems(1.0)
    }
    pub fn text_lg() -> Rems {
        rems(15.0 / DEFAULT_UI_FONT_SIZE)
    }
    pub fn text_xl() -> Rems {
        rems(19.0 / DEFAULT_UI_FONT_SIZE)
    }
    pub fn text_2xl() -> Rems {
        rems(24.0 / DEFAULT_UI_FONT_SIZE)
    }
    pub fn text_display() -> Rems {
        rems(32.0 / DEFAULT_UI_FONT_SIZE)
    }
    /// Monospace metadata, slightly smaller than body text so it sits optically level.
    pub fn text_mono() -> Rems {
        rems(DEFAULT_MONO_FONT_SIZE / DEFAULT_UI_FONT_SIZE)
    }

    // Shell geometry.
    pub fn title_bar_height() -> Rems {
        Self::size(40.0)
    }
    pub fn status_bar_height() -> Rems {
        Self::size(26.0)
    }
    pub fn nav_row_height() -> Rems {
        Self::size(30.0)
    }
    pub fn icon_sm() -> Rems {
        Self::size(14.0)
    }
    pub fn icon_md() -> Rems {
        Self::size(16.0)
    }
    pub fn icon_lg() -> Rems {
        Self::size(20.0)
    }
    /// The hero mark on empty states.
    pub fn hero_size() -> Rems {
        Self::size(72.0)
    }
    pub fn dot_size() -> Rems {
        Self::size(7.0)
    }
    /// Max width of onboarding and empty-state copy.
    pub fn prose_width() -> Rems {
        Self::size(520.0)
    }
    pub fn search_width() -> Rems {
        Self::size(380.0)
    }
    pub fn control_height() -> Rems {
        Self::size(28.0)
    }
    /// Settings window size (an overlay inside the main window).
    pub fn settings_width() -> Rems {
        Self::size(860.0)
    }
    pub fn settings_height() -> Rems {
        Self::size(600.0)
    }
    pub fn settings_nav_width() -> Rems {
        Self::size(196.0)
    }
    pub fn settings_field_width() -> Rems {
        Self::size(240.0)
    }
    pub fn palette_width() -> Rems {
        Self::size(600.0)
    }
    pub fn palette_list_height() -> Rems {
        Self::size(360.0)
    }
    /// Distance of overlays from the top of the window.
    pub fn overlay_top() -> Rems {
        Self::size(88.0)
    }
    /// Keyboard hint chips.
    pub fn kbd_height() -> Rems {
        Self::size(18.0)
    }
    /// Grip area of panel splitters (the visible line stays a hairline).
    pub const SPLITTER_GRIP: Pixels = px(6.0);

    /// Panel widths in logical pixels (persisted as such).
    pub const NAVIGATOR_WIDTH: Pixels = px(232.0);
    pub const NAVIGATOR_MIN: Pixels = px(184.0);
    pub const NAVIGATOR_MAX: Pixels = px(400.0);
    pub const INSPECTOR_WIDTH: Pixels = px(300.0);
    pub const INSPECTOR_MIN: Pixels = px(240.0);
    pub const INSPECTOR_MAX: Pixels = px(560.0);
    /// The central view never gets narrower than this when panels are resized.
    pub const CENTER_MIN: Pixels = px(360.0);
}

/// Motion durations (ease-out). Every animation takes its duration from here, through
/// [`Motion::duration`], so reduce motion turns all of them into 0 ms at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// Hover and press feedback, chips.
    Fast,
    /// Panels collapsing, overlays appearing.
    Normal,
    /// View transitions, the onboarding hero.
    Slow,
    /// The live scan indicator's pulse period.
    Pulse,
}

impl Motion {
    pub const ALL: [Motion; 4] = [Motion::Fast, Motion::Normal, Motion::Slow, Motion::Pulse];

    /// The nominal duration, ignoring reduce motion.
    pub const fn nominal_millis(self) -> u64 {
        match self {
            Motion::Fast => 110,
            Motion::Normal => 180,
            Motion::Slow => 280,
            Motion::Pulse => 1400,
        }
    }

    /// The duration to animate for: zero when reduce motion is on.
    pub fn resolve(self, reduce_motion: bool) -> Duration {
        if reduce_motion {
            Duration::ZERO
        } else {
            Duration::from_millis(self.nominal_millis())
        }
    }

    /// [`Self::resolve`] against the app's reduce-motion state (set from settings and the OS).
    pub fn duration(self, cx: &App) -> Duration {
        self.resolve(cx.reduce_motion())
    }
}

/// Clamp a UI font size to the supported range (non-finite values fall back to the default).
pub fn clamp_ui_font_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE).round()
    } else {
        DEFAULT_UI_FONT_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_a_4px_grid_at_default_size() {
        let px_of = |r: Rems| r.0 * DEFAULT_UI_FONT_SIZE;
        assert!((px_of(Tokens::space(1.0)) - 4.0).abs() < 1e-4);
        assert!((px_of(Tokens::space(3.0)) - 12.0).abs() < 1e-4);
    }

    #[test]
    fn reduce_motion_zeroes_every_duration() {
        for m in Motion::ALL {
            assert_eq!(m.resolve(true), Duration::ZERO);
            assert!(m.resolve(false) > Duration::ZERO);
        }
    }

    #[test]
    fn font_sizes_clamp() {
        assert_eq!(clamp_ui_font_size(2.0), MIN_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(f32::NAN), DEFAULT_UI_FONT_SIZE);
    }
}
