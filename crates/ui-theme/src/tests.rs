//! Theme checks over both bundled themes. Colours are resolved through gpui-kit's own loader where gpui-kit
//! paints them, so the tests see what is drawn.

#![allow(clippy::unwrap_used)]

use std::rc::Rc;

use gpui_kit::Hsla;
use gpui_kit::component::{Theme, ThemeColor, ThemeSet};

use crate::color::Oklch;
use crate::contrast::{AA_SECONDARY, AA_TEXT, ratio};
use crate::palette::{ACORN, Colors, Mode, Palette};
use crate::{THEME_DARK_JSON, THEME_LIGHT_JSON};

/// gpui-kit's resolved colours for a bundled theme file.
fn kit_colors(json: &str) -> ThemeColor {
    let mut set: ThemeSet = serde_json::from_str(json).unwrap();
    let config = set.themes.remove(0);
    let mut theme = Theme::default();
    theme.apply_config(&Rc::new(config));
    theme.colors
}

fn both() -> [(Mode, Colors, ThemeColor); 2] {
    [
        (
            Mode::Dark,
            Palette::dark().resolve(),
            kit_colors(THEME_DARK_JSON),
        ),
        (
            Mode::Light,
            Palette::light().resolve(),
            kit_colors(THEME_LIGHT_JSON),
        ),
    ]
}

fn same(a: Hsla, b: Hsla) -> bool {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    (a.r - b.r).abs() < 0.003
        && (a.g - b.g).abs() < 0.003
        && (a.b - b.b).abs() < 0.003
        && (a.a - b.a).abs() < 0.003
}

#[test]
fn scenario_theming_accent_is_acorn() {
    let acorn = ACORN.to_hsla();
    let hue = Oklch::from_hsla(acorn);
    assert!(
        (50.0..=90.0).contains(&hue.h) && hue.c > 0.1,
        "acorn is a warm amber: {hue:?}"
    );
    for (mode, colors, kit) in both() {
        assert!(
            same(colors.selection, acorn),
            "{mode:?}: selection highlight"
        );
        assert!(same(kit.primary, acorn), "{mode:?}: primary actions");
        assert!(
            same(kit.sidebar_primary, acorn),
            "{mode:?}: sidebar primary"
        );
        assert!(
            same(kit.list_active_border, acorn),
            "{mode:?}: list selection"
        );
        // The focus ring is acorn too: the same colour on dark, its deeper step on light surfaces.
        let ring = Oklch::from_hsla(kit.ring);
        assert!(
            (50.0..=90.0).contains(&ring.h) && ring.c > 0.1,
            "{mode:?}: focus ring is acorn-hued: {ring:?}"
        );
    }
}

#[test]
fn scenario_theming_photo_surround_is_neutral() {
    for (mode, colors, _) in both() {
        for (name, color) in [("gallery", colors.gallery), ("loupe", colors.loupe)] {
            let o = Oklch::from_hsla(color);
            assert!(o.c < 0.02, "{mode:?} {name}: chroma {} >= 0.02", o.c);
        }
    }
}

#[test]
fn scenario_theming_text_contrast_meets_aa() {
    let mut failures = Vec::new();
    for (mode, c, kit) in both() {
        let surfaces = [
            ("chrome", c.chrome),
            ("background", c.background),
            ("panel", c.panel),
            ("raised", c.raised),
            ("overlay", c.overlay),
            ("hover", c.hover),
            ("pressed", c.pressed),
            ("gallery", c.gallery),
            ("loupe", c.loupe),
        ];
        let mut check = |name: String, fg: Hsla, bg: Hsla, min: f32| {
            let r = ratio(fg, bg);
            if r < min {
                failures.push(format!("{mode:?}: {name} is {r:.2}:1, needs {min}:1"));
            }
        };
        for (surface, bg) in surfaces {
            check(format!("text on {surface}"), c.text, bg, AA_TEXT);
            check(
                format!("text_muted on {surface}"),
                c.text_muted,
                bg,
                AA_SECONDARY,
            );
        }
        for (surface, bg) in [
            ("background", c.background),
            ("panel", c.panel),
            ("overlay", c.overlay),
            ("raised", c.raised),
        ] {
            check(
                format!("text_faint on {surface}"),
                c.text_faint,
                bg,
                AA_SECONDARY,
            );
            check(
                format!("accent_text on {surface}"),
                c.accent_text,
                bg,
                AA_SECONDARY,
            );
        }
        for (name, bg) in [
            ("accent", c.accent),
            ("accent_hover", c.accent_hover),
            ("accent_active", c.accent_active),
        ] {
            check(format!("on_accent on {name}"), c.on_accent, bg, AA_TEXT);
        }
        for (name, bg) in [
            ("danger", c.danger),
            ("warning", c.warning),
            ("success", c.success),
            ("info", c.info),
        ] {
            check(format!("on_semantic on {name}"), c.on_semantic, bg, AA_TEXT);
        }
        // Selected rows: text on the soft acorn tint over a panel.
        let soft = crate::contrast::over(c.accent_soft, c.panel);
        check("text on accent_soft".into(), c.text, soft, AA_TEXT);

        // What gpui-kit components paint.
        let kit_pairs: [(&str, Hsla, Hsla, f32); 12] = [
            (
                "kit foreground/background",
                kit.foreground,
                kit.background,
                AA_TEXT,
            ),
            (
                "kit foreground/sidebar",
                kit.foreground,
                kit.sidebar,
                AA_TEXT,
            ),
            (
                "kit foreground/popover",
                kit.popover_foreground,
                kit.popover,
                AA_TEXT,
            ),
            (
                "kit muted/background",
                kit.muted_foreground,
                kit.background,
                AA_SECONDARY,
            ),
            (
                "kit muted/muted",
                kit.muted_foreground,
                kit.muted,
                AA_SECONDARY,
            ),
            ("kit primary", kit.primary_foreground, kit.primary, AA_TEXT),
            (
                "kit secondary",
                kit.secondary_foreground,
                kit.secondary,
                AA_TEXT,
            ),
            ("kit danger", kit.danger_foreground, kit.danger, AA_TEXT),
            ("kit warning", kit.warning_foreground, kit.warning, AA_TEXT),
            ("kit success", kit.success_foreground, kit.success, AA_TEXT),
            ("kit status bar", kit.foreground, kit.status_bar, AA_TEXT),
            ("kit ring", kit.ring, kit.background, AA_SECONDARY),
        ];
        for (name, fg, bg, min) in kit_pairs {
            check(name.into(), fg, bg, min);
        }
    }
    assert!(
        failures.is_empty(),
        "contrast failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn bundled_themes_have_the_expected_names_and_fonts() {
    for (json, name, dark) in [
        (THEME_DARK_JSON, crate::DARK_THEME, true),
        (THEME_LIGHT_JSON, crate::LIGHT_THEME, false),
    ] {
        let set: ThemeSet = serde_json::from_str(json).unwrap();
        let theme = &set.themes[0];
        assert_eq!(theme.name.as_ref(), name);
        assert_eq!(theme.mode.is_dark(), dark);
        assert_eq!(
            theme.font_family.as_deref(),
            Some(crate::fonts::UI_FONT_FAMILY)
        );
        assert_eq!(
            theme.mono_font_family.as_deref(),
            Some(crate::fonts::MONO_FONT_FAMILY)
        );
    }
}
