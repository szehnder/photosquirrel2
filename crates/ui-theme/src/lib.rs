//! Design tokens, themes and bundled fonts (ADR-0028).
//!
//! - Colours are authored in Oklch ([`palette`]) and reach UI code two ways: gpui-kit's theme (generated
//!   into `assets/themes/graphite-*.json`, read with `cx.theme()`), and [`colors`] for roles gpui-kit has no
//!   slot for (photo surround, the acorn ramp, text tiers).
//! - Sizes, type and motion come from [`Tokens`] and [`Motion`].
//! - This crate is the only UI code allowed to contain raw colours or pixel literals (`cargo xtask tokens`).

pub mod color;
pub mod contrast;
pub mod fonts;
pub mod palette;
pub mod theme_json;
pub mod tokens;

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, SharedString, WindowAppearance, px};

pub use color::{Oklch, oklch};
pub use palette::{ACORN, Colors, Mode, Palette};
pub use theme_json::{DARK_THEME, LIGHT_THEME};
pub use tokens::{
    DEFAULT_MONO_FONT_SIZE, DEFAULT_UI_FONT_SIZE, Motion, Tokens, clamp_ui_font_size,
};

/// The bundled gpui-kit theme files, generated from [`Palette`] (see [`theme_json`]).
pub const THEME_DARK_JSON: &str = include_str!("../../../assets/themes/graphite-dark.json");
pub const THEME_LIGHT_JSON: &str = include_str!("../../../assets/themes/graphite-light.json");

/// The user's theme choice. Graphite dark is the default (photo-first).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    /// Follow the operating system's appearance, live.
    System,
}

impl ThemeChoice {
    pub fn resolve(self, appearance: WindowAppearance) -> Mode {
        match self {
            ThemeChoice::Dark => Mode::Dark,
            ThemeChoice::Light => Mode::Light,
            ThemeChoice::System => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
            },
        }
    }
}

/// The active theme's resolved colours.
struct ActiveColors(Colors);

impl Global for ActiveColors {}

/// Colours of the active theme for roles outside gpui-kit's theme (photo surround, acorn ramp, text tiers).
pub fn colors(cx: &App) -> &Colors {
    &cx.global::<ActiveColors>().0
}

/// The active mode.
pub fn mode(cx: &App) -> Mode {
    if Theme::global(cx).mode.is_dark() {
        Mode::Dark
    } else {
        Mode::Light
    }
}

/// The name gpui-kit reports for the active theme (`Graphite Dark` / `Graphite Light`).
pub fn active_theme_name(cx: &App) -> SharedString {
    let theme = Theme::global(cx);
    if theme.mode.is_dark() {
        theme.dark_theme.name.clone()
    } else {
        theme.light_theme.name.clone()
    }
}

/// Register fonts and both themes and activate graphite dark. Call once, after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    if let Err(err) = fonts::register(cx) {
        log::warn!("failed to register bundled fonts: {err:#}");
    }
    for json in [THEME_DARK_JSON, THEME_LIGHT_JSON] {
        if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(json) {
            log::error!("bundled theme does not parse: {err:#}");
        }
    }
    let registry = ThemeRegistry::global(cx);
    let light = registry.themes().get(LIGHT_THEME).cloned();
    let dark = registry.themes().get(DARK_THEME).cloned();
    if let (Some(light), Some(dark)) = (light, dark) {
        let theme = Theme::global_mut(cx);
        theme.light_theme = light;
        theme.dark_theme = dark;
    }
    cx.set_global(ActiveColors(Palette::dark().resolve()));
    // Force the first application even though gpui-kit may already be in dark mode.
    Theme::change(ThemeMode::Dark, None, cx);
    apply_font_size(DEFAULT_UI_FONT_SIZE, cx);
}

/// Switch to `mode` and `ui_font_size` live: every window repaints. Cheap when nothing changed.
pub fn apply(mode: Mode, ui_font_size: f32, cx: &mut App) {
    let target = match mode {
        Mode::Dark => ThemeMode::Dark,
        Mode::Light => ThemeMode::Light,
    };
    let changed = Theme::global(cx).mode != target || self::mode(cx) != mode;
    if changed || *colors(cx) != Palette::for_mode(mode).resolve() {
        // Loading the mode's theme resets font sizes to the file's, so sizes are applied afterwards.
        Theme::change(target, None, cx);
        cx.set_global(ActiveColors(Palette::for_mode(mode).resolve()));
    }
    apply_font_size(ui_font_size, cx);
    cx.refresh_windows();
}

fn apply_font_size(ui_font_size: f32, cx: &mut App) {
    let ui = px(clamp_ui_font_size(ui_font_size));
    let mono = px(clamp_ui_font_size(ui_font_size) * DEFAULT_MONO_FONT_SIZE / DEFAULT_UI_FONT_SIZE);
    let theme = Theme::global(cx);
    if theme.font_size != ui || theme.mono_font_size != mono {
        Theme::update(cx, |theme| {
            theme.font_size = ui;
            theme.mono_font_size = mono;
        });
    }
}

/// Tooltip plus accessible name for icon-only buttons (a tooltip alone is not announced).
pub trait ButtonTip {
    fn tip(self, text: impl Into<SharedString>) -> Self;
}

impl ButtonTip for gpui_kit::component::button::Button {
    fn tip(self, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        self.accessibility_label(text.clone()).tooltip(text)
    }
}

#[cfg(test)]
mod tests;
