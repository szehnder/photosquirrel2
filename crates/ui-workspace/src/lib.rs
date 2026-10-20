//! The PhotoSquirrel application shell: title bar, navigator, central view, inspector, status bar, command
//! palette, settings window, onboarding and the interrupted-run banner.
//!
//! The binary calls [`init`] once, then opens a window whose root view is a [`Shell`] (wrapped in gpui-kit's
//! `Root` by `gpui_kit::open_window`).
//!
//! ## Extension points for later changes
//! - **Views:** [`views::ViewRegistry::register`] replaces a navigator entry's placeholder with a real view
//!   (`gallery` -> Library and Excluded, `scan-dashboard` -> Sources, `duplicate-review` -> Duplicates,
//!   `ai-insights` -> AI Review, `export` -> Export and Reports). The factory gets a [`views::ViewContext`]
//!   with the [`service::ServiceHandle`].
//! - **Inspector:** [`views::ViewRegistry::set_inspector`], or feed the default [`inspector::InspectorView`]
//!   an [`inspector::InspectedItem`].
//! - **Commands:** declare a GPUI action, add a binding to [`keymap`], register a
//!   [`palette::CommandMeta`] with [`palette::CommandRegistry::register`].
//! - **Engine:** implement [`service::LibraryBackend`] over `ps-service` and pass it in [`ShellConfig`].
//! - **Settings:** add rows to [`settings_window::definitions`].

pub mod actions;
pub mod assets;
pub mod gallery_inspector;
pub mod inspector;
pub mod keymap;
pub mod library_backend;
pub mod motion;
pub mod net;
pub mod onboarding;
pub mod palette;
pub mod persist;
pub mod review_source;
pub mod scan_backend;
pub mod service;
pub mod settings;
pub mod settings_ai;
pub mod settings_window;
pub mod shell;
pub mod startup;
pub mod views;
pub mod widgets;

mod chrome;

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, Global};
use ps_model::settings::ThemeSetting;
use ps_service::settings::SettingsStore;

pub use shell::Shell;
pub use ui_theme as theme;

use crate::actions::*;
use crate::palette::{CommandMeta, CommandRegistry};
use crate::service::{LibraryBackend, ServiceHandle};
use crate::settings::AppSettings;

/// Everything the shell needs at start-up.
pub struct ShellConfig {
    /// Where `settings.toml` lives (`None`: defaults, nothing saved).
    pub settings: Option<SettingsStore>,
    /// `--theme` for this run (not saved).
    pub theme_override: Option<ThemeSetting>,
    /// Where `ui-state.json` (and later the catalog) lives (`None`: nothing saved).
    pub data_dir: Option<PathBuf>,
    pub backend: Arc<dyn LibraryBackend>,
}

impl ShellConfig {
    /// No persistence, an empty library: tests and `--no-persist`.
    pub fn ephemeral(backend: Arc<dyn LibraryBackend>) -> Self {
        Self {
            settings: None,
            theme_override: None,
            data_dir: None,
            backend,
        }
    }
}

/// App-wide shell state the window reads when it is built.
pub struct ShellGlobals {
    pub service: ServiceHandle,
    /// The gallery's data source (from [`LibraryBackend::gallery`]).
    pub gallery: Arc<dyn ui_gallery::source::GallerySource>,
    pub data_dir: Option<PathBuf>,
}

impl Global for ShellGlobals {}

/// Initialise theme, fonts, keymap, commands and settings. Call once, after `gpui_kit::init`.
pub fn init(config: ShellConfig, cx: &mut App) {
    ui_theme::init(cx);
    keymap::bind_default_keys(cx);
    register_commands(cx);
    AppSettings::init(config.settings, config.theme_override, cx);
    ui_gallery::init(cx);
    crate::views::library::register(cx);
    crate::views::scan::register(cx);
    cx.set_global(review_source::ReviewGlobal(config.backend.clone().review()));
    crate::views::duplicates::register(cx);
    crate::views::ai_review::register(cx);
    let gallery = config.backend.clone().gallery();
    if let Some(scans) = config.backend.clone().scans() {
        crate::views::scan::install(scans, gallery.clone(), cx);
    }
    cx.set_global(ShellGlobals {
        gallery,
        service: ServiceHandle::spawn(config.backend),
        data_dir: config.data_dir,
    });
}

fn register_commands(cx: &mut App) {
    if cx.has_global::<CommandRegistry>() {
        return;
    }
    for meta in [
        CommandMeta::new("Go to", "Library", ShowLibrary).keywords(&["grid", "photos", "gallery"]),
        CommandMeta::new("Go to", "Loupe", ShowLoupe)
            .keywords(&["single", "large", "zoom", "photo"]),
        CommandMeta::new("Go to", "Duplicates", ShowDuplicates)
            .keywords(&["groups", "copies", "review"]),
        CommandMeta::new("Go to", "Sources", ShowSources)
            .keywords(&["drives", "folders", "nas", "usb"]),
        CommandMeta::new("Go to", "Excluded", ShowExcluded).keywords(&["skipped", "junk", "icons"]),
        CommandMeta::new("Go to", "AI Review", ShowAiReview).keywords(&["llm", "tags", "describe"]),
        CommandMeta::new("Go to", "Export", ShowExport)
            .keywords(&["copy", "library", "apple", "google"]),
        CommandMeta::new("Go to", "Reports", ShowReports).keywords(&["completion", "summary"]),
        CommandMeta::new("Library", "Add Source…", AddSource)
            .keywords(&["folder", "drive", "import"]),
        CommandMeta::new("Library", "Rescan All Sources", Rescan)
            .keywords(&["scan", "refresh", "update"]),
        CommandMeta::new("Library", "Resume Interrupted Scan", ResumeScan).keywords(&["continue"]),
        CommandMeta::new("View", "Toggle Inspector", ToggleInspector)
            .keywords(&["details", "metadata", "panel"]),
        CommandMeta::new("View", "Toggle Navigator", ToggleNavigator)
            .keywords(&["sidebar", "panel"]),
        CommandMeta::new("View", "Zoom In", ZoomIn).keywords(&["bigger", "text size"]),
        CommandMeta::new("View", "Zoom Out", ZoomOut).keywords(&["smaller", "text size"]),
        CommandMeta::new("View", "Reset Zoom", ZoomReset),
        CommandMeta::new("View", "Toggle Light/Dark Theme", ToggleTheme)
            .keywords(&["appearance", "mode"]),
        CommandMeta::new("Preferences", "Open Settings", OpenSettings)
            .keywords(&["preferences", "options"]),
        CommandMeta::new("Application", "Show All Commands", OpenCommandPalette)
            .keywords(&["palette"]),
        CommandMeta::new("Application", "Quit PhotoSquirrel", Quit).keywords(&["exit", "close"]),
    ] {
        CommandRegistry::register(cx, meta);
    }
}
