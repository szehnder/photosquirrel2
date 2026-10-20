#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use ps_model::settings::{ProviderId, ThemeSetting};
use ps_model::{CameraSignals, Dimensions, Format, MediaKind};

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
    assert!(text.contains("settings_version = 2"), "{text}");
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
    assert!(!s.ai.auto_start_queue, "the AI queue stays stopped");
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

/// A stand-in for `ps_media::classify` that applies the ADR-0015 size rule to stored probe fields.
fn size_rule(probe: &ProbeRecord, _: &Path, s: &ClassifierSettings) -> PhotoClass {
    let Some(d) = probe.display else {
        return PhotoClass::Junk;
    };
    if probe.camera.is_camera_exif() {
        return PhotoClass::Photo;
    }
    if d.short_edge() < s.min_side_px || d.pixels() < s.min_pixels {
        PhotoClass::Junk
    } else {
        PhotoClass::LikelyPhoto
    }
}

fn stored(name: &str, w: u32, h: u32, settings: &ClassifierSettings) -> StoredClassification {
    let mut probe = ProbeRecord::empty(50_000);
    probe.format = Some(Format::Jpeg);
    probe.stored = Some(Dimensions {
        width: w,
        height: h,
    });
    probe.display = probe.stored;
    probe.camera = CameraSignals::default();
    assert_eq!(probe.kind(), Some(MediaKind::Image));
    let path = PathBuf::from(format!("/source/{name}.jpg"));
    let class = size_rule(&probe, &path, settings);
    StoredClassification {
        path,
        probe,
        class,
        classifier_hash: settings.stable_hash(),
    }
}

#[test]
fn scenario_settings_threshold_change_reclassifies() {
    let old = ClassifierSettings::default();
    let mut rows = vec![
        stored("icon", 64, 64, &old),
        stored("small-150", 150, 1000, &old),
        stored("small-180", 1200, 180, &old),
        stored("photo", 4000, 3000, &old),
    ];
    assert_eq!(rows[1].class, PhotoClass::Junk);
    assert_eq!(rows[2].class, PhotoClass::Junk);

    // The user lowers the minimum side to 100 px (and the pixel floor with it, so the side rule decides).
    let mut new = old.clone();
    new.min_side_px = 100;
    new.min_pixels = 10_000;
    assert_ne!(new.stable_hash(), old.stable_hash());
    // `classify` gets stored records and paths that do not exist: there is nothing to read.
    let changed = reclassify_stale(&mut rows, &new, size_rule);
    let names: Vec<_> = changed
        .iter()
        .map(|c| c.path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["small-150", "small-180"]);
    assert!(
        changed
            .iter()
            .all(|c| c.from == PhotoClass::Junk && c.to == PhotoClass::LikelyPhoto)
    );
    assert_eq!(rows[0].class, PhotoClass::Junk, "64 px stays junk");
    assert!(rows.iter().all(|r| r.classifier_hash == new.stable_hash()));
    assert!(
        !rows[1].path.exists(),
        "no file content was available to read"
    );

    // Unchanged settings: nothing to do on the next rescan.
    assert!(reclassify_stale(&mut rows, &new, |_, _, _| unreachable!()).is_empty());
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
