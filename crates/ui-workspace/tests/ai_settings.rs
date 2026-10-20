//! The AI provider rows of the settings window (ai-insights task 3.7): presets, keys in the keychain, the
//! explicit consent that names the provider, and the connection test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, WindowOptions};
use ps_model::settings::ProviderId;
use ps_service::secrets::{MemoryBackend, Secrets, StoredIn};
use support::*;
use ui_workspace::review_source::SecretsGlobal;
use ui_workspace::settings::AppSettings;
use ui_workspace::settings_ai::{AiSettingsPanel, KeyState, consent_text};

fn panel(cx: &mut TestAppContext) -> (Entity<AiSettingsPanel>, VisualTestContext) {
    init_app(cx, Arc::new(FakeBackend::default()), None);
    cx.update(|cx| {
        cx.set_global(SecretsGlobal(
            Secrets::with_backend(Arc::new(MemoryBackend::default())).with_env(|_| None),
        ))
    });
    let mut entity = None;
    let handle = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let p = cx.new(|cx| AiSettingsPanel::new(window, cx));
                entity = Some(p.clone());
                p
            })
        })
        .unwrap()
        .0;
    let vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    (entity.unwrap(), vcx)
}

#[gpui_kit::test]
fn hosted_provider_needs_named_explicit_consent(cx: &mut TestAppContext) {
    let (panel, mut cx) = panel(cx);
    assert_eq!(
        cx.update(|_, cx| AppSettings::get(cx).ai.provider),
        ProviderId::LmStudio
    );
    assert!(
        !exists("setting-ai.hosted_consent", &mut cx),
        "no consent row for a local provider"
    );
    assert_eq!(
        panel.read_with(&cx, |p, _| p.key_state().clone()),
        KeyState::NotNeeded
    );

    click("ai-provider-4", &mut cx); // Anthropic
    let ai = cx.update(|_, cx| AppSettings::get(cx).ai.clone());
    assert_eq!(ai.provider, ProviderId::Anthropic);
    assert!(!ai.hosted_consent);
    assert!(exists("setting-ai.hosted_consent", &mut cx));
    assert!(consent_text(ProviderId::Anthropic).contains("Anthropic"));
    assert!(consent_text(ProviderId::Anthropic).contains("Upload previews"));
    assert_eq!(
        panel.read_with(&cx, |p, _| p.key_state().clone()),
        KeyState::Missing
    );

    // Turning the switch on asks first; only the explicit confirmation consents.
    cx.update(|_, cx| panel.update(cx, |p, cx| p.toggle_consent(true, cx)));
    assert!(panel.read_with(&cx, |p, _| p.is_confirming_consent()));
    assert!(!cx.update(|_, cx| AppSettings::get(cx).ai.hosted_consent));
    click("ai-consent-allow", &mut cx);
    assert!(cx.update(|_, cx| AppSettings::get(cx).ai.hosted_consent));

    // Consent is for one named provider: choosing another withdraws it.
    click("ai-provider-5", &mut cx); // Gemini
    let ai = cx.update(|_, cx| AppSettings::get(cx).ai.clone());
    assert_eq!(ai.provider, ProviderId::Gemini);
    assert!(!ai.hosted_consent);
}

#[gpui_kit::test]
fn api_key_goes_to_the_keychain(cx: &mut TestAppContext) {
    let (panel, mut cx) = panel(cx);
    click("ai-provider-3", &mut cx); // OpenAI
    let key = panel.read_with(&cx, |p, _| p.key_input().clone());
    cx.update(|window, cx| key.update(cx, |k, cx| k.set_value("sk-test-123", window, cx)));
    click("ai-key-save", &mut cx);
    wait_until(&mut cx, "the key stored", |cx| {
        let p = panel.clone();
        cx.update(|_, cx| p.read(cx).key_state() == &KeyState::Stored(StoredIn::Keychain))
    });
    assert_eq!(
        key.read_with(&cx, |k, _| k.value().to_string()),
        "",
        "the field is cleared"
    );
    let s = cx.update(|_, cx| AppSettings::get(cx).clone());
    let stored =
        ps_service::ai::api_key(&s.ai, &cx.update(|_, cx| SecretsGlobal::get(cx))).unwrap();
    assert_eq!(stored.value, "sk-test-123");
}

#[gpui_kit::test]
fn test_connection_reports_an_unreachable_server(cx: &mut TestAppContext) {
    let (panel, mut cx) = panel(cx);
    cx.update(|_, cx| {
        let _ = AppSettings::update(cx, |s| s.ai.base_url = "http://127.0.0.1:9/v1".into());
    });
    click("ai-test-connection", &mut cx);
    wait_until(&mut cx, "the test report", |cx| {
        let p = panel.clone();
        cx.update(|_, cx| p.read(cx).test_report().is_some())
    });
    let r = panel.read_with(&cx, |p, _| p.test_report().cloned().unwrap());
    assert!(!r.reachable);
    assert!(exists("ai-test-result", &mut cx));
}
