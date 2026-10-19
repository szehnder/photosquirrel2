//! The navigator's views and the registry later changes plug their real views into.
//!
//! Every [`ViewId`] has a factory in the [`ViewRegistry`]. Until a change registers one (the `gallery` change
//! registers `Library` and `Excluded`, `scan-dashboard` registers `Sources`, and so on), the shell shows a
//! [`PlaceholderView`]. The right-hand inspector is registered the same way ([`ViewRegistry::set_inspector`]).
//!
//! ```ignore
//! ViewRegistry::register(cx, ViewId::Library, |ctx, window, cx| {
//!     cx.new(|cx| LibraryView::new(ctx.service.clone(), window, cx)).into()
//! });
//! ```

pub mod library;

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use ui_theme::Tokens;

use crate::actions::*;
use crate::assets::IconName;
use crate::service::ServiceHandle;

/// The seven navigator entries, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewId {
    Sources,
    Library,
    Duplicates,
    Excluded,
    AiReview,
    Export,
    Reports,
}

/// Which part of the navigator an entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavGroup {
    Catalog,
    Review,
    Output,
}

impl NavGroup {
    pub fn label(self) -> &'static str {
        match self {
            NavGroup::Catalog => "Catalog",
            NavGroup::Review => "Review",
            NavGroup::Output => "Deliver",
        }
    }
}

impl ViewId {
    pub const ALL: [ViewId; 7] = [
        ViewId::Sources,
        ViewId::Library,
        ViewId::Duplicates,
        ViewId::Excluded,
        ViewId::AiReview,
        ViewId::Export,
        ViewId::Reports,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ViewId::Sources => "Sources",
            ViewId::Library => "Library",
            ViewId::Duplicates => "Duplicates",
            ViewId::Excluded => "Excluded",
            ViewId::AiReview => "AI Review",
            ViewId::Export => "Export",
            ViewId::Reports => "Reports",
        }
    }

    /// Stable id for element ids and persistence (`library`, `ai_review`, ...).
    pub fn key(self) -> &'static str {
        match self {
            ViewId::Sources => "sources",
            ViewId::Library => "library",
            ViewId::Duplicates => "duplicates",
            ViewId::Excluded => "excluded",
            ViewId::AiReview => "ai_review",
            ViewId::Export => "export",
            ViewId::Reports => "reports",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == key)
    }

    pub fn group(self) -> NavGroup {
        match self {
            ViewId::Sources | ViewId::Library => NavGroup::Catalog,
            ViewId::Duplicates | ViewId::Excluded | ViewId::AiReview => NavGroup::Review,
            ViewId::Export | ViewId::Reports => NavGroup::Output,
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            ViewId::Sources => IconName::HardDrive,
            ViewId::Library => IconName::Images,
            ViewId::Duplicates => IconName::Copy,
            ViewId::Excluded => IconName::EyeOff,
            ViewId::AiReview => IconName::Sparkles,
            ViewId::Export => IconName::FolderOutput,
            ViewId::Reports => IconName::ChartColumn,
        }
    }

    /// The action that shows this view.
    pub fn action(self) -> Box<dyn Action> {
        match self {
            ViewId::Sources => Box::new(ShowSources),
            ViewId::Library => Box::new(ShowLibrary),
            ViewId::Duplicates => Box::new(ShowDuplicates),
            ViewId::Excluded => Box::new(ShowExcluded),
            ViewId::AiReview => Box::new(ShowAiReview),
            ViewId::Export => Box::new(ShowExport),
            ViewId::Reports => Box::new(ShowReports),
        }
    }

    /// One line for the placeholder and the palette.
    pub fn summary(self) -> &'static str {
        match self {
            ViewId::Sources => "Drives, USB backups and NAS shares, with live scan progress.",
            ViewId::Library => "The best copy of every photo, in one fast grid.",
            ViewId::Duplicates => "Groups of the same photo at any size; review and pick keepers.",
            ViewId::Excluded => {
                "Icons, caches and tiny images set aside, with the reason for each."
            }
            ViewId::AiReview => "Optional descriptions and tags from your local model.",
            ViewId::Export => "Copy the keepers into a clean library. Sources stay untouched.",
            ViewId::Reports => "What was found, kept, skipped and exported, folder by folder.",
        }
    }

    /// The OpenSpec change that delivers the real view (shown on placeholders).
    pub fn delivered_by(self) -> &'static str {
        match self {
            ViewId::Sources => "scan-dashboard",
            ViewId::Library | ViewId::Excluded => "gallery",
            ViewId::Duplicates => "duplicate-review",
            ViewId::AiReview => "ai-insights",
            ViewId::Export | ViewId::Reports => "export",
        }
    }
}

/// How the Library shows photos.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryMode {
    #[default]
    Grid,
    Loupe,
}

/// What a view factory gets: the service handle for data, and the Library mode.
#[derive(Clone)]
pub struct ViewContext {
    pub service: ServiceHandle,
    pub library_mode: LibraryMode,
    /// The shell's default inspector (a custom inspector shows it when it has nothing selected).
    pub default_inspector: Entity<crate::inspector::InspectorView>,
}

pub type ViewFactory = Rc<dyn Fn(&ViewContext, &mut Window, &mut App) -> AnyView>;
pub type InspectorFactory = Rc<dyn Fn(&ViewContext, ViewId, &mut Window, &mut App) -> AnyView>;
pub type LibraryModeHook = Rc<dyn Fn(LibraryMode, &mut Window, &mut App)>;

/// Factories for the central views and the inspector.
#[derive(Default)]
pub struct ViewRegistry {
    views: HashMap<ViewId, ViewFactory>,
    inspector: Option<InspectorFactory>,
    library_mode: Option<LibraryModeHook>,
}

impl Global for ViewRegistry {}

impl ViewRegistry {
    /// Replace the view for `id` (call during start-up, before the window opens).
    pub fn register(
        cx: &mut App,
        id: ViewId,
        factory: impl Fn(&ViewContext, &mut Window, &mut App) -> AnyView + 'static,
    ) {
        cx.default_global::<ViewRegistry>()
            .views
            .insert(id, Rc::new(factory));
    }

    /// Replace the inspector (it gets the active view, to show what that view has selected).
    pub fn set_inspector(
        cx: &mut App,
        factory: impl Fn(&ViewContext, ViewId, &mut Window, &mut App) -> AnyView + 'static,
    ) {
        cx.default_global::<ViewRegistry>().inspector = Some(Rc::new(factory));
    }

    /// Called when the shell switches the Library between grid and loupe (G, E, Esc, the palette).
    pub fn on_library_mode(
        cx: &mut App,
        hook: impl Fn(LibraryMode, &mut Window, &mut App) + 'static,
    ) {
        cx.default_global::<ViewRegistry>().library_mode = Some(Rc::new(hook));
    }

    pub(crate) fn library_mode_hook(cx: &App) -> Option<LibraryModeHook> {
        cx.try_global::<ViewRegistry>()?.library_mode.clone()
    }

    pub fn is_registered(cx: &App, id: ViewId) -> bool {
        cx.try_global::<ViewRegistry>()
            .is_some_and(|r| r.views.contains_key(&id))
    }

    pub(crate) fn view_factory(cx: &App, id: ViewId) -> Option<ViewFactory> {
        cx.try_global::<ViewRegistry>()?.views.get(&id).cloned()
    }

    pub(crate) fn inspector_factory(cx: &App) -> Option<InspectorFactory> {
        cx.try_global::<ViewRegistry>()?.inspector.clone()
    }
}

/// A view that a later change replaces: what it will show, in the shell's empty-state style.
pub struct PlaceholderView {
    id: ViewId,
    mode: LibraryMode,
}

impl PlaceholderView {
    pub fn new(id: ViewId, mode: LibraryMode) -> Self {
        Self { id, mode }
    }
}

impl Render for PlaceholderView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let title = match (self.id, self.mode) {
            (ViewId::Library, LibraryMode::Loupe) => "Loupe".to_owned(),
            (id, _) => id.label().to_owned(),
        };
        let photo_surface = matches!(self.id, ViewId::Library | ViewId::Duplicates);
        let bg = match (self.id, self.mode) {
            (ViewId::Library, LibraryMode::Loupe) => colors.loupe,
            _ if photo_surface => colors.gallery,
            _ => colors.background,
        };
        v_flex()
            .id(SharedString::from(format!("view-{}", self.id.key())))
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(Tokens::space(3.0))
            .bg(bg)
            .child(
                div()
                    .size(Tokens::size(56.0))
                    .rounded(Tokens::RADIUS_LG)
                    .bg(colors.raised)
                    .border_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(self.id.icon())
                            .size(Tokens::icon_lg())
                            .text_color(colors.text_muted),
                    ),
            )
            .child(
                div()
                    .text_size(Tokens::text_xl())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(title),
            )
            .child(
                div()
                    .max_w(Tokens::prose_width())
                    .text_center()
                    .text_size(Tokens::text_md())
                    .text_color(colors.text_muted)
                    .child(self.id.summary()),
            )
            .child(
                h_flex()
                    .mt(Tokens::space(1.0))
                    .px(Tokens::space(2.0))
                    .py(Tokens::space(0.5))
                    .rounded(Tokens::RADIUS_SM)
                    .bg(colors.raised)
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child("Coming in a later milestone"),
            )
    }
}
