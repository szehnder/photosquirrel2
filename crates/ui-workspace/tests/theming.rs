//! Headless tests for spec `theming` that need the running shell.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::TestAppContext;
use gpui_kit::component::Theme;
use ps_model::settings::{MotionSetting, ThemeSetting};
use support::*;
use ui_theme::{DARK_THEME, LIGHT_THEME, Motion, Palette};
use ui_workspace::inspector::{InspectedItem, InspectorView};
use ui_workspace::settings::AppSettings;
use ui_workspace::widgets::ValueStyle;

#[gpui_kit::test]
fn scenario_theming_dark_theme_by_default(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::default()), Some(&dirs));
    let (_shell, mut cx) = open(cx);
    cx.update(|_, cx| {
        assert_eq!(AppSettings::get(cx).appearance.theme, ThemeSetting::Dark);
        assert!(Theme::global(cx).mode.is_dark());
        assert_eq!(ui_theme::active_theme_name(cx).as_ref(), DARK_THEME);
        assert_eq!(*ui_theme::colors(cx), Palette::dark().resolve());
    });
}

#[gpui_kit::test]
fn scenario_theming_switch_to_light_live(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    let frames_before = cx.update(|_, cx| shell.read(cx).frames_drawn());
    // Select the light theme in the settings window's Appearance section.
    cx.simulate_keystrokes("ctrl-,");
    let settings = cx.update(|_, cx| shell.read(cx).settings_window().cloned().unwrap());
    cx.update(|_, cx| {
        settings.update(cx, |s, cx| {
            s.select_section(ui_workspace::settings_window::Section::Appearance, cx)
        })
    });
    cx.run_until_parked();
    click("setting-choice-appearance.theme-1", &mut cx);
    cx.update(|_, cx| {
        assert_eq!(AppSettings::get(cx).appearance.theme, ThemeSetting::Light);
        assert!(!Theme::global(cx).mode.is_dark(), "gpui-kit theme switched");
        assert_eq!(ui_theme::active_theme_name(cx).as_ref(), LIGHT_THEME);
        assert_eq!(*ui_theme::colors(cx), Palette::light().resolve());
    });
    // Every open view repainted: the shell rendered again, without a restart.
    cx.update(|window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.render_frame(cx);
    });
    let frames_after = cx.update(|_, cx| shell.read(cx).frames_drawn());
    assert!(frames_after > frames_before);
    assert!(
        exists("settings-window", &mut cx),
        "the settings window stays open"
    );
    // Saved too.
    wait_until(&mut cx, "the settings write", |_| {
        std::fs::read_to_string(dirs.config().join("settings.toml"))
            .is_ok_and(|t| t.contains("theme = \"light\""))
    });
}

#[gpui_kit::test]
fn scenario_theming_reduced_motion_disables_animation(cx: &mut TestAppContext) {
    init_app(cx, Arc::new(FakeBackend::default()), None);
    let (_shell, mut cx) = open(cx);
    cx.update(|_, cx| {
        AppSettings::update(cx, |s| s.appearance.motion = MotionSetting::Full).unwrap();
        assert!(Motion::ALL.iter().all(|m| m.duration(cx) > Duration::ZERO));
        AppSettings::update(cx, |s| s.appearance.motion = MotionSetting::Reduce).unwrap();
        assert!(cx.reduce_motion(), "gpui and gpui-kit animations are off");
        for m in Motion::ALL {
            assert_eq!(m.duration(cx), Duration::ZERO, "{m:?}");
        }
        // "Match system" follows the OS preference.
        AppSettings::update(cx, |s| s.appearance.motion = MotionSetting::System).unwrap();
        AppSettings::set_os_reduce_motion(true, cx);
        assert!(Motion::ALL.iter().all(|m| m.duration(cx).is_zero()));
    });
}

#[gpui_kit::test]
fn scenario_theming_metadata_uses_mono_font(cx: &mut TestAppContext) {
    // A library with photos: over the welcome view of an empty one the inspector is hidden.
    init_app(cx, Arc::new(FakeBackend::small_corpus()), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    let hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let inspector = cx.update(|_, cx| shell.read(cx).inspector().clone());
    cx.update(|_, cx| {
        inspector.update(cx, |i: &mut InspectorView, cx| {
            i.set_item(
                Some(InspectedItem {
                    name: "IMG_2041.HEIC".into(),
                    path: "/Volumes/NAS/Photos/2023/IMG_2041.HEIC".into(),
                    content_hash: hash.into(),
                    dimensions: Some((4032, 3024)),
                    camera: Some("iPhone 14 Pro".into()),
                    captured: Some("2023-08-14 18:22:05".into()),
                }),
                cx,
            )
        })
    });
    assert!(
        exists("inspector-hash", &mut cx),
        "the inspector renders the hash"
    );
    let item = cx.update(|_, cx| inspector.read(cx).item().cloned().unwrap());
    let (_, _, value, style) = item
        .rows()
        .into_iter()
        .find(|r| r.0 == "inspector-hash")
        .unwrap();
    assert_eq!(value, hash);
    assert_eq!(style, ValueStyle::Mono);
    assert_eq!(style.font_family(), "JetBrains Mono");
    // The family resolves to the bundled font (real text system, bundled bytes only).
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    text_system.add_fonts(ui_theme::fonts::font_data()).unwrap();
    let id = text_system
        .font_id(&gpui_kit::font(style.font_family()))
        .unwrap();
    assert!(text_system.font_metrics(id).units_per_em > 0);
}
