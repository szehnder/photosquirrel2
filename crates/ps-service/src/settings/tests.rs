#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use ps_model::settings::{ProviderId, ThemeSetting};

use super::*;
use crate::secrets::{MemoryBackend, Secrets, default_account};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn scenario_settings_settings_persist_across_restart() {
    let data = temp();
    // The app resolves the store from its data dir, edits and saves...
    let app = SettingsStore::resolve(None, Some(data.path())).unwrap();
    app.update(|s| {
        s.appearance.theme = ThemeSetting::Light;
        s.scanning.preview_cache_gb = 12.5;
    })
    .unwrap();
    // ...a restart (new store, same directory) restores both, and the CLI resolves the same file.
    let restarted = SettingsStore::resolve(None, Some(data.path()))
        .unwrap()
        .load();
    assert!(restarted.recovered.is_none());
    assert_eq!(restarted.settings.appearance.theme, ThemeSetting::Light);
    assert_eq!(restarted.settings.scanning.preview_cache_gb, 12.5);
    let cli = SettingsStore::resolve(None, Some(data.path())).unwrap();
    assert_eq!(cli.path(), app.path());
    // Human-readable and versioned.
    let text = std::fs::read_to_string(app.path()).unwrap();
    assert!(text.contains("settings_version = 3"), "{text}");
    assert!(text.contains("theme = \"light\""), "{text}");
}

#[test]
fn scenario_settings_defaults_are_local() {
    let dir = temp();
    let loaded = SettingsStore::in_dir(dir.path()).load();
    let s = loaded.settings;
    assert!(loaded.recovered.is_none());
    assert_eq!(s.ai.provider, ProviderId::LmStudio);
    assert_eq!(s.ai.base_url, "http://localhost:1234/v1");
    assert!(s.ai.consent.is_none(), "hosted consent is off");
    let c = &s.scanning.classifier;
    assert_eq!(c.min_side_px, 200);
    assert_eq!(c.min_pixels, 120_000);
    assert_eq!(c.min_video_duration_ms, 2_000);
    assert_eq!(s.scanning.preview_cache_gb, 4.0);
    assert!(
        !dir.path().join(FILE_NAME).exists(),
        "reading writes nothing"
    );
}

#[test]
fn scenario_settings_no_secrets_in_settings_file() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    let secrets = Secrets::with_backend(Arc::new(MemoryBackend::default())).with_env(|_| None);
    let key = "sk-ant-THIS-IS-A-SECRET-0123456789";
    let account = default_account(ProviderId::Anthropic);
    secrets.set_api_key(&account, key).unwrap();
    store
        .update(|s| {
            s.ai.provider = ProviderId::Anthropic;
            s.ai.base_url = ProviderId::Anthropic.default_base_url().into();
            s.ai.consent = Some(ps_model::settings::HostedConsent {
                provider: ProviderId::Anthropic,
                host: "api.anthropic.com".into(),
            });
            s.ai.key_accounts.push(ps_model::settings::KeyAccount {
                provider: ProviderId::Anthropic,
                account: account.clone(),
            });
        })
        .unwrap();
    let text = std::fs::read_to_string(store.path()).unwrap();
    assert!(
        !text.contains(key),
        "settings file contains the key:\n{text}"
    );
    assert!(!text.contains("THIS-IS-A-SECRET"));
    assert!(text.contains(&account), "only the account name is stored");
    // The key is still retrievable from the keychain through the account name.
    let loaded = store.load().settings;
    let acct = &loaded.ai.key_accounts[0].account;
    assert_eq!(
        secrets.api_key(ProviderId::Anthropic, acct).unwrap().value,
        key
    );
}

#[test]
fn invalid_values_are_rejected_and_nothing_is_written() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    store.update(|s| s.scanning.preview_cache_gb = 8.0).unwrap();
    let before = std::fs::read_to_string(store.path()).unwrap();
    let err = store
        .update(|s| s.scanning.preview_cache_gb = -1.0)
        .unwrap_err();
    match err {
        SettingsError::Invalid(errors) => {
            assert_eq!(errors[0].key, "scanning.preview_cache_gb");
        }
        other => panic!("{other}"),
    }
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), before);

    let not_exec = dir.path().join("ffmpeg.txt");
    std::fs::write(&not_exec, "x").unwrap();
    let mut s = Settings::default();
    s.scanning.poster_frame_tool = Some(not_exec);
    assert_eq!(validate(&s)[0].key, "scanning.poster_frame_tool");
}

#[test]
fn windows_executables_are_recognised_by_extension() {
    for ok in [
        "ffmpeg.exe",
        "C:/tools/FFMPEG.EXE",
        "wrap.cmd",
        "wrap.bat",
        "x.com",
    ] {
        assert!(
            super::has_windows_executable_extension(Path::new(ok)),
            "{ok}"
        );
    }
    for no in ["ffmpeg", "ffmpeg.txt", "ffmpeg.exe.txt", "exe"] {
        assert!(
            !super::has_windows_executable_extension(Path::new(no)),
            "{no}"
        );
    }
}

#[test]
fn corrupt_file_is_backed_up_and_defaults_used() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    std::fs::write(store.path(), "settings_version = 1\n[appearance\ntheme = ").unwrap();
    let loaded = store.load();
    let recovery = loaded.recovered.expect("recovery notice");
    assert_eq!(loaded.settings, Settings::default());
    assert!(recovery.backup.exists());
    assert!(
        recovery
            .backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("settings.toml.bad-")
    );
    assert!(!store.path().exists(), "the bad file moved aside");
    // Told once: the next start loads defaults quietly.
    assert!(store.load().recovered.is_none());
}

#[test]
fn unknown_and_missing_keys_are_tolerated() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    std::fs::write(
        store.path(),
        "settings_version = 1\nfuture_key = 3\n[appearance]\ntheme = \"light\"\n",
    )
    .unwrap();
    let s = store.load().settings;
    assert_eq!(s.appearance.theme, ThemeSetting::Light);
    assert_eq!(s.scanning.preview_cache_gb, 4.0);
}

/// ADR-0047: consent is stored bound to a provider and a host and round-trips; a version-1 file's bare
/// `hosted_consent = true` is not taken as consent for anything.
#[test]
fn consent_is_bound_to_provider_and_host_and_v1_consent_is_dropped() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    std::fs::write(
        store.path(),
        "settings_version = 1\n[ai]\nprovider = \"gemini\"\n\
         base_url = \"https://generativelanguage.googleapis.com/v1beta\"\nhosted_consent = true\n",
    )
    .unwrap();
    let loaded = store.load();
    assert!(loaded.recovered.is_none(), "an old file is not corrupt");
    assert_eq!(loaded.settings.ai.provider, ProviderId::Gemini);
    assert!(
        loaded.settings.ai.consent.is_none(),
        "v1 consent is not carried over"
    );
    assert!(!crate::ai::consent_covers(&loaded.settings.ai));
    store
        .update(|s| s.ai.consent = crate::ai::consent_for(&s.ai))
        .unwrap();
    let text = std::fs::read_to_string(store.path()).unwrap();
    assert!(
        text.contains("host = \"generativelanguage.googleapis.com\""),
        "{text}"
    );
    let again = store.load().settings;
    assert!(crate::ai::consent_covers(&again.ai));
    let mut moved = again.ai.clone();
    moved.base_url = "https://other.example.com/v1beta".into();
    assert!(
        !crate::ai::consent_covers(&moved),
        "another host needs consent"
    );
}

/// Keys that were removed (`ai.auto_start_queue`, never implemented) still load: unknown keys are ignored.
#[test]
fn removed_keys_are_tolerated() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    std::fs::write(
        store.path(),
        "settings_version = 1\n[ai]\nauto_start_queue = true\nmax_concurrent_requests = 3\n",
    )
    .unwrap();
    let loaded = store.load();
    assert!(loaded.recovered.is_none());
    assert_eq!(loaded.settings.ai.max_concurrent_requests, 3);
}

/// ADR-0052: hidden folders are scanned by default, and a version-2 file's never-honoured `false` becomes
/// `true`; from version 3 on the user's choice stands.
#[test]
fn hidden_folders_default_on_and_v2_files_migrate() {
    let dir = temp();
    let store = SettingsStore::in_dir(dir.path());
    assert!(store.load().settings.scanning.include_hidden);
    std::fs::write(
        store.path(),
        "settings_version = 2\n[scanning]\ninclude_hidden = false\n",
    )
    .unwrap();
    assert!(store.load().settings.scanning.include_hidden);
    store.update(|s| s.scanning.include_hidden = false).unwrap();
    assert!(!store.load().settings.scanning.include_hidden);
}

/// The poster-frame tool is resolved again for each run (ADR-0114): a configured tool counts while it is a file,
/// and one installed after the options were built is found by the resolver.
#[test]
fn the_poster_tool_is_resolved_for_each_run() {
    let dir = temp();
    let tool = dir.path().join("ffmpeg-custom");
    let mut s = Settings::default();
    s.scanning.poster_frame_tool = Some(tool.clone());
    let mut opts = ScanOptions::default();
    apply_scan_settings(&s, &mut opts);
    // Not there yet: `ffmpeg` on PATH, if any, stands in.
    assert_eq!(opts.poster_tool, ps_media::poster::ffmpeg_on_path());
    assert_eq!(resolve_poster_tool(&s), ps_media::poster::ffmpeg_on_path());
    std::fs::write(&tool, "#!/bin/sh\n").unwrap();
    let resolve = opts.poster_tool_resolver.clone().unwrap();
    assert_eq!(resolve(), Some(tool.clone()));
    assert_eq!(resolve_poster_tool(&s), Some(tool.clone()));
    // A directory at the configured path is not a tool.
    let d = dir.path().join("ffmpeg-dir");
    std::fs::create_dir(&d).unwrap();
    s.scanning.poster_frame_tool = Some(d);
    assert_eq!(resolve_poster_tool(&s), ps_media::poster::ffmpeg_on_path());
}
