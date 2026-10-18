//! Headless tests for spec `settings` that need the settings window or the running shell.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use support::*;
use ui_workspace::settings::AppSettings;

#[gpui_kit::test]
fn scenario_settings_search_filters_settings(cx: &mut TestAppContext) {
    init_app(cx, Arc::new(FakeBackend::default()), None);
    let (shell, mut cx) = open(cx);
    cx.simulate_keystrokes("ctrl-,");
    assert!(exists("settings-window", &mut cx));
    for section in ["general", "appearance", "scanning", "ai", "export"] {
        assert!(
            exists(&format!("settings-section-{section}"), &mut cx),
            "{section}"
        );
    }
    // Focus is in the search field; type.
    cx.simulate_input("cache");
    cx.run_until_parked();
    let settings = cx.update(|_, cx| shell.read(cx).settings_window().cloned().unwrap());
    let names = cx.update(|_, cx| settings.read(cx).visible_names(cx));
    assert!(!names.is_empty());
    assert!(
        names.iter().all(|n| n.to_lowercase().contains("cache")),
        "{names:?}"
    );
    let keys = cx.update(|_, cx| settings.read(cx).visible_keys(cx));
    assert!(exists("setting-scanning.preview_cache_gb", &mut cx));
    assert!(!exists("setting-appearance.theme", &mut cx));
    for key in keys {
        assert!(exists(&format!("setting-{key}"), &mut cx));
    }
}

#[gpui_kit::test]
fn scenario_settings_invalid_value_rejected(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::default()), Some(&dirs));
    let (shell, mut cx) = open(cx);
    // A saved baseline.
    cx.update(|_, cx| AppSettings::update(cx, |s| s.scanning.preview_cache_gb = 6.0).unwrap());
    let path = dirs.config().join("settings.toml");
    wait_until(&mut cx, "the baseline write", |_| path.exists());
    let before = std::fs::read_to_string(&path).unwrap();

    cx.simulate_keystrokes("ctrl-,");
    let settings = cx.update(|_, cx| shell.read(cx).settings_window().cloned().unwrap());
    cx.update(|_, cx| {
        settings.update(cx, |s, cx| {
            s.select_section(ui_workspace::settings_window::Section::Scanning, cx)
        })
    });
    cx.run_until_parked();
    let field = cx.update(|_, cx| {
        settings
            .read(cx)
            .field_state("scanning.preview_cache_gb")
            .cloned()
            .unwrap()
    });
    // The user clears the field and types -1.
    cx.update(|window, cx| {
        field.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.input("-1", cx));
    cx.run_until_parked();
    assert!(
        exists("setting-error-scanning.preview_cache_gb", &mut cx),
        "the field shows an error"
    );
    let message = cx.update(|_, cx| {
        settings
            .read(cx)
            .error("scanning.preview_cache_gb")
            .map(str::to_owned)
    });
    assert!(message.unwrap().contains("larger than 0"));
    cx.update(|_, cx| assert_eq!(AppSettings::get(cx).scanning.preview_cache_gb, 6.0));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "saved settings unchanged"
    );

    // A valid value clears the error and saves.
    cx.update(|window, cx| field.update(cx, |s, cx| s.set_value("", window, cx)));
    cx.update(|window, cx| window.input("12", cx));
    cx.run_until_parked();
    assert!(!exists("setting-error-scanning.preview_cache_gb", &mut cx));
    cx.update(|_, cx| assert_eq!(AppSettings::get(cx).scanning.preview_cache_gb, 12.0));
}

#[gpui_kit::test]
fn scenario_settings_corrupt_settings_fall_back(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    std::fs::create_dir_all(dirs.config()).unwrap();
    std::fs::write(
        dirs.config().join("settings.toml"),
        "settings_version = 1\n[appearance\ntheme = ",
    )
    .unwrap();
    init_app(cx, Arc::new(FakeBackend::default()), Some(&dirs));
    let (shell, mut cx) = open(cx);
    cx.update(|_, cx| {
        assert_eq!(
            *AppSettings::get(cx),
            ps_model::settings::Settings::default()
        );
    });
    let backups: Vec<_> = std::fs::read_dir(dirs.config())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("settings.toml.bad-")
        })
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "a backup of the bad file exists beside it"
    );
    assert!(exists("toast", &mut cx), "a notice is shown");
    let toast = cx.update(|_, cx| shell.read(cx).toast().cloned().unwrap());
    assert_eq!(toast.title.as_ref(), "Settings were reset");
    // Told once: it is dismissed and a second window does not repeat it.
    click("toast-dismiss", &mut cx);
    assert!(!exists("toast", &mut cx));
    cx.update(|_, cx| assert!(AppSettings::recovery(cx).is_none()));
}
