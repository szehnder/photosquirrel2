//! The settings window: an overlay over the main window with sections (General, Appearance, Scanning, AI,
//! Export) and a search field that filters settings by name. Values apply as they change (no Save button);
//! invalid input shows an error under the field and is not saved.
//!
//! Settings are a table ([`definitions`]) so search, rendering and tests share one source of truth. A later
//! change adds rows by extending the table (e.g. `ai-insights` adds API key entry under AI).

use std::collections::HashMap;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::settings::{ExportProfile, MotionSetting, Settings, ThemeSetting};
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::keymap;
use crate::settings::AppSettings;
use crate::widgets::{eyebrow, kbd, segmented};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    General,
    Appearance,
    Scanning,
    Ai,
    Export,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::General,
        Section::Appearance,
        Section::Scanning,
        Section::Ai,
        Section::Export,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Scanning => "Scanning",
            Section::Ai => "AI",
            Section::Export => "Export",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::SlidersHorizontal,
            Section::Appearance => IconName::Palette,
            Section::Scanning => IconName::ScanSearch,
            Section::Ai => IconName::Sparkles,
            Section::Export => IconName::FolderOutput,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Appearance => "appearance",
            Section::Scanning => "scanning",
            Section::Ai => "ai",
            Section::Export => "export",
        }
    }
}

/// How a setting is edited.
pub enum Control {
    Toggle {
        get: fn(&Settings) -> bool,
        set: fn(&mut Settings, bool),
    },
    Choice {
        options: &'static [&'static str],
        get: fn(&Settings) -> usize,
        set: fn(&mut Settings, usize),
    },
    /// A text field: `get` formats the current value, `parse` writes the typed text or explains why not.
    Field {
        get: fn(&Settings) -> String,
        parse: fn(&str, &mut Settings) -> Result<(), String>,
        unit: Option<&'static str>,
        placeholder: &'static str,
    },
}

pub struct SettingDef {
    /// Dotted path, matching `FieldError::key` (`scanning.preview_cache_gb`).
    pub key: &'static str,
    pub section: Section,
    pub name: &'static str,
    pub description: &'static str,
    pub control: Control,
}

fn parse_f64(text: &str) -> Result<f64, String> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| "Enter a number.".to_owned())
}

fn parse_u64(text: &str) -> Result<u64, String> {
    let t = text.trim().replace(['_', ','], "");
    if t.starts_with('-') {
        return Err("Enter a positive whole number.".into());
    }
    t.parse::<u64>()
        .map_err(|_| "Enter a whole number.".to_owned())
}

const THEMES: &[&str] = &["Graphite dark", "Graphite light", "Match system"];
const MOTIONS: &[&str] = &["Match system", "Reduce", "Full"];
const PROFILES: &[&str] = &["Apple Photos", "Google Photos", "Lightroom"];

fn index_of<T: PartialEq + Copy>(all: &[T], v: T) -> usize {
    all.iter().position(|x| *x == v).unwrap_or(0)
}

/// Every setting, in display order.
pub fn definitions() -> Vec<SettingDef> {
    use Section::*;
    vec![
        SettingDef {
            key: "general.offer_resume",
            section: General,
            name: "Offer to resume interrupted scans",
            description: "If a scan was stopped by a crash or power loss, offer to pick up where it left off.",
            control: Control::Toggle {
                get: |s| s.general.offer_resume,
                set: |s, v| s.general.offer_resume = v,
            },
        },
        SettingDef {
            key: "general.pause_on_battery",
            section: General,
            name: "Pause scans on battery",
            description: "Hashing and thumbnails are heavy; wait for power before continuing.",
            control: Control::Toggle {
                get: |s| s.general.pause_on_battery,
                set: |s, v| s.general.pause_on_battery = v,
            },
        },
        SettingDef {
            key: "appearance.theme",
            section: Appearance,
            name: "Theme",
            description: "Graphite dark keeps the interface out of the way of your photos.",
            control: Control::Choice {
                options: THEMES,
                get: |s| index_of(&ThemeSetting::ALL, s.appearance.theme),
                set: |s, i| s.appearance.theme = ThemeSetting::ALL[i],
            },
        },
        SettingDef {
            key: "appearance.motion",
            section: Appearance,
            name: "Motion",
            description: "Reduce makes every transition instant.",
            control: Control::Choice {
                options: MOTIONS,
                get: |s| index_of(&MotionSetting::ALL, s.appearance.motion),
                set: |s, i| s.appearance.motion = MotionSetting::ALL[i],
            },
        },
        SettingDef {
            key: "appearance.ui_font_size",
            section: Appearance,
            name: "Text size",
            description: "Scales the whole interface. Also Ctrl+= and Ctrl+-.",
            control: Control::Field {
                get: |s| format!("{}", s.appearance.ui_font_size),
                parse: |t, s| {
                    s.appearance.ui_font_size = parse_f64(t)? as f32;
                    Ok(())
                },
                unit: Some("px"),
                placeholder: "13",
            },
        },
        SettingDef {
            key: "scanning.classifier.min_side_px",
            section: Scanning,
            name: "Minimum image side",
            description: "Smaller images without camera data count as icons or graphics, not photos.",
            control: Control::Field {
                get: |s| s.scanning.classifier.min_side_px.to_string(),
                parse: |t, s| {
                    s.scanning.classifier.min_side_px = u32::try_from(parse_u64(t)?)
                        .map_err(|_| "That number is too large.".to_owned())?;
                    Ok(())
                },
                unit: Some("px"),
                placeholder: "200",
            },
        },
        SettingDef {
            key: "scanning.classifier.min_pixels",
            section: Scanning,
            name: "Minimum image pixels",
            description: "Width × height below this, without camera data, is not a photo.",
            control: Control::Field {
                get: |s| s.scanning.classifier.min_pixels.to_string(),
                parse: |t, s| {
                    s.scanning.classifier.min_pixels = parse_u64(t)?;
                    Ok(())
                },
                unit: Some("px"),
                placeholder: "120000",
            },
        },
        SettingDef {
            key: "scanning.classifier.min_video_duration_ms",
            section: Scanning,
            name: "Minimum video length",
            description: "Shorter clips are set aside (Live Photo motion stays with its still).",
            control: Control::Field {
                get: |s| {
                    format!(
                        "{}",
                        s.scanning.classifier.min_video_duration_ms as f64 / 1000.0
                    )
                },
                parse: |t, s| {
                    let secs = parse_f64(t)?;
                    if secs < 0.0 {
                        return Err("Enter 0 or more seconds.".into());
                    }
                    s.scanning.classifier.min_video_duration_ms = (secs * 1000.0).round() as u64;
                    Ok(())
                },
                unit: Some("s"),
                placeholder: "2",
            },
        },
        SettingDef {
            key: "scanning.preview_cache_gb",
            section: Scanning,
            name: "Preview cache size",
            description: "Large previews for the loupe, kept on disk up to this size.",
            control: Control::Field {
                get: |s| format!("{}", s.scanning.preview_cache_gb),
                parse: |t, s| {
                    s.scanning.preview_cache_gb = parse_f64(t)?;
                    Ok(())
                },
                unit: Some("GB"),
                placeholder: "4",
            },
        },
        SettingDef {
            key: "scanning.poster_frame_tool",
            section: Scanning,
            name: "Poster-frame tool",
            description: "Path to ffmpeg for video thumbnails. Empty: look it up on PATH.",
            control: Control::Field {
                get: |s| {
                    s.scanning
                        .poster_frame_tool
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                },
                parse: |t, s| {
                    let t = t.trim();
                    s.scanning.poster_frame_tool = (!t.is_empty()).then(|| t.into());
                    Ok(())
                },
                unit: None,
                placeholder: "/usr/bin/ffmpeg",
            },
        },
        SettingDef {
            key: "scanning.classifier.extra_cache_patterns",
            section: Scanning,
            name: "Extra cache folders",
            description: "Folder names to treat as thumbnail caches, separated by commas.",
            control: Control::Field {
                get: |s| s.scanning.classifier.extra_cache_patterns.join(", "),
                parse: |t, s| {
                    s.scanning.classifier.extra_cache_patterns = t
                        .split(',')
                        .map(|p| p.trim().to_owned())
                        .filter(|p| !p.is_empty())
                        .collect();
                    Ok(())
                },
                unit: None,
                placeholder: ".thumbs, previews",
            },
        },
        SettingDef {
            key: "scanning.include_hidden",
            section: Scanning,
            name: "Include hidden folders",
            description: "Also look inside folders whose names start with a dot.",
            control: Control::Toggle {
                get: |s| s.scanning.include_hidden,
                set: |s, v| s.scanning.include_hidden = v,
            },
        },
        SettingDef {
            key: "ai.max_concurrent_requests",
            section: Ai,
            name: "Concurrent requests",
            description: "How many photos are described at once.",
            control: Control::Field {
                get: |s| s.ai.max_concurrent_requests.to_string(),
                parse: |t, s| {
                    s.ai.max_concurrent_requests = u32::try_from(parse_u64(t)?)
                        .map_err(|_| "That number is too large.".to_owned())?;
                    Ok(())
                },
                unit: None,
                placeholder: "2",
            },
        },
        SettingDef {
            key: "ai.request_timeout_s",
            section: Ai,
            name: "Request timeout",
            description: "How long one photo may take. Local vision models on a laptop can need a minute or more; \
                          empty uses 300 s for local providers and 120 s for hosted ones.",
            control: Control::Field {
                get: |s| {
                    s.ai.request_timeout_s
                        .map(|t| t.to_string())
                        .unwrap_or_default()
                },
                parse: |t, s| {
                    let t = t.trim();
                    s.ai.request_timeout_s = if t.is_empty() {
                        None
                    } else {
                        Some(
                            u32::try_from(parse_u64(t)?)
                                .map_err(|_| "That number is too large.".to_owned())?,
                        )
                    };
                    Ok(())
                },
                unit: Some("s"),
                placeholder: "300",
            },
        },
        SettingDef {
            key: "export.profile",
            section: Export,
            name: "Default library profile",
            description: "Folder layout and sidecars tuned for where the library goes next.",
            control: Control::Choice {
                options: PROFILES,
                get: |s| index_of(&ExportProfile::ALL, s.export.profile),
                set: |s, i| s.export.profile = ExportProfile::ALL[i],
            },
        },
        SettingDef {
            key: "export.sidecars",
            section: Export,
            name: "Write XMP sidecars",
            description: "Keywords and descriptions travel with each exported photo (profiles that use them; \
                          the Export view can change it per export).",
            control: Control::Toggle {
                get: |s| s.export.sidecars,
                set: |s, v| s.export.sidecars = v,
            },
        },
    ]
}

/// Settings whose **name** contains `query` (case-insensitive), in display order.
pub fn filter<'a>(defs: &'a [SettingDef], query: &str) -> Vec<&'a SettingDef> {
    let q = query.trim().to_lowercase();
    defs.iter()
        .filter(|d| q.is_empty() || d.name.to_lowercase().contains(&q))
        .collect()
}

pub enum SettingsWindowEvent {
    Close,
}

/// The settings overlay.
pub struct SettingsWindow {
    defs: Vec<SettingDef>,
    section: Section,
    search: Entity<InputState>,
    fields: HashMap<&'static str, Entity<InputState>>,
    errors: HashMap<&'static str, String>,
    focus: FocusHandle,
    /// Provider, address, key, model, consent and connection test (`crate::settings_ai`).
    ai_panel: Entity<crate::settings_ai::AiSettingsPanel>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsWindowEvent> for SettingsWindow {}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let defs = definitions();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let mut subscriptions = vec![cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        })];
        let mut fields = HashMap::new();
        let current = AppSettings::get(cx).clone();
        for def in &defs {
            if let Control::Field {
                get, placeholder, ..
            } = &def.control
            {
                let value = get(&current);
                let placeholder = *placeholder;
                let state = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder(placeholder);
                    s.set_value(value, window, cx);
                    s
                });
                let key = def.key;
                subscriptions.push(
                    cx.subscribe(&state, move |this, state, e: &InputEvent, cx| {
                        if matches!(e, InputEvent::Change) {
                            let text = state.read(cx).value().to_string();
                            this.field_changed(key, &text, cx);
                        }
                    }),
                );
                fields.insert(def.key, state);
            }
        }
        subscriptions.push(cx.observe_global::<AppSettings>(|_, cx| cx.notify()));
        Self {
            defs,
            // Dev-only: scripted screenshots open a section directly (agents' windows get one frame).
            section: std::env::var("PHOTOSQUIRREL_SETTINGS_SECTION")
                .ok()
                .and_then(|k| Section::ALL.into_iter().find(|s| s.key() == k))
                .unwrap_or(Section::General),
            search,
            fields,
            errors: HashMap::new(),
            focus: cx.focus_handle(),
            ai_panel: cx.new(|cx| crate::settings_ai::AiSettingsPanel::new(window, cx)),
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn search_state(&self) -> &Entity<InputState> {
        &self.search
    }

    pub fn field_state(&self, key: &str) -> Option<&Entity<InputState>> {
        self.fields.get(key)
    }

    pub fn section(&self) -> Section {
        self.section
    }

    pub fn select_section(&mut self, section: Section, cx: &mut Context<'_, Self>) {
        self.section = section;
        cx.notify();
    }

    /// The error under a field, if its last input was rejected.
    pub fn error(&self, key: &str) -> Option<&str> {
        self.errors.get(key).map(String::as_str)
    }

    /// Keys of the settings currently shown.
    pub fn visible_keys(&self, cx: &App) -> Vec<&'static str> {
        self.visible(cx).into_iter().map(|d| d.key).collect()
    }

    /// Names of the settings currently shown.
    pub fn visible_names(&self, cx: &App) -> Vec<&'static str> {
        self.visible(cx).into_iter().map(|d| d.name).collect()
    }

    fn visible(&self, cx: &App) -> Vec<&SettingDef> {
        let query = self.search.read(cx).value();
        if query.trim().is_empty() {
            self.defs
                .iter()
                .filter(|d| d.section == self.section)
                .collect()
        } else {
            filter(&self.defs, &query)
        }
    }

    fn field_changed(&mut self, key: &'static str, text: &str, cx: &mut Context<'_, Self>) {
        let Some(def) = self.defs.iter().find(|d| d.key == key) else {
            return;
        };
        let Control::Field { parse, .. } = &def.control else {
            return;
        };
        let mut candidate = AppSettings::get(cx).clone();
        let result = parse(text, &mut candidate).and_then(|()| {
            AppSettings::update(cx, |s| *s = candidate).map_err(|errors| {
                errors
                    .iter()
                    .find(|e| e.key == key)
                    .or(errors.first())
                    .map(|e| e.message.clone())
                    .unwrap_or_default()
            })
        });
        match result {
            Ok(()) => {
                self.errors.remove(key);
            }
            Err(message) => {
                self.errors.insert(key, message);
            }
        }
        cx.notify();
    }

    fn render_control(&self, def: &SettingDef, cx: &mut Context<'_, Self>) -> AnyElement {
        let settings = AppSettings::get(cx);
        let colors = *ui_theme::colors(cx);
        match &def.control {
            Control::Toggle { get, set } => {
                let set = *set;
                Switch::new(SharedString::from(format!("setting-switch-{}", def.key)))
                    .checked(get(settings))
                    .accessibility_label(def.name)
                    .on_click(move |checked, _, cx| {
                        let checked = *checked;
                        let _ = AppSettings::update(cx, |s| set(s, checked));
                    })
                    .into_any_element()
            }
            Control::Choice { options, get, set } => {
                let set = *set;
                segmented(
                    format!("setting-choice-{}", def.key),
                    options,
                    get(settings),
                    move |ix, _, cx| {
                        let _ = AppSettings::update(cx, |s| set(s, ix));
                    },
                    cx,
                )
                .into_any_element()
            }
            Control::Field { unit, .. } => {
                let Some(state) = self.fields.get(def.key) else {
                    return div().into_any_element();
                };
                let invalid = self.errors.contains_key(def.key);
                div()
                    .w(Tokens::settings_field_width())
                    .rounded(Tokens::RADIUS_MD)
                    .when(invalid, |d| d.border_1().border_color(colors.danger))
                    .child({
                        let input = Input::new(state).aria_label(def.name);
                        match unit {
                            Some(unit) => input.suffix(
                                div()
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_faint)
                                    .child(*unit),
                            ),
                            None => input,
                        }
                    })
                    .into_any_element()
            }
        }
    }

    fn render_item(
        &self,
        def: &SettingDef,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let colors = *ui_theme::colors(cx);
        let control = self.render_control(def, cx);
        let error = self.errors.get(def.key).cloned();
        h_flex()
            .id(SharedString::from(format!("setting-{}", def.key)))
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
                            .child(def.name),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child(def.description),
                    ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_end()
                    .gap(Tokens::space(1.0))
                    .child(control)
                    .children(error.map(|message| {
                        h_flex()
                            .id(SharedString::from(format!("setting-error-{}", def.key)))
                            .test_support()
                            .max_w(Tokens::settings_field_width())
                            .gap(Tokens::space(1.0))
                            .text_size(Tokens::text_xs())
                            .text_color(colors.danger)
                            .child(Icon::new(IconName::CircleAlert).size(Tokens::icon_sm()))
                            .child(message)
                    })),
            )
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let query = self.search.read(cx).value();
        let searching = !query.trim().is_empty();
        let visible: Vec<&SettingDef> = self.visible(cx);
        let matching_sections: Vec<Section> = Section::ALL
            .into_iter()
            .filter(|s| visible.iter().any(|d| d.section == *s))
            .collect();

        let nav: Vec<_> = Section::ALL
            .into_iter()
            .map(|section| {
                let active = !searching && section == self.section;
                let dim = searching && !matching_sections.contains(&section);
                h_flex()
                    .id(SharedString::from(format!(
                        "settings-section-{}",
                        section.key()
                    )))
                    .test_support()
                    .h(Tokens::nav_row_height())
                    .px(Tokens::space(2.5))
                    .gap(Tokens::space(2.5))
                    .rounded(Tokens::RADIUS_MD)
                    .cursor_pointer()
                    .text_color(if dim {
                        colors.text_faint
                    } else {
                        colors.text_muted
                    })
                    .when(active, |d| d.bg(colors.pressed).text_color(colors.text))
                    .when(!active, |d| d.hover(|d| d.bg(colors.hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search.update(cx, |s, cx| s.set_value("", window, cx));
                        this.select_section(section, cx);
                    }))
                    .child(
                        Icon::new(section.icon())
                            .size(Tokens::icon_md())
                            .text_color(if active {
                                colors.accent_text
                            } else {
                                colors.text_faint
                            }),
                    )
                    .child(section.label())
            })
            .collect();

        let mut body = v_flex().w_full();
        if searching {
            for section in &matching_sections {
                body = body.child(
                    div()
                        .pt(Tokens::space(4.0))
                        .child(eyebrow(section.label(), cx)),
                );
                for def in visible.iter().filter(|d| d.section == *section) {
                    body = body.child(self.render_item(def, cx));
                }
            }
            if visible.is_empty() {
                body = body.child(
                    div()
                        .id("settings-no-results")
                        .test_support()
                        .pt(Tokens::space(8.0))
                        .text_color(colors.text_muted)
                        .child(format!("No settings match “{}”.", query.trim())),
                );
            }
        } else {
            if self.section == Section::Ai {
                body = body.child(self.ai_panel.clone());
            }
            for def in &visible {
                body = body.child(self.render_item(def, cx));
            }
        }

        let title = if searching {
            "Search results".to_owned()
        } else {
            self.section.label().to_owned()
        };

        h_flex()
            .id("settings-window")
            .test_support()
            .key_context(keymap::OVERLAY)
            .track_focus(&self.focus)
            .w(Tokens::settings_width())
            .h(Tokens::settings_height())
            // Never taller than the window allows (minimum window size); the body scrolls.
            .max_h(window.viewport_size().height * 0.8)
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.overlay)
            .border_1()
            .border_color(colors.border_strong)
            .shadow_lg()
            .overflow_hidden()
            .child(
                v_flex()
                    .w(Tokens::settings_nav_width())
                    .h_full()
                    .flex_none()
                    .p(Tokens::space(3.0))
                    .gap(Tokens::space(1.0))
                    .bg(colors.panel)
                    .border_r_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .id("settings-search")
                            .test_support()
                            .mb(Tokens::space(2.0))
                            .child(
                                Input::new(&self.search)
                                    .aria_label("Search settings")
                                    // The field keeps its plain border when focused: a calm search box, not
                                    // an accent ring competing with the selected section.
                                    .focus_bordered(false)
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .size(Tokens::icon_sm())
                                            .text_color(colors.text_faint),
                                    )
                                    .cleanable(true),
                            ),
                    )
                    .children(nav)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_faint)
                            .child("Saved automatically to settings.toml"),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .px(Tokens::space(6.0))
                            .pt(Tokens::space(5.0))
                            .pb(Tokens::space(2.0))
                            .justify_between()
                            .child(
                                div()
                                    .text_size(Tokens::text_xl())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .child(title),
                            )
                            .child(
                                h_flex()
                                    .gap(Tokens::space(1.5))
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_faint)
                                    .child(kbd("Esc", cx))
                                    .child("close"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(
                                div()
                                    .id("settings-body")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .px(Tokens::space(6.0))
                                    .pb(Tokens::space(6.0))
                                    .child(body),
                            )
                            // A scroll cue: the last lines fade into the panel edge, so a section that
                            // continues below reads as scrollable. It covers only the bottom padding when
                            // scrolled to the end.
                            .child(
                                div()
                                    .id("settings-scroll-cue")
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .h(Tokens::space(6.0))
                                    .bg(linear_gradient(
                                        180.0,
                                        linear_color_stop(colors.overlay.opacity(0.0), 0.0),
                                        linear_color_stop(colors.overlay, 1.0),
                                    )),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    #[test]
    fn every_section_has_settings_and_keys_are_unique() {
        let defs = definitions();
        for s in Section::ALL {
            assert!(defs.iter().any(|d| d.section == s), "{s:?} is empty");
        }
        let mut keys: Vec<_> = defs.iter().map(|d| d.key).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(n, keys.len());
    }

    #[test]
    fn search_matches_names_case_insensitively() {
        let defs = definitions();
        let names: Vec<_> = filter(&defs, "CACHE").iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["Preview cache size", "Extra cache folders"]);
    }
}
