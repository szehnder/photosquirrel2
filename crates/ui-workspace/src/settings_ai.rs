//! The AI provider rows of the settings window (ai-insights task 3.7): provider with presets (LM Studio by
//! default, OpenAI-compatible local servers, OpenAI, Anthropic, Gemini), server address, API key in the OS
//! keychain (ADR-0031), the model picked from the provider's model list, the explicit hosted-provider consent
//! that names the provider and says previews will be uploaded (ADR-0021), and "Test connection" (which lists
//! models and never sends a photo).
//!
//! Consent is given to one named provider at one host (ADR-0047): choosing another provider or editing the server
//! address withdraws it, and the consent card names the destination host.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::settings::{ProviderId, Settings};
use ps_service::ai::TestReport;
use ps_service::secrets::StoredIn;
use ui_theme::Tokens;
use ui_theme::fonts::MONO_FONT_FAMILY;

use crate::assets::IconName;
use crate::review_source::SecretsGlobal;
use crate::settings::AppSettings;
use crate::views::review_kit::{Tone, action_button};
use crate::widgets::eyebrow;

/// A provider choice: a provider plus, for OpenAI-compatible servers, a preset address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub label: &'static str,
    pub detail: &'static str,
    pub provider: ProviderId,
    pub base_url: &'static str,
}

pub const PRESETS: [Preset; 6] = [
    Preset {
        label: "LM Studio",
        detail: "Local · default",
        provider: ProviderId::LmStudio,
        base_url: "http://localhost:1234/v1",
    },
    Preset {
        label: "Ollama",
        detail: "Local server",
        provider: ProviderId::OpenAiCompatible,
        base_url: "http://localhost:11434/v1",
    },
    Preset {
        label: "llama.cpp / other",
        detail: "Local server",
        provider: ProviderId::OpenAiCompatible,
        base_url: "http://localhost:8080/v1",
    },
    Preset {
        label: "OpenAI",
        detail: "Hosted",
        provider: ProviderId::OpenAi,
        base_url: "https://api.openai.com/v1",
    },
    Preset {
        label: "Anthropic",
        detail: "Hosted",
        provider: ProviderId::Anthropic,
        base_url: "https://api.anthropic.com/v1",
    },
    Preset {
        label: "Google Gemini",
        detail: "Hosted",
        provider: ProviderId::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
    },
];

/// The preset the settings describe (OpenAI-compatible servers by address; unknown addresses -> "other").
pub fn preset_index(s: &Settings) -> usize {
    PRESETS
        .iter()
        .position(|p| p.provider == s.ai.provider && p.base_url == s.ai.base_url)
        .or_else(|| match s.ai.provider {
            ProviderId::OpenAiCompatible => Some(2),
            p => PRESETS.iter().position(|x| x.provider == p),
        })
        .unwrap_or(0)
}

/// Apply a preset: provider and address; switching provider clears the model, and any change of provider or
/// address withdraws hosted consent (ADR-0047).
pub fn apply_preset(s: &mut Settings, ix: usize) {
    let p = PRESETS[ix.min(PRESETS.len() - 1)];
    if s.ai.provider != p.provider {
        s.ai.model.clear();
    }
    if s.ai.provider != p.provider || s.ai.base_url != p.base_url {
        s.ai.consent = None;
    }
    s.ai.provider = p.provider;
    s.ai.base_url = p.base_url.to_owned();
}

/// Set the server address; consent given for another host is withdrawn (ADR-0047).
pub fn set_base_url(s: &mut Settings, url: String) {
    s.ai.base_url = url;
    if !ps_service::ai::consent_covers(&s.ai) {
        s.ai.consent = None;
    }
}

/// The consent wording (ADR-0021, ADR-0047): names the provider and the host, and says images will be uploaded.
pub fn consent_text(provider: ProviderId, host: &str) -> String {
    format!(
        "Upload previews of your photos to {} at {host}. Each photo you queue is sent there as a 1024 px JPEG \
         preview to be described; originals never leave this computer. Nothing is sent until you start the AI \
         queue.",
        provider.label()
    )
}

/// Where the active provider's key is, for the key row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyState {
    NotNeeded,
    Missing,
    Stored(StoredIn),
}

pub struct AiSettingsPanel {
    url: Entity<InputState>,
    key: Entity<InputState>,
    key_state: KeyState,
    /// The consent switch was turned on and waits for the explicit confirmation.
    confirming_consent: bool,
    test: Option<TestReport>,
    testing: bool,
    message: Option<(bool, String)>,
    _subs: Vec<Subscription>,
}

impl AiSettingsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let settings = AppSettings::get(cx).clone();
        let url = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("http://localhost:1234/v1");
            s.set_value(settings.ai.base_url.clone(), window, cx);
            s
        });
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste an API key")
        });
        let subs = vec![
            cx.subscribe(&url, |this, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let text = state.read(cx).value().trim().to_owned();
                    if text != AppSettings::get(cx).ai.base_url {
                        if let Err(errs) = AppSettings::update(cx, |s| set_base_url(s, text)) {
                            this.message = errs.first().map(|e| (false, e.message.clone()));
                        } else {
                            this.message = None;
                        }
                        cx.notify();
                    }
                }
            }),
            cx.observe_global::<AppSettings>(|_, cx| cx.notify()),
        ];
        let mut this = Self {
            url,
            key,
            key_state: KeyState::NotNeeded,
            confirming_consent: false,
            test: None,
            testing: false,
            message: None,
            _subs: subs,
        };
        this.refresh_key_state(cx);
        this
    }

    pub fn key_state(&self) -> &KeyState {
        &self.key_state
    }

    pub fn is_confirming_consent(&self) -> bool {
        self.confirming_consent
    }

    pub fn test_report(&self) -> Option<&TestReport> {
        self.test.as_ref()
    }

    pub fn key_input(&self) -> &Entity<InputState> {
        &self.key
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|m| m.1.as_str())
    }

    fn refresh_key_state(&mut self, cx: &mut Context<'_, Self>) {
        let s = AppSettings::get(cx).clone();
        self.key_state = if s.ai.provider == ProviderId::LmStudio {
            KeyState::NotNeeded
        } else {
            match ps_service::ai::api_key(&s.ai, &SecretsGlobal::get(cx)) {
                Some(k) => KeyState::Stored(k.from),
                None => KeyState::Missing,
            }
        };
    }

    pub fn choose_preset(&mut self, ix: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let _ = AppSettings::update(cx, |s| apply_preset(s, ix));
        let url = AppSettings::get(cx).ai.base_url.clone();
        self.url.update(cx, |s, cx| s.set_value(url, window, cx));
        self.confirming_consent = false;
        self.test = None;
        self.message = None;
        self.refresh_key_state(cx);
        cx.notify();
    }

    /// The consent switch: turning it on asks for confirmation first; turning it off is immediate.
    pub fn toggle_consent(&mut self, on: bool, cx: &mut Context<'_, Self>) {
        if on {
            self.confirming_consent = true;
        } else {
            self.confirming_consent = false;
            let _ = AppSettings::update(cx, |s| s.ai.consent = None);
        }
        cx.notify();
    }

    pub fn confirm_consent(&mut self, cx: &mut Context<'_, Self>) {
        self.confirming_consent = false;
        let _ = AppSettings::update(cx, |s| s.ai.consent = ps_service::ai::consent_for(&s.ai));
        cx.notify();
    }

    pub fn save_key(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let key = self.key.read(cx).value().trim().to_owned();
        if key.is_empty() {
            return;
        }
        let s = AppSettings::get(cx).clone();
        let account = ps_service::ai::key_account(&s.ai, s.ai.provider);
        let secrets = SecretsGlobal::get(cx);
        let task = cx
            .background_executor()
            .spawn(async move { secrets.set_api_key(&account, &key) });
        self.key.update(cx, |k, cx| k.set_value("", window, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.message = Some(match r {
                    Ok(StoredIn::Keychain) => (true, "Key stored in the system keychain.".into()),
                    Ok(_) => (
                        true,
                        "No keychain is available: the key is kept for this session only.".into(),
                    ),
                    Err(e) => (false, format!("Could not store the key: {e}")),
                });
                this.refresh_key_state(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn remove_key(&mut self, cx: &mut Context<'_, Self>) {
        let s = AppSettings::get(cx).clone();
        let account = ps_service::ai::key_account(&s.ai, s.ai.provider);
        let secrets = SecretsGlobal::get(cx);
        let task = cx
            .background_executor()
            .spawn(async move { secrets.delete_api_key(&account) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.message = Some(match r {
                    Ok(()) => (true, "Key removed.".into()),
                    Err(e) => (false, format!("Could not remove the key: {e}")),
                });
                this.refresh_key_state(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn test_connection(&mut self, cx: &mut Context<'_, Self>) {
        if self.testing {
            return;
        }
        self.testing = true;
        let settings = AppSettings::get(cx).clone();
        let secrets = SecretsGlobal::get(cx);
        let task = cx
            .background_executor()
            .spawn(async move { ps_service::ai::test_connection(&settings, &secrets) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.testing = false;
                this.test = Some(r);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn set_model(&mut self, model: String, cx: &mut Context<'_, Self>) {
        let _ = AppSettings::update(cx, |s| s.ai.model = model);
        cx.notify();
    }
}

fn row(
    name: &'static str,
    description: impl Into<SharedString>,
    control: AnyElement,
    id: &'static str,
    cx: &App,
) -> AnyElement {
    let colors = ui_theme::colors(cx);
    h_flex()
        .id(id)
        .test_support()
        .w_full()
        .items_start()
        .justify_between()
        .gap(Tokens::space(6.0))
        .py(Tokens::space(3.0))
        .border_b_1()
        .border_color(colors.border)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(Tokens::space(0.5))
                .child(
                    div()
                        .text_color(colors.text)
                        .font_weight(FontWeight::MEDIUM)
                        .child(name),
                )
                .child(
                    div()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child(description.into()),
                ),
        )
        .child(
            v_flex()
                .flex_none()
                .items_end()
                .gap(Tokens::space(1.0))
                .child(control),
        )
        .into_any_element()
}

impl Render for AiSettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let s = AppSettings::get(cx).clone();
        let provider = s.ai.provider;
        let current = preset_index(&s);

        let presets = h_flex()
            .id("ai-provider-presets")
            .flex_wrap()
            .justify_end()
            .max_w(Tokens::size(420.0))
            .gap(Tokens::space(1.5))
            .children(PRESETS.iter().enumerate().map(|(ix, p)| {
                let on = ix == current;
                v_flex()
                    .id(SharedString::from(format!("ai-provider-{ix}")))
                    .test_support()
                    .role(Role::RadioButton)
                    .aria_label(p.label)
                    .w(Tokens::size(132.0))
                    .px(Tokens::space(2.5))
                    .py(Tokens::space(1.5))
                    .rounded(Tokens::RADIUS_MD)
                    .border_1()
                    .cursor_pointer()
                    .when(on, |d| d.bg(colors.accent_soft).border_color(colors.accent))
                    .when(!on, |d| {
                        d.bg(colors.raised)
                            .border_color(colors.border)
                            .hover(|d| d.bg(colors.hover))
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.choose_preset(ix, window, cx)),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .font_weight(FontWeight::MEDIUM)
                            .child(p.label),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_2xs())
                            .text_color(if p.provider.is_hosted() {
                                colors.warning
                            } else {
                                colors.text_faint
                            })
                            .child(p.detail),
                    )
            }));

        let url = div()
            .w(Tokens::settings_field_width())
            .child(Input::new(&self.url).aria_label("Server address"));

        let models: Vec<String> = self
            .test
            .as_ref()
            .map(|t| t.models.clone())
            .unwrap_or_default();
        let model_label = if s.ai.model.is_empty() {
            "Server's loaded model".to_owned()
        } else {
            s.ai.model.clone()
        };
        let weak = cx.weak_entity();
        let current_model = s.ai.model.clone();
        let model = Button::new("ai-model-picker")
            .label(model_label)
            .compact()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Server's loaded model")
                            .checked(current_model.is_empty())
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |v, cx| v.set_model(String::new(), cx));
                            }),
                    );
                }
                if models.is_empty() {
                    menu = menu.label("Test the connection to list the provider's models");
                }
                for m in &models {
                    let weak = weak.clone();
                    let name = m.clone();
                    menu = menu.item(
                        PopupMenuItem::new(m.clone())
                            .checked(*m == current_model)
                            .on_click(move |_, _, cx| {
                                let name = name.clone();
                                let _ = weak.update(cx, |v, cx| v.set_model(name, cx));
                            }),
                    );
                }
                menu
            });

        let key_status = match &self.key_state {
            KeyState::NotNeeded => None,
            KeyState::Missing => Some((colors.warning, "No key stored".to_owned())),
            KeyState::Stored(StoredIn::Keychain) => {
                Some((colors.success, "Stored in the system keychain".to_owned()))
            }
            KeyState::Stored(StoredIn::Session) => Some((
                colors.warning,
                "Kept for this session only (no keychain available)".to_owned(),
            )),
            KeyState::Stored(StoredIn::Environment) => Some((
                colors.info,
                format!(
                    "From ${} (environment)",
                    ps_service::secrets::env_var(provider).unwrap_or("API_KEY")
                ),
            )),
        };
        let key_row = (self.key_state != KeyState::NotNeeded).then(|| {
            let stored = matches!(
                self.key_state,
                KeyState::Stored(StoredIn::Keychain | StoredIn::Session)
            );
            let control = v_flex()
                .items_end()
                .gap(Tokens::space(1.0))
                .child(
                    h_flex()
                        .gap(Tokens::space(1.5))
                        .child(
                            div()
                                .w(Tokens::settings_field_width())
                                .child(Input::new(&self.key).aria_label("API key")),
                        )
                        .child(action_button(
                            "ai-key-save",
                            "Save",
                            Some(IconName::KeyRound),
                            None,
                            Tone::Normal,
                            cx,
                            |this, window, cx| this.save_key(window, cx),
                        ))
                        .when(stored, |d| {
                            d.child(action_button(
                                "ai-key-remove",
                                "Remove",
                                None,
                                None,
                                Tone::Quiet,
                                cx,
                                |this, _, cx| this.remove_key(cx),
                            ))
                        }),
                )
                .when_some(key_status.clone(), |d, (fg, t)| {
                    d.child(
                        h_flex()
                            .id("ai-key-status")
                            .test_support()
                            .gap(Tokens::space(1.0))
                            .text_size(Tokens::text_xs())
                            .text_color(fg)
                            .child(div().size(Tokens::dot_size()).rounded_full().bg(fg))
                            .child(t),
                    )
                })
                .into_any_element();
            row(
                "API key",
                "Kept in your operating system's keychain, never in settings files.",
                control,
                "setting-ai.api_key",
                cx,
            )
        });

        let hosted = ps_service::ai::is_hosted(&s.ai);
        let host = ps_service::ai::destination_host(&s.ai).unwrap_or_else(|| s.ai.base_url.clone());
        let consent = hosted.then(|| {
            let on = ps_service::ai::consent_covers(&s.ai);
            let confirming = self.confirming_consent;
            v_flex()
                .id("setting-ai.hosted_consent")
                .test_support()
                .my(Tokens::space(3.0))
                .p(Tokens::space(4.0))
                .gap(Tokens::space(2.5))
                .rounded(Tokens::RADIUS_LG)
                .border_1()
                .border_color(if on {
                    colors.warning
                } else {
                    colors.border_strong
                })
                .bg(ui_theme::Colors::tint(
                    colors.warning,
                    if on { 0.10 } else { 0.05 },
                ))
                .child(
                    h_flex()
                        .gap(Tokens::space(3.0))
                        .items_start()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(Tokens::icon_md())
                                .text_color(colors.warning),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(Tokens::space(1.0))
                                .child(
                                    div()
                                        .id("ai-consent-title")
                                        .test_support()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!(
                                            "Send photo previews to {} at {host}",
                                            provider.label()
                                        )),
                                )
                                .child(
                                    div()
                                        .id("ai-consent-text")
                                        .test_support()
                                        .text_size(Tokens::text_sm())
                                        .text_color(colors.text_muted)
                                        .child(consent_text(provider, &host)),
                                ),
                        )
                        .child(
                            Switch::new("ai-consent-switch")
                                .checked(on || confirming)
                                .accessibility_label(format!(
                                    "Allow uploads to {}",
                                    provider.label()
                                ))
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.toggle_consent(*checked, cx)
                                })),
                        ),
                )
                .when(confirming, |d| {
                    d.child(
                        h_flex()
                            .id("ai-consent-confirm")
                            .test_support()
                            .gap(Tokens::space(2.0))
                            .pl(Tokens::space(7.0))
                            .child(action_button(
                                "ai-consent-allow",
                                format!("Allow uploads to {}", provider.label()),
                                Some(IconName::Check),
                                None,
                                Tone::Primary,
                                cx,
                                |this, _, cx| this.confirm_consent(cx),
                            ))
                            .child(action_button(
                                "ai-consent-cancel",
                                "Cancel",
                                None,
                                None,
                                Tone::Quiet,
                                cx,
                                |this, _, cx| this.toggle_consent(false, cx),
                            )),
                    )
                })
                .when(on, |d| {
                    d.child(
                        div()
                            .pl(Tokens::space(7.0))
                            .text_size(Tokens::text_xs())
                            .text_color(colors.warning)
                            .child(format!(
                                "Allowed for {} at {host} only. Another provider or server address needs \
                                 your consent again.",
                                provider.label()
                            )),
                    )
                })
                .into_any_element()
        });

        let test_line = match (&self.test, self.testing) {
            (_, true) => Some((
                colors.text_muted,
                "Testing… (lists models, sends no photo)".to_owned(),
            )),
            (Some(r), _) if r.reachable => Some((
                colors.success,
                format!(
                    "Connected{} · {} model{}{}{}",
                    r.latency
                        .map(|l| format!(" in {} ms", l.as_millis()))
                        .unwrap_or_default(),
                    r.models.len(),
                    if r.models.len() == 1 { "" } else { "s" },
                    match r.model_found {
                        Some(false) => " · configured model not found",
                        _ => "",
                    },
                    match r.accepts_images {
                        Some(false) => " · model does not accept images",
                        Some(true) => " · accepts images",
                        None => "",
                    }
                ),
            )),
            (Some(r), _) => Some((
                colors.danger,
                format!(
                    "Not reachable: {}",
                    r.error.clone().unwrap_or_else(|| "no answer".into())
                ),
            )),
            _ => None,
        };
        let test = v_flex()
            .items_end()
            .gap(Tokens::space(1.0))
            .child(action_button(
                "ai-test-connection",
                "Test connection",
                Some(IconName::Plug),
                None,
                Tone::Normal,
                cx,
                |this, _, cx| this.test_connection(cx),
            ))
            .when_some(test_line, |d, (fg, t)| {
                d.child(
                    div()
                        .id("ai-test-result")
                        .test_support()
                        .max_w(Tokens::size(360.0))
                        .text_size(Tokens::text_xs())
                        .text_color(fg)
                        .child(t),
                )
            })
            .into_any_element();

        v_flex()
            .id("settings-ai-provider")
            .test_support()
            .w_full()
            .child(row(
                "Provider",
                if hosted {
                    "Hosted: previews are uploaded to the provider, only with your consent below."
                } else {
                    "Local: every photo stays on this computer."
                },
                presets.into_any_element(),
                "setting-ai.provider",
                cx,
            ))
            .child(row("Server address", "Where the provider listens.", url.into_any_element(), "setting-ai.base_url", cx))
            .children(key_row)
            .child(row(
                "Model",
                "Pick from the provider's model list (Test connection fetches it). Vision models only.",
                model.into_any_element(),
                "setting-ai.model",
                cx,
            ))
            // Right under the model it fetches; the hosted consent card follows.
            .child(row(
                "Test connection",
                "Checks the provider answers and has the model. No photo is sent.",
                test,
                "setting-ai.test",
                cx,
            ))
            .children(consent)
            .children(self.message.clone().map(|(ok, m)| {
                div()
                    .id("ai-settings-message")
                    .test_support()
                    .py(Tokens::space(2.0))
                    .font_family(MONO_FONT_FAMILY)
                    .text_size(Tokens::text_xs())
                    .text_color(if ok { colors.success } else { colors.danger })
                    .child(m)
            }))
            .child(div().pt(Tokens::space(4.0)).child(eyebrow("Queue", cx)))
    }
}
