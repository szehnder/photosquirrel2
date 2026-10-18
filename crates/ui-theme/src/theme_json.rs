//! gpui-kit theme files generated from the Oklch palettes.
//!
//! `assets/themes/graphite-{dark,light}.json` are checked-in outputs of [`theme_set`]; the app loads the files
//! (no generation at start-up), and `bundled_theme_files_match_the_palettes` fails when they drift. Regenerate
//! with `UPDATE_THEMES=1 cargo test -p ui-theme theme_json`.

use serde_json::{Map, Value, json};

use crate::fonts::{MONO_FONT_FAMILY, UI_FONT_FAMILY};
use crate::palette::{Mode, Palette};
use crate::tokens::{DEFAULT_MONO_FONT_SIZE, DEFAULT_UI_FONT_SIZE, RADIUS_LG, RADIUS_MD};

pub const DARK_THEME: &str = "Graphite Dark";
pub const LIGHT_THEME: &str = "Graphite Light";

/// The theme name gpui-kit registers for `mode`.
pub fn theme_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Dark => DARK_THEME,
        Mode::Light => LIGHT_THEME,
    }
}

/// A one-theme gpui-kit `ThemeSet` for `palette`.
pub fn theme_set(palette: &Palette) -> Value {
    let p = palette;
    let hex = |c: crate::Oklch| Value::String(c.to_hex());
    let mut colors = Map::new();
    let mut put = |k: &str, v: Value| {
        colors.insert(k.to_owned(), v);
    };
    let dark = p.mode == Mode::Dark;

    put("background", hex(p.background));
    put("foreground", hex(p.text));
    put("border", hex(p.border));
    put("input.border", hex(p.border_strong));
    put("ring", hex(p.accent_text));
    put("caret", hex(p.accent_text));
    put("selection.background", hex(p.accent.alpha(0.32)));
    put("overlay", hex(p.scrim));
    put("window.border", hex(p.border));
    put("drag.border", hex(p.accent));
    put("drop_target.background", hex(p.accent.alpha(0.14)));

    put("accent.background", hex(p.hover));
    put("accent.foreground", hex(p.text));
    put("muted.background", hex(p.raised));
    put("muted.foreground", hex(p.text_muted));

    put("primary.background", hex(p.accent));
    put("primary.hover.background", hex(p.accent_hover));
    put("primary.active.background", hex(p.accent_active));
    put("primary.foreground", hex(p.on_accent));
    put("secondary.background", hex(p.raised));
    put("secondary.hover.background", hex(p.hover));
    put("secondary.active.background", hex(p.pressed));
    put("secondary.foreground", hex(p.text));

    for (name, color) in [
        ("danger", p.danger),
        ("warning", p.warning),
        ("success", p.success),
        ("info", p.info),
    ] {
        put(&format!("{name}.background"), hex(color));
        put(
            &format!("{name}.hover.background"),
            hex(color.with_l(color.l + 0.04)),
        );
        put(
            &format!("{name}.active.background"),
            hex(color.with_l(color.l - 0.05)),
        );
        put(&format!("{name}.foreground"), hex(p.on_semantic));
    }

    put("link", hex(p.accent_text));
    put(
        "link.hover",
        hex(p.accent_hover.with_l(p.accent_text.l + 0.04)),
    );
    put("link.active", hex(p.accent_text));

    put("popover.background", hex(p.overlay));
    put("popover.foreground", hex(p.text));
    put("group_box.background", hex(p.panel));
    put("group_box.foreground", hex(p.text));
    put("group_box.title.foreground", hex(p.text_muted));
    put("description_list.label.background", hex(p.panel));
    put("description_list.label.foreground", hex(p.text_muted));
    put("accordion.background", hex(p.panel));

    put("list.background", hex(p.panel));
    put("list.even.background", hex(p.panel));
    put("list.head.background", hex(p.panel));
    put("list.hover.background", hex(p.hover));
    put("list.active.background", hex(p.accent_soft));
    put("list.active.border", hex(p.accent));
    put("table.background", hex(p.background));
    put("table.even.background", hex(p.panel));
    put("table.head.background", hex(p.panel));
    put("table.head.foreground", hex(p.text_muted));
    put("table.hover.background", hex(p.hover));
    put("table.active.background", hex(p.accent_soft));
    put("table.active.border", hex(p.accent));
    put("table.row.border", hex(p.border));

    put("sidebar.background", hex(p.panel));
    put("sidebar.foreground", hex(p.text));
    put("sidebar.border", hex(p.border));
    put("sidebar.accent.background", hex(p.hover));
    put("sidebar.accent.foreground", hex(p.text));
    put("sidebar.primary.background", hex(p.accent));
    put("sidebar.primary.foreground", hex(p.on_accent));

    put("tab.background", hex(p.panel.alpha(0.0)));
    put("tab.foreground", hex(p.text_muted));
    put("tab.active.background", hex(p.background));
    put("tab.active.foreground", hex(p.text));
    put("tab_bar.background", hex(p.panel));
    put("tab_bar.segmented.background", hex(p.raised));

    put("title_bar.background", hex(p.chrome));
    put("title_bar.border", hex(p.border));
    put("status_bar.background", hex(p.chrome));
    put("status_bar.border", hex(p.border));

    put("scrollbar.background", hex(p.background.alpha(0.0)));
    put("scrollbar.thumb.background", hex(p.text_faint.alpha(0.55)));
    put(
        "scrollbar.thumb.hover.background",
        hex(p.text_muted.alpha(0.8)),
    );
    put("skeleton.background", hex(p.raised));
    put("progress.bar.background", hex(p.accent));
    put("slider.background", hex(p.accent));
    put("slider.thumb.background", hex(p.text));
    put("switch.background", hex(p.border_strong));
    put(
        "switch.thumb.background",
        hex(if dark { p.text } else { p.raised }),
    );

    put("chart.1", hex(p.accent));
    put("chart.2", hex(p.info));
    put("chart.3", hex(p.success));
    put("chart.4", hex(p.danger));
    put("chart.5", hex(p.warning));
    put("chart.grid", hex(p.border));
    put("chart.bullish", hex(p.success));
    put("chart.bearish", hex(p.danger));

    for (name, color) in [
        ("red", p.danger),
        ("green", p.success),
        ("blue", p.info),
        ("yellow", p.warning),
        ("magenta", crate::color::oklch(0.70, 0.15, 320.0)),
        ("cyan", crate::color::oklch(0.78, 0.11, 200.0)),
    ] {
        put(&format!("base.{name}"), hex(color));
        put(
            &format!("base.{name}.light"),
            hex(color.with_l(color.l + 0.08)),
        );
    }

    json!({
        "$schema": "https://github.com/longbridge/gpui-component/raw/refs/heads/main/.theme-schema.json",
        "name": theme_name(p.mode),
        "author": "PhotoSquirrel",
        "themes": [{
            "is_default": dark,
            "name": theme_name(p.mode),
            "mode": if dark { "dark" } else { "light" },
            "font.family": UI_FONT_FAMILY,
            "font.size": DEFAULT_UI_FONT_SIZE,
            "mono_font.family": MONO_FONT_FAMILY,
            "mono_font.size": DEFAULT_MONO_FONT_SIZE,
            "radius": RADIUS_MD as usize,
            "radius.lg": RADIUS_LG as usize,
            "shadow": true,
            "colors": Value::Object(colors),
        }]
    })
}

/// The file text for `mode` (pretty JSON with a trailing newline).
pub fn theme_file(mode: Mode) -> String {
    let mut s =
        serde_json::to_string_pretty(&theme_set(&Palette::for_mode(mode))).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn bundled_theme_files_match_the_palettes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes");
        let update = std::env::var_os("UPDATE_THEMES").is_some();
        for (mode, file, bundled) in [
            (Mode::Dark, "graphite-dark.json", crate::THEME_DARK_JSON),
            (Mode::Light, "graphite-light.json", crate::THEME_LIGHT_JSON),
        ] {
            let generated = theme_file(mode);
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(file), &generated).unwrap();
            } else {
                assert_eq!(
                    bundled, generated,
                    "assets/themes/{file} is stale: run UPDATE_THEMES=1 cargo test -p ui-theme theme_json"
                );
            }
        }
    }
}
