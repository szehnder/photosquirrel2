//! The main window's root view: title bar, navigator, central view, inspector, status bar, overlays.
//!
//! Start-up never waits on the engine: the shell paints its chrome with a loading state, asks the service
//! thread for the catalog summary and an interrupted run, and fills in when they arrive.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::*;
use ps_model::ItemId;
use ps_model::settings::ThemeSetting;
use ui_theme::{Mode, Tokens};

use crate::ShellGlobals;
use crate::actions::*;
use crate::inspector::InspectorView;
use crate::keymap;
use crate::palette::{CommandPalette, PaletteEvent};
use crate::persist::{UiState, UiStateFile, WindowGeometry};
use crate::service::{ActivitySnapshot, CatalogSummary, InterruptedRun, Loadable, ServiceHandle};
use crate::settings::AppSettings;
use crate::settings_window::SettingsWindow;
use crate::views::{LibraryMode, PlaceholderView, ViewContext, ViewId, ViewRegistry};

/// Layout changes settle this long before they are written.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);
/// Live activity is sampled this often while something runs (ADR-0019).
const ACTIVITY_INTERVAL: Duration = Duration::from_millis(100);
/// How long the first frame may wait for an instant catalog answer (see `Reply::wait_briefly`).
const START_GRACE: Duration = Duration::from_millis(3);
/// Info toasts disappear after this long.
const TOAST_DURATION: Duration = Duration::from_secs(6);

/// Which panel a splitter resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Navigator,
    Inspector,
}

/// Drag payload of a splitter (renders nothing).
#[derive(Debug, Clone, Copy)]
pub struct DraggedSplitter(pub Side);

impl Render for DraggedSplitter {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        Empty
    }
}

pub(crate) enum Overlay {
    Palette(Entity<CommandPalette>),
    Settings(Entity<SettingsWindow>),
    /// "Why no preview?" for one file (ADR-0109).
    PreviewHelp(Entity<crate::views::preview_help::PreviewHelpDialog>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub title: SharedString,
    pub body: SharedString,
}

/// The state of the interrupted-run banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerState {
    Offered(InterruptedRun),
    Resuming(InterruptedRun),
}

pub struct Shell {
    pub(crate) focus: FocusHandle,
    pub(crate) service: ServiceHandle,
    ui_file: Option<UiStateFile>,
    pub(crate) layout: UiState,
    pub(crate) summary: Loadable<CatalogSummary>,
    pub(crate) banner: Option<BannerState>,
    pub(crate) activity: Option<ActivitySnapshot>,
    views: HashMap<(ViewId, LibraryMode), AnyView>,
    inspector_default: Entity<InspectorView>,
    inspector_custom: Option<AnyView>,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) toast: Option<Toast>,
    /// Frames painted (tests check the UI keeps drawing).
    frames: u64,
    save_task: Option<Task<()>>,
    activity_task: Option<Task<()>>,
    toast_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let globals = cx.global::<ShellGlobals>();
        let service = globals.service.clone();
        let ui_file = globals.data_dir.as_deref().map(UiStateFile::in_dir);
        let layout = ui_file
            .as_ref()
            .and_then(UiStateFile::load)
            .unwrap_or_default();

        let subscriptions = vec![
            cx.observe_global::<AppSettings>(|_, cx| cx.notify()),
            cx.observe_window_bounds(window, |this, window, cx| this.schedule_save(window, cx)),
            cx.observe_window_appearance(window, |_, window, cx| {
                AppSettings::set_appearance(window.appearance(), cx);
            }),
        ];
        AppSettings::set_appearance(window.appearance(), cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = weak.upgrade() {
                this.update(cx, |this, cx| this.save_now(window, cx));
            }
            true
        });

        let inspector_default = cx.new(|_| InspectorView::new());
        let mut this = Self {
            focus: cx.focus_handle(),
            service,
            ui_file,
            layout,
            summary: Loadable::Loading,
            banner: None,
            activity: None,
            views: HashMap::new(),
            inspector_default,
            inspector_custom: None,
            overlay: None,
            toast: None,
            frames: 0,
            save_task: None,
            activity_task: None,
            toast_task: None,
            _subscriptions: subscriptions,
        };
        if let Some(recovery) = AppSettings::recovery(cx).cloned() {
            this.show_toast(
                ToastKind::Warning,
                "Settings were reset",
                format!(
                    "Your settings file could not be read, so defaults are in use. The old file was kept as {}.",
                    recovery
                        .backup
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                false,
                cx,
            );
            AppSettings::dismiss_recovery(cx);
        }
        this.refresh(window, cx);
        this
    }

    /// The shell of `window`, if its root view is one.
    pub fn for_window(window: &Window, cx: &App) -> Option<Entity<Shell>> {
        let root = window.root::<gpui_kit::component::Root>()??;
        root.read(cx).view().clone().downcast::<Shell>().ok()
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    // ---- state for tests and later changes ------------------------------------------------------------

    pub fn active_view(&self) -> ViewId {
        self.layout.active_view
    }

    pub fn library_mode(&self) -> LibraryMode {
        self.layout.library_mode
    }

    pub fn layout(&self) -> &UiState {
        &self.layout
    }

    pub fn summary(&self) -> &Loadable<CatalogSummary> {
        &self.summary
    }

    pub fn banner(&self) -> Option<&BannerState> {
        self.banner.as_ref()
    }

    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    /// The live activity sample (a scan running), if any.
    pub fn activity_snapshot(&self) -> Option<&ActivitySnapshot> {
        self.activity.as_ref()
    }

    pub fn frames_drawn(&self) -> u64 {
        self.frames
    }

    pub fn palette(&self) -> Option<&Entity<CommandPalette>> {
        match &self.overlay {
            Some(Overlay::Palette(p)) => Some(p),
            _ => None,
        }
    }

    pub fn settings_window(&self) -> Option<&Entity<SettingsWindow>> {
        match &self.overlay {
            Some(Overlay::Settings(s)) => Some(s),
            _ => None,
        }
    }

    /// The default inspector (to feed it a selection).
    pub fn inspector(&self) -> &Entity<InspectorView> {
        &self.inspector_default
    }

    /// Whether the central view currently shows onboarding.
    /// Whether the inspector is on screen: as the layout says, except over the welcome view of an empty
    /// library, where it would only say "No photo selected" beside zero counts.
    pub fn inspector_visible(&self) -> bool {
        self.layout.inspector_shown() && !self.shows_onboarding()
    }

    pub fn shows_onboarding(&self) -> bool {
        matches!(self.layout.active_view, ViewId::Library | ViewId::Sources)
            && self.summary.ready().is_some_and(CatalogSummary::is_empty)
    }

    // ---- data ---------------------------------------------------------------------------------------

    /// Ask the service thread for the summary and an interrupted run; the UI shows loading until then.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.summary.ready().is_none() {
            self.summary = Loadable::Loading;
        }
        let summary = self
            .service
            .call(|b| b.summary().map_err(|e| format!("{e:#}")));
        let interrupted = self.service.call(|b| b.interrupted_run().ok().flatten());
        let first = self.summary.is_loading() && self.frames == 0;
        let grace = crate::startup::StartupGrace::remaining(cx)
            .unwrap_or(START_GRACE)
            .max(START_GRACE);
        let early = first.then(|| summary.wait_briefly(grace)).flatten();
        if let Some(early) = &early {
            self.summary = match early {
                Ok(s) => Loadable::Ready(s.clone()),
                Err(e) => Loadable::Failed(e.clone()),
            };
            Self::publish_totals(&self.summary, cx);
        }
        cx.spawn_in(window, async move |this, cx| {
            let executor = cx.background_executor().clone();
            let summary = match early {
                Some(v) => Ok(v),
                None => summary.wait(executor.clone()).await,
            };
            let _ = this.update(cx, |this, cx| {
                log::info!("catalog summary arrived: {summary:?}");
                this.summary = match summary {
                    Ok(Ok(s)) => Loadable::Ready(s),
                    Ok(Err(e)) => Loadable::Failed(e),
                    Err(_) => Loadable::Failed("the service stopped".into()),
                };
                Self::publish_totals(&this.summary, cx);
                cx.notify();
            });
            let interrupted = interrupted.wait(executor).await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                let offer = AppSettings::get(cx).general.offer_resume;
                if !matches!(this.banner, Some(BannerState::Resuming(_))) {
                    this.banner = interrupted.filter(|_| offer).map(BannerState::Offered);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Share the Library's file count with the views (the Library toolbar shows it next to its item count).
    fn publish_totals(summary: &Loadable<CatalogSummary>, cx: &mut App) {
        if let Some(s) = summary.ready() {
            cx.set_global(crate::views::library::LibraryTotals {
                files: s.photos + s.videos,
            });
        }
    }

    /// Resume the interrupted run named in the banner.
    pub fn resume(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(BannerState::Offered(run)) = self.banner.clone() else {
            if self.banner.is_none() {
                self.show_toast(
                    ToastKind::Info,
                    "Nothing to resume",
                    "No scan was interrupted.",
                    true,
                    cx,
                );
            }
            return;
        };
        self.banner = Some(BannerState::Resuming(run.clone()));
        cx.notify();
        let id = run.id;
        let result = self
            .service
            .call(move |b| b.resume_run(id).map_err(|e| format!("{e:#}")));
        cx.spawn_in(window, async move |this, cx| {
            let result = result
                .wait(cx.background_executor().clone())
                .await
                .unwrap_or_else(|_| Err("the service stopped".into()));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.banner = None;
                    this.start_activity_sampling(window, cx);
                    this.refresh(window, cx);
                }
                Err(e) => {
                    this.banner = Some(BannerState::Offered(run));
                    this.show_toast(ToastKind::Warning, "Could not resume", e, true, cx);
                }
            });
        })
        .detach();
    }

    pub fn rescan(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let result = self
            .service
            .call(|b| b.rescan().map_err(|e| format!("{e:#}")));
        cx.spawn_in(window, async move |this, cx| {
            let result = result
                .wait(cx.background_executor().clone())
                .await
                .unwrap_or_else(|_| Err("the service stopped".into()));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.start_activity_sampling(window, cx),
                Err(e) => this.show_toast(ToastKind::Info, "Nothing to rescan yet", e, true, cx),
            });
        })
        .detach();
    }

    pub fn add_source(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add source".into()),
        });
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            for path in paths {
                let result = service
                    .call(move |b| b.add_source(path).map_err(|e| format!("{e:#}")))
                    .wait(cx.background_executor().clone())
                    .await
                    .unwrap_or_else(|_| Err("the service stopped".into()));
                if let Err(e) = result {
                    let _ = this.update(cx, |this, cx| {
                        this.show_toast(ToastKind::Info, "Source not added", e, true, cx);
                    });
                    return;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| this.refresh(window, cx));
        })
        .detach();
    }

    /// Follow a run started elsewhere (Fill in previews from the Library or the dialog): the status bar and the
    /// Library's live updates sample it like a scan, until it ends.
    pub fn watch_activity(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.start_activity_sampling(window, cx);
    }

    /// Sample the backend's activity every 100 ms until it reports idle (ADR-0019).
    fn start_activity_sampling(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let service = self.service.clone();
        self.activity_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let snapshot = service
                    .call(|b| b.activity())
                    .wait(cx.background_executor().clone())
                    .await
                    .ok()
                    .flatten();
                let idle = snapshot.is_none();
                let alive = this
                    .update(cx, |this, cx| {
                        if this.activity != snapshot {
                            this.activity = snapshot;
                            cx.notify();
                        }
                    })
                    .is_ok();
                if idle || !alive {
                    break;
                }
                cx.background_executor().timer(ACTIVITY_INTERVAL).await;
            }
            let _ = this.update_in(cx, |this, window, cx| this.refresh(window, cx));
        }));
    }

    pub fn show_toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        auto_hide: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.toast = Some(Toast {
            kind,
            title: title.into(),
            body: body.into(),
        });
        self.toast_task = auto_hide.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TOAST_DURATION).await;
                let _ = this.update(cx, |this, cx| {
                    this.toast = None;
                    cx.notify();
                });
            })
        });
        cx.notify();
    }

    pub fn dismiss_toast(&mut self, cx: &mut Context<'_, Self>) {
        self.toast = None;
        self.toast_task = None;
        cx.notify();
    }

    // ---- navigation and layout ----------------------------------------------------------------------

    pub fn show_view(&mut self, view: ViewId, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.layout.active_view != view {
            self.layout.active_view = view;
            // The old view may hold focus and is no longer rendered: take it back so shell keys keep working.
            // Views that want focus (the grid, Duplicates) take it again once shown.
            if !self.focus.is_focused(window) {
                window.focus(&self.focus, cx);
            }
            if view == ViewId::Library {
                self.layout.library_mode = LibraryMode::Grid;
                if let Some(hook) = ViewRegistry::library_mode_hook(cx) {
                    window.defer(cx, move |window, cx| hook(LibraryMode::Grid, window, cx));
                }
            }
            self.schedule_save(window, cx);
            cx.notify();
        }
    }

    pub fn show_library(
        &mut self,
        mode: LibraryMode,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.layout.active_view != ViewId::Library || self.layout.library_mode != mode {
            self.layout.active_view = ViewId::Library;
            self.layout.library_mode = mode;
            self.schedule_save(window, cx);
            cx.notify();
            if let Some(hook) = ViewRegistry::library_mode_hook(cx) {
                window.defer(cx, move |window, cx| hook(mode, window, cx));
            }
        }
    }

    pub fn toggle_panel(&mut self, side: Side, window: &mut Window, cx: &mut Context<'_, Self>) {
        match side {
            Side::Navigator => self.layout.navigator_open = !self.layout.navigator_open,
            Side::Inspector => self.layout.toggle_inspector(),
        }
        self.schedule_save(window, cx);
        cx.notify();
    }

    /// Set a panel width (clamped to its range and so the centre keeps its minimum width).
    pub fn set_panel_width(
        &mut self,
        side: Side,
        width: Pixels,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let total = window.viewport_size().width;
        let other = match side {
            Side::Navigator if self.inspector_visible() => px(self.layout.inspector_width),
            Side::Inspector if self.layout.navigator_open => px(self.layout.navigator_width),
            _ => Pixels::ZERO,
        };
        let (lo, hi) = match side {
            Side::Navigator => (Tokens::NAVIGATOR_MIN, Tokens::NAVIGATOR_MAX),
            Side::Inspector => (Tokens::INSPECTOR_MIN, Tokens::INSPECTOR_MAX),
        };
        let room = (total - other - Tokens::CENTER_MIN).max(lo);
        let w = f32::from(width.clamp(lo, hi.min(room)));
        let slot = match side {
            Side::Navigator => &mut self.layout.navigator_width,
            Side::Inspector => &mut self.layout.inspector_width,
        };
        if (*slot - w).abs() >= 0.5 {
            *slot = w;
            self.schedule_save(window, cx);
            cx.notify();
        }
    }

    fn on_splitter_drag(
        &mut self,
        event: &DragMoveEvent<DraggedSplitter>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let side = event.drag(cx).0;
        let x = event.event.position.x;
        let width = match side {
            Side::Navigator => x - event.bounds.left(),
            Side::Inspector => event.bounds.right() - x,
        };
        self.set_panel_width(side, width, window, cx);
    }

    fn zoom(&mut self, delta: Option<f32>, cx: &mut Context<'_, Self>) {
        let _ = AppSettings::update(cx, |s| {
            let size = match delta {
                Some(d) => s.appearance.ui_font_size + d,
                None => ps_model::settings::DEFAULT_UI_FONT_SIZE,
            };
            s.appearance.ui_font_size = size.clamp(
                ps_model::settings::MIN_UI_FONT_SIZE,
                ps_model::settings::MAX_UI_FONT_SIZE,
            );
        });
    }

    fn toggle_theme(&mut self, cx: &mut Context<'_, Self>) {
        let next = match AppSettings::mode(cx) {
            Mode::Dark => ThemeSetting::Light,
            Mode::Light => ThemeSetting::Dark,
        };
        let _ = AppSettings::update(cx, |s| s.appearance.theme = next);
    }

    // ---- overlays -----------------------------------------------------------------------------------

    pub fn open_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<CommandPalette> {
        if let Some(p) = self.palette() {
            let p = p.clone();
            p.update(cx, |p, cx| p.focus_input(window, cx));
            return p;
        }
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
        let sub = cx.subscribe_in(
            &palette,
            window,
            |this, _, event: &PaletteEvent, window, cx| {
                match event {
                    PaletteEvent::Run(action) => {
                        let action = action.boxed_clone();
                        this.close_overlay(window, cx);
                        // Route through the focused shell, after the overlay is gone.
                        let focus = this.focus.clone();
                        window.defer(cx, move |window, cx| {
                            window.focus(&focus, cx);
                            window.dispatch_action(action, cx);
                        });
                    }
                    PaletteEvent::Dismiss => this.close_overlay(window, cx),
                }
            },
        );
        self._subscriptions.push(sub);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.overlay = Some(Overlay::Palette(palette.clone()));
        cx.notify();
        palette
    }

    pub fn open_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<SettingsWindow> {
        if let Some(s) = self.settings_window() {
            return s.clone();
        }
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        settings.update(cx, |s, cx| s.focus_search(window, cx));
        self.overlay = Some(Overlay::Settings(settings.clone()));
        cx.notify();
        settings
    }

    /// "Why no preview?" for the selected Library item, when it has a placeholder.
    pub fn show_selected_preview_help(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let model = crate::views::library::LibraryHub::model(cx);
        let item = {
            let m = model.read(cx);
            m.selection.focus.and_then(|i| {
                m.summary(i)
                    .filter(|s| s.placeholder.is_some())
                    .map(|s| s.id)
            })
        };
        if let Some(id) = item {
            self.open_preview_help(id, window, cx);
        }
    }

    /// The open "Why no preview?" dialog.
    pub fn preview_help(&self) -> Option<&Entity<crate::views::preview_help::PreviewHelpDialog>> {
        match &self.overlay {
            Some(Overlay::PreviewHelp(d)) => Some(d),
            _ => None,
        }
    }

    /// Open "Why no preview?" for `item` (ADR-0109): its content is read in the background, or on this thread
    /// within the start-up grace (scripted screenshots).
    pub fn open_preview_help(
        &mut self,
        item: ItemId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<crate::views::preview_help::PreviewHelpDialog> {
        use crate::views::preview_help::{PreviewHelpDialog, PreviewHelpEvent};
        let source = crate::views::library::LibraryHub::model(cx)
            .read(cx)
            .source()
            .clone();
        let primed = crate::startup::StartupGrace::deadline(cx).and_then(|deadline| {
            let s = source.clone();
            crate::startup::StartupGrace::within(deadline, move || {
                PreviewHelpDialog::read(&s, item)
            })
        });
        let dialog = cx.new(|cx| match primed {
            Some(state) => PreviewHelpDialog::with_state(item, source, state, cx),
            None => PreviewHelpDialog::new(item, source, cx),
        });
        self._subscriptions.push(cx.subscribe_in(
            &dialog,
            window,
            |this, _, e: &PreviewHelpEvent, window, cx| match e {
                PreviewHelpEvent::Close => this.close_overlay(window, cx),
                PreviewHelpEvent::ShowThem { reason, format } => {
                    let (reason, format) = (*reason, *format);
                    this.close_overlay(window, cx);
                    this.show_library(LibraryMode::Grid, window, cx);
                    if let Some(lib) = crate::views::library::LibraryHub::library(cx) {
                        lib.update(cx, |l, cx| {
                            l.show_no_preview(Some(reason), format, cx);
                        });
                    }
                }
                PreviewHelpEvent::FillStarted(_) => {
                    this.close_overlay(window, cx);
                    if let Some(lib) = crate::views::library::LibraryHub::library(cx) {
                        lib.update(cx, |l, cx| l.fill_started(cx));
                    }
                    this.watch_activity(window, cx);
                    this.show_toast(
                        ToastKind::Info,
                        "Filling in previews",
                        "Only the files that can get a preview now are read; progress is in Sources.",
                        true,
                        cx,
                    );
                }
            },
        ));
        let f = dialog.read(cx).focus_handle().clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::PreviewHelp(dialog.clone()));
        cx.notify();
        dialog
    }

    pub fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.overlay.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Apply a shell command by action name before the first frame (`--command`), so scripted screenshots
    /// show the state in frame one. Returns `false` for actions the shell does not handle directly; the caller
    /// dispatches those after the first frame.
    pub fn apply_startup_command(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        match name {
            "shell::OpenSettings" => {
                self.open_settings(window, cx);
            }
            "shell::OpenCommandPalette" => {
                self.open_palette(window, cx);
            }
            "shell::ToggleInspector" => self.toggle_panel(Side::Inspector, window, cx),
            "shell::ToggleNavigator" => self.toggle_panel(Side::Navigator, window, cx),
            "shell::ShowLoupe" => self.show_library(LibraryMode::Loupe, window, cx),
            "shell::ShowLibrary" => self.show_library(LibraryMode::Grid, window, cx),
            "shell::ShowPreviewHelp" => {
                // Scripted screenshots: "Why no preview?" for the `--gallery-select` item (ADR-0109).
                self.show_library(LibraryMode::Grid, window, cx);
                let model = crate::views::library::LibraryHub::model(cx);
                let id = {
                    let m = model.read(cx);
                    crate::startup::StartupGrace::select(cx)
                        .or(m.selection.focus)
                        .and_then(|i| m.index.id(i))
                };
                if let Some(id) = id {
                    self.open_preview_help(id, window, cx);
                }
            }
            other => match ViewId::ALL.into_iter().find(|v| v.action().name() == other) {
                Some(view) => self.show_view(view, window, cx),
                None => return false,
            },
        }
        true
    }

    // ---- persistence --------------------------------------------------------------------------------

    fn snapshot(&self, window: &Window) -> UiState {
        UiState {
            window: Some(WindowGeometry::from_bounds(window.window_bounds())),
            ..self.layout.clone()
        }
    }

    fn schedule_save(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.ui_file.is_none() {
            return;
        }
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update_in(cx, |this, window, cx| this.save_in_background(window, cx));
        }));
    }

    /// Snapshot the layout now and write `ui-state.json` on a background thread (after changes settle).
    pub fn save_in_background(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.save_task = None;
        let Some(file) = self.ui_file.clone() else {
            return;
        };
        let state = self.snapshot(window);
        let ticket = file.ticket();
        cx.background_spawn(async move {
            if let Err(err) = file.save_ticketed(&state, ticket) {
                log::warn!("could not save {}: {err}", file.path().display());
            }
        })
        .detach();
    }

    /// Write `ui-state.json` now, on this thread (on close and quit, when the process may exit next).
    pub fn save_now(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.save_task = None;
        let Some(file) = self.ui_file.clone() else {
            return;
        };
        let state = self.snapshot(window);
        let _ = cx;
        if let Err(err) = file.save(&state) {
            log::warn!("could not save {}: {err}", file.path().display());
        }
    }

    /// Remember the Excluded view's chips, sort and mode (written with the layout, after changes settle).
    pub fn set_excluded_state(
        &mut self,
        state: crate::persist::ExcludedViewState,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.layout.excluded != state {
            self.layout.excluded = state;
            self.schedule_save(window, cx);
        }
    }

    // ---- views --------------------------------------------------------------------------------------

    fn view_context(&self) -> ViewContext {
        ViewContext {
            service: self.service.clone(),
            library_mode: self.layout.library_mode,
            excluded: self.layout.excluded.clone(),
            default_inspector: self.inspector_default.clone(),
        }
    }

    pub(crate) fn central_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyView {
        let id = self.layout.active_view;
        let mode = if id == ViewId::Library {
            self.layout.library_mode
        } else {
            LibraryMode::Grid
        };
        if let Some(view) = self.views.get(&(id, mode)) {
            return view.clone();
        }
        let ctx = self.view_context();
        let view = match ViewRegistry::view_factory(cx, id) {
            Some(factory) => factory(&ctx, window, cx),
            None => cx.new(|_| PlaceholderView::new(id, mode)).into(),
        };
        self.views.insert((id, mode), view.clone());
        view
    }

    pub(crate) fn inspector_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyView {
        let (active, summary) = (self.layout.active_view, self.summary.clone());
        self.inspector_default
            .update(cx, |i, cx| i.set_context(active, summary, cx));
        if let Some(factory) = ViewRegistry::inspector_factory(cx) {
            if let Some(view) = &self.inspector_custom {
                return view.clone();
            }
            let view = factory(&self.view_context(), self.layout.active_view, window, cx);
            self.inspector_custom = Some(view.clone());
            return view;
        }
        let (view, summary) = (self.layout.active_view, self.summary.clone());
        self.inspector_default
            .update(cx, |i, cx| i.set_context(view, summary, cx));
        self.inspector_default.clone().into()
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.frames += 1;
        log::debug!("shell frame {}", self.frames);
        let colors = *ui_theme::colors(cx);
        let title_bar = crate::chrome::title_bar(self, window, cx);
        let body = crate::chrome::body(self, window, cx);
        let status_bar = crate::chrome::status_bar(self, cx);
        let overlay = crate::chrome::overlay(self, cx);
        let toast = crate::chrome::toast(self, cx);

        let mut context = KeyContext::new_with_defaults();
        context.add(keymap::SHELL);
        div()
            .id("shell")
            .test_support()
            .key_context(context)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(ui_theme::fonts::UI_FONT_FAMILY)
            .text_size(Tokens::text_md())
            .on_drag_move(cx.listener(Self::on_splitter_drag))
            .on_action(
                cx.listener(|this, _: &ShowSources, w, cx| this.show_view(ViewId::Sources, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowLibrary, w, cx| {
                this.show_library(LibraryMode::Grid, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowLoupe, w, cx| {
                this.show_library(LibraryMode::Loupe, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowDuplicates, w, cx| {
                this.show_view(ViewId::Duplicates, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ShowExcluded, w, cx| {
                    this.show_view(ViewId::Excluded, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ShowAiReview, w, cx| {
                    this.show_view(ViewId::AiReview, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ShowExport, w, cx| this.show_view(ViewId::Export, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowReports, w, cx| this.show_view(ViewId::Reports, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowPreviewHelp, w, cx| {
                this.show_selected_preview_help(w, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleInspector, w, cx| {
                this.toggle_panel(Side::Inspector, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleNavigator, w, cx| {
                this.toggle_panel(Side::Navigator, w, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, w, cx| {
                this.open_palette(w, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, w, cx| {
                this.open_settings(w, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, w, cx| this.close_overlay(w, cx)))
            .on_action(cx.listener(|this, _: &AddSource, w, cx| this.add_source(w, cx)))
            .on_action(cx.listener(|this, _: &Rescan, w, cx| this.rescan(w, cx)))
            .on_action(cx.listener(|this, _: &ResumeScan, w, cx| this.resume(w, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(1.0), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(Some(-1.0), cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom(None, cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &Quit, w, cx| {
                this.save_now(w, cx);
                cx.quit();
            }))
            .child(title_bar)
            .child(body)
            .child(status_bar)
            .children(overlay)
            .children(toast)
    }
}
