//! The settings global: the user's [`Settings`] in memory, applied live (theme, motion, text size) and written
//! to `settings.toml` in the background through `ps-service::settings`. Every change is validated first; an
//! invalid change is returned as field errors and nothing is applied or written.
//!
//! The app owns the file while it runs (ADR-0049): each save re-reads it first, and when it changed on disk a
//! consent the file no longer holds is withdrawn here too (and a running AI queue pauses); other values are the
//! app's.

use std::sync::{Arc, Mutex};

use gpui_kit::*;
use ps_model::settings::{FieldError, Settings, ThemeSetting};
use ps_service::settings::{Recovery, SaveBase, SettingsStore};
use ui_theme::{Mode, ThemeChoice};

pub struct AppSettings {
    settings: Settings,
    store: Option<SettingsStore>,
    /// `--theme` for this run: shown instead of the saved theme, never written.
    theme_override: Option<ThemeSetting>,
    appearance: WindowAppearance,
    os_reduce_motion: bool,
    /// The settings file was corrupt at start-up (shown once).
    recovered: Option<Recovery>,
    /// The latest settings waiting to be written (the writer takes whatever is newest).
    pending_write: Arc<Mutex<Option<Settings>>>,
    /// What we last loaded or saved, to notice edits made by others ([`SettingsStore::save_merged`]).
    base: Arc<Mutex<SaveBase>>,
}

impl Global for AppSettings {}

fn theme_choice(t: ThemeSetting) -> ThemeChoice {
    match t {
        ThemeSetting::Dark => ThemeChoice::Dark,
        ThemeSetting::Light => ThemeChoice::Light,
        ThemeSetting::System => ThemeChoice::System,
    }
}

impl AppSettings {
    /// Load from `store` (blocking: a small file, read once before the window opens) and apply.
    pub fn init(store: Option<SettingsStore>, theme_override: Option<ThemeSetting>, cx: &mut App) {
        let (settings, recovered, base) = match &store {
            Some(store) => {
                let (loaded, base) = store.load_tracked();
                (loaded.settings, loaded.recovered, base)
            }
            None => (Settings::default(), None, SaveBase::default()),
        };
        cx.set_global(AppSettings {
            settings,
            store,
            theme_override,
            appearance: cx.window_appearance(),
            os_reduce_motion: false,
            recovered,
            pending_write: Arc::default(),
            base: Arc::new(Mutex::new(base)),
        });
        Self::apply(cx);
    }

    pub fn get(cx: &App) -> &Settings {
        &cx.global::<AppSettings>().settings
    }

    /// The settings, when the global is installed (views built in isolated tests may run without it).
    pub fn try_get(cx: &App) -> Option<&Settings> {
        cx.try_global::<AppSettings>().map(|a| &a.settings)
    }

    pub fn store(cx: &App) -> Option<&SettingsStore> {
        cx.try_global::<AppSettings>()?.store.as_ref()
    }

    /// The theme in effect (the `--theme` override, else the saved one).
    pub fn effective_theme(cx: &App) -> ThemeSetting {
        let this = cx.global::<AppSettings>();
        this.theme_override
            .unwrap_or(this.settings.appearance.theme)
    }

    /// The corrupt-file notice, until [`Self::dismiss_recovery`].
    pub fn recovery(cx: &App) -> Option<&Recovery> {
        cx.try_global::<AppSettings>()?.recovered.as_ref()
    }

    pub fn dismiss_recovery(cx: &mut App) {
        cx.global_mut::<AppSettings>().recovered = None;
    }

    /// Validate and apply `edit`; write in the background. On invalid values nothing changes.
    pub fn update(cx: &mut App, edit: impl FnOnce(&mut Settings)) -> Result<(), Vec<FieldError>> {
        let this = cx.global::<AppSettings>();
        let mut next = this.settings.clone();
        edit(&mut next);
        if next == this.settings {
            return Ok(());
        }
        let errors = ps_service::settings::validate(&next);
        if !errors.is_empty() {
            return Err(errors);
        }
        // A new AI destination (provider, server address or consent) pauses a running queue before anything
        // more is sent (ADR-0047). The pause writes the catalog: off the UI thread.
        if ps_service::ai::destination_fingerprint(&next.ai)
            != ps_service::ai::destination_fingerprint(&this.settings.ai)
            && let Some(src) = crate::review_source::ReviewGlobal::get(cx)
        {
            cx.background_spawn(async move {
                if let Err(e) = src.ai_destination_changed() {
                    log::error!("could not pause the AI queue after a provider change: {e:#}");
                }
            })
            .detach();
        }
        let this = cx.global_mut::<AppSettings>();
        if next.appearance.theme != this.settings.appearance.theme {
            // Choosing a theme in settings replaces the run's `--theme` override.
            this.theme_override = None;
        }
        this.settings = next.clone();
        if let Some(store) = this.store.clone() {
            let pending = this.pending_write.clone();
            let base = this.base.clone();
            *pending.lock().unwrap_or_else(|p| p.into_inner()) = Some(next);
            let write = cx.background_spawn(async move {
                let latest = pending.lock().unwrap_or_else(|p| p.into_inner()).take()?;
                let mut base = base.lock().unwrap_or_else(|p| p.into_inner());
                match store.save_merged(&latest, &mut base) {
                    // The file withdrew the consent we were about to write: adopt that in memory.
                    Ok(saved) if saved.ai.consent != latest.ai.consent => latest.ai.consent,
                    Ok(_) => None,
                    Err(err) => {
                        log::error!("could not save settings: {err}");
                        None
                    }
                }
            });
            cx.spawn(async move |cx| {
                if let Some(withdrawn) = write.await {
                    cx.update(|cx| Self::withdraw_consent(&withdrawn, cx));
                }
            })
            .detach();
        }
        Self::apply(cx);
        Ok(())
    }

    /// The settings file no longer holds `consent` (edited by hand or by another process while we ran): withdraw
    /// it in memory too, which pauses a running AI queue (ADR-0049). A consent given again since stays.
    pub fn withdraw_consent(consent: &ps_model::settings::HostedConsent, cx: &mut App) {
        if Self::get(cx).ai.consent.as_ref() == Some(consent) {
            log::warn!("AI consent was withdrawn in the settings file; the AI queue pauses");
            let _ = Self::update(cx, |s| s.ai.consent = None);
        }
    }

    /// The OS appearance changed (for `System`).
    pub fn set_appearance(appearance: WindowAppearance, cx: &mut App) {
        let this = cx.global_mut::<AppSettings>();
        if this.appearance != appearance {
            this.appearance = appearance;
            Self::apply(cx);
        }
    }

    /// The OS reduce-motion preference was read (for `MotionSetting::System`).
    pub fn set_os_reduce_motion(reduce: bool, cx: &mut App) {
        let this = cx.global_mut::<AppSettings>();
        if this.os_reduce_motion != reduce {
            this.os_reduce_motion = reduce;
            Self::apply(cx);
        }
    }

    /// Whether motion is reduced right now.
    pub fn reduce_motion(cx: &App) -> bool {
        let this = cx.global::<AppSettings>();
        this.settings
            .appearance
            .motion
            .reduce(this.os_reduce_motion)
    }

    /// The light/dark mode in effect.
    pub fn mode(cx: &App) -> Mode {
        let this = cx.global::<AppSettings>();
        theme_choice(Self::effective_theme(cx)).resolve(this.appearance)
    }

    fn apply(cx: &mut App) {
        let mode = Self::mode(cx);
        let reduce = Self::reduce_motion(cx);
        let size = cx.global::<AppSettings>().settings.appearance.ui_font_size;
        cx.set_reduce_motion(reduce);
        ui_theme::apply(mode, size, cx);
    }
}
