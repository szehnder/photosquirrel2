//! The Library (grid and loupe) and Excluded views of the `gallery` change, and the shared gallery state
//! the inspector reads ([`LibraryHub`]).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{Icon, Sizable as _, ThemeStyled as _, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{
    AiKind, Disposition, ExcludedListing, ExclusionCategory, ExclusionCounts, ExclusionSelection,
    Filter, ItemId, MediaKind, SceneCategory, Sort, SortField,
};
use ui_gallery::loupe::LoupeEvent;
use ui_gallery::model::GalleryEvent;
use ui_gallery::source::{ExcludedRow, GalleryQuery, InsightCriteria};
use ui_gallery::{GalleryGrid, GalleryModel, LoupeView};
use ui_theme::Tokens;

use crate::ShellGlobals;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::library_backend::{excluded_filter, library_filter};
use crate::persist::{ExcludedMode, ExcludedViewState};
use crate::shell::Shell;
use crate::views::{LibraryMode, ViewId, ViewRegistry};
use crate::widgets::eyebrow;

/// How often the Library re-queries while a scan runs (design: at most every 2 s).
pub const LIVE_REFRESH: Duration = Duration::from_secs(2);

/// The gallery state shared by the Library view and the inspector (one per app; the app has one window).
#[derive(Default)]
pub struct LibraryHub {
    pub model: Option<Entity<GalleryModel>>,
    pub library: Option<WeakEntity<LibraryView>>,
    pub excluded: Option<WeakEntity<ExcludedView>>,
    pub inspector: Option<WeakEntity<crate::gallery_inspector::GalleryInspector>>,
}

impl Global for LibraryHub {}

impl LibraryHub {
    /// The gallery model, created on first use over the shell's gallery source.
    pub fn model(cx: &mut App) -> Entity<GalleryModel> {
        if let Some(m) = cx.default_global::<LibraryHub>().model.clone() {
            return m;
        }
        let source = cx.global::<ShellGlobals>().gallery.clone();
        let deadline = crate::startup::StartupGrace::deadline(cx);
        let query = StartupFilters::get(cx).query();
        let model = cx.new(|cx| {
            let mut m = GalleryModel::new(source, query.clone());
            // The default cells are small enough for half-size (192 px) textures: a quarter of the upload.
            let primed = deadline.is_some_and(|d| m.prime(d, 60, 1, cx));
            log::debug!("gallery primed: {primed}");
            if !primed {
                m.set_query(query, cx);
            }
            m
        });
        cx.default_global::<LibraryHub>().model = Some(model.clone());
        model
    }

    pub fn excluded(cx: &App) -> Option<Entity<ExcludedView>> {
        cx.try_global::<LibraryHub>()?.excluded.as_ref()?.upgrade()
    }

    pub fn inspector(cx: &App) -> Option<Entity<crate::gallery_inspector::GalleryInspector>> {
        cx.try_global::<LibraryHub>()?.inspector.as_ref()?.upgrade()
    }

    pub fn library(cx: &App) -> Option<Entity<LibraryView>> {
        cx.try_global::<LibraryHub>()?.library.as_ref()?.upgrade()
    }
}

/// Filters the Library starts with (`--gallery-filter`, scripted screenshots of the filter bar); none by
/// default.
#[derive(Clone, Debug, Default)]
pub struct StartupFilters(pub FilterBar);

impl Global for StartupFilters {}

impl StartupFilters {
    pub fn set(cx: &mut App, filters: FilterBar) {
        cx.set_global(StartupFilters(filters));
    }

    pub fn get(cx: &App) -> FilterBar {
        cx.try_global::<StartupFilters>()
            .map(|f| f.0.clone())
            .unwrap_or_default()
    }
}

pub fn default_query() -> GalleryQuery {
    GalleryQuery {
        filter: library_filter(),
        sort: Sort::default(),
        stacks: true,
        ..GalleryQuery::default()
    }
}

actions!(
    excluded,
    [
        /// The next file of the Excluded list (Down, J).
        NextRow,
        /// The previous file (Up, K).
        PrevRow,
        /// Extend the selection to the next file (Shift+Down).
        ExtendNext,
        /// Extend the selection to the previous file (Shift+Up).
        ExtendPrev,
        /// The first file (Home).
        FirstRow,
        /// The last file (End).
        LastRow,
        /// Select every file shown (Ctrl/Cmd+A).
        SelectAllRows,
        /// Put the selected files back in the Library (P).
        PutBackSelection,
        /// Put back every file shown, after a confirmation (Shift+P).
        AskPutBackAll,
        /// Confirm "Put back all shown" (Enter).
        ConfirmPutBack,
        /// Cancel the confirmation, else clear the selection (Escape).
        CancelOrClear,
    ]
);

/// Key context of the Excluded list.
pub const EXCLUDED_LIST_CONTEXT: &str = "ExcludedList";

/// The Excluded list's keys: the arrows (and J/K, as in Duplicates) move, Shift extends, single letters act on
/// the selection, Enter confirms and Escape cancels a pending confirmation (as Duplicates' keys do).
fn bind_excluded_keys(cx: &mut App) {
    let c = Some(EXCLUDED_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", NextRow, c),
        KeyBinding::new("j", NextRow, c),
        KeyBinding::new("up", PrevRow, c),
        KeyBinding::new("k", PrevRow, c),
        KeyBinding::new("shift-down", ExtendNext, c),
        KeyBinding::new("shift-up", ExtendPrev, c),
        KeyBinding::new("home", FirstRow, c),
        KeyBinding::new("end", LastRow, c),
        KeyBinding::new("secondary-a", SelectAllRows, c),
        KeyBinding::new("p", PutBackSelection, c),
        KeyBinding::new("shift-p", AskPutBackAll, c),
        KeyBinding::new("enter", ConfirmPutBack, c),
        KeyBinding::new("escape", CancelOrClear, c),
    ]);
}

/// Register the Library and Excluded views and the gallery inspector.
pub fn register(cx: &mut App) {
    bind_excluded_keys(cx);
    ViewRegistry::register(cx, ViewId::Library, |ctx, window, cx| {
        if let Some(v) = LibraryHub::library(cx) {
            v.update(cx, |v, cx| v.set_mode(ctx.library_mode, window, cx));
            return v.into();
        }
        let model = LibraryHub::model(cx);
        let view = cx.new(|cx| LibraryView::new(model, ctx.library_mode, window, cx));
        cx.default_global::<LibraryHub>().library = Some(view.downgrade());
        view.into()
    });
    ViewRegistry::register(cx, ViewId::Excluded, |ctx, window, cx| {
        let source = cx.global::<ShellGlobals>().gallery.clone();
        let state = ctx.excluded.clone();
        let view = cx.new(|cx| ExcludedView::new(source, state, window, cx));
        cx.default_global::<LibraryHub>().excluded = Some(view.downgrade());
        view.into()
    });
    ViewRegistry::on_library_mode(cx, |mode, window, cx| {
        if let Some(v) = LibraryHub::library(cx) {
            v.update(cx, |v, cx| v.set_mode(mode, window, cx));
        }
    });
    ViewRegistry::set_inspector(cx, |ctx, _, window, cx| {
        let model = LibraryHub::model(cx);
        let fallback = ctx.default_inspector.clone();
        let view = cx
            .new(|cx| crate::gallery_inspector::GalleryInspector::new(model, fallback, window, cx));
        cx.default_global::<LibraryHub>().inspector = Some(view.downgrade());
        view.into()
    });
}

/// The filter bar's state.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FilterBar {
    pub sort: Sort,
    pub kind: Option<MediaKind>,
    pub camera: Option<String>,
    pub gps: Option<bool>,
    pub stacks_off: bool,
    /// AI insight filters (effective insight: the user's corrections win).
    pub insight: InsightCriteria,
    /// Completion-report dispositions (any of them; empty: no disposition filter).
    pub dispositions: Vec<Disposition>,
    /// Only items without a preview, of this reason and format when given (ADR-0109).
    pub no_preview: Option<ps_model::NoPreviewFilter>,
}

impl FilterBar {
    /// Parse a comma-separated filter spec (`--gallery-filter`): `children`, `pets`, `people`,
    /// `category=<name>`, a disposition (`exported`, `covered`, `excluded`, `pending`, `unreadable`,
    /// `missing`), `photos`, `videos`, `no-stacks`. Unknown words are an error.
    pub fn parse(spec: &str) -> Result<FilterBar, String> {
        let mut f = FilterBar::default();
        for word in spec.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            match word {
                "children" => f.insight.children = true,
                "pets" => f.insight.pets = true,
                "people" => f.insight.people = true,
                "photos" => f.kind = Some(MediaKind::Image),
                "videos" => f.kind = Some(MediaKind::Video),
                "no-stacks" => f.stacks_off = true,
                "no-preview" => f.no_preview = Some(ps_model::NoPreviewFilter::default()),
                w => {
                    if let Some(spec) = w.strip_prefix("no-preview=") {
                        // `no-preview=<reason>` or `no-preview=<reason>:<format>`.
                        let (reason, format) = spec.split_once(':').unwrap_or((spec, ""));
                        f.no_preview = Some(ps_model::NoPreviewFilter {
                            reason: Some(
                                ps_model::PreviewReason::from_key(reason).ok_or_else(|| {
                                    format!("unknown placeholder reason {reason}")
                                })?,
                            ),
                            format: (!format.is_empty())
                                .then(|| {
                                    ps_model::Format::from_storage(format)
                                        .ok_or_else(|| format!("unknown format {format}"))
                                })
                                .transpose()?,
                        });
                    } else if let Some(name) = w.strip_prefix("category=") {
                        let c = SceneCategory::ALL
                            .into_iter()
                            .find(|c| {
                                crate::views::ai_review::category_label(*c)
                                    .eq_ignore_ascii_case(name)
                            })
                            .ok_or_else(|| format!("unknown category {name}"))?;
                        f.insight.category = Some(c);
                    } else if let Some(d) = Disposition::ALL.into_iter().find(|d| d.as_str() == w) {
                        f = f.toggle_disposition(d);
                    } else {
                        return Err(format!("unknown gallery filter {w}"));
                    }
                }
            }
        }
        Ok(f)
    }

    /// With `d` added to the disposition filter, or removed when it is in it.
    pub fn toggle_disposition(mut self, d: Disposition) -> FilterBar {
        if let Some(i) = self.dispositions.iter().position(|x| *x == d) {
            self.dispositions.remove(i);
        } else {
            self.dispositions.push(d);
            self.dispositions.sort();
        }
        self
    }

    /// The AI and export-status filters in force, each with the bar it leaves when removed.
    pub fn active_extras(&self) -> Vec<(String, String, FilterBar)> {
        let mut out = Vec::new();
        let i = &self.insight;
        for (on, id, label, clear) in [
            (i.children, "children", "Contains children", 0),
            (i.pets, "pets", "Contains pets", 1),
            (i.people, "people", "Contains people", 2),
        ] {
            if on {
                let mut next = self.clone();
                match clear {
                    0 => next.insight.children = false,
                    1 => next.insight.pets = false,
                    _ => next.insight.people = false,
                }
                out.push((id.to_owned(), label.to_owned(), next));
            }
        }
        if let Some(c) = i.category {
            let mut next = self.clone();
            next.insight.category = None;
            out.push((
                "category".to_owned(),
                format!("Category: {}", crate::views::ai_review::category_label(c)),
                next,
            ));
        }
        for d in &self.dispositions {
            out.push((
                d.as_str().to_owned(),
                format!("Status: {}", crate::views::kit::disposition_label(*d)),
                self.clone().toggle_disposition(*d),
            ));
        }
        if let Some(np) = &self.no_preview {
            let mut next = self.clone();
            next.no_preview = None;
            out.push(("no-preview".to_owned(), no_preview_label(np), next));
        }
        out
    }

    pub fn query(&self) -> GalleryQuery {
        GalleryQuery {
            filter: Filter {
                kind: self.kind,
                camera_model: self.camera.clone(),
                has_gps: self.gps,
                no_preview: self.no_preview.clone(),
                ..library_filter()
            },
            sort: self.sort,
            stacks: !self.stacks_off,
            insight: self.insight.clone(),
            dispositions: self.dispositions.clone(),
        }
    }
}

pub struct LibraryView {
    model: Entity<GalleryModel>,
    grid: Entity<GalleryGrid>,
    loupe: Entity<LoupeView>,
    mode: LibraryMode,
    pub filters: FilterBar,
    cameras: Vec<(String, u64)>,
    /// Placeholders by format and reason (the No preview menu and the Fill in previews notice, ADR-0109).
    previews: Vec<ps_model::PreviewStatusRow>,
    /// Fill in previews was started from here: the notice stays hidden until the counts are read again.
    filling: bool,
    live_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The Fill in previews notice was dismissed for this session (ADR-0109).
#[derive(Default)]
pub struct PreviewNoticeDismissed(pub bool);

impl Global for PreviewNoticeDismissed {}

/// "No preview: needs a library · HEIC".
pub fn no_preview_label(np: &ps_model::NoPreviewFilter) -> String {
    let mut s = match np.reason {
        Some(r) => format!("No preview: {}", r.label()),
        None => "No preview".to_owned(),
    };
    if let Some(f) = np.format {
        s.push_str(" · ");
        s.push_str(f.label());
    }
    s
}

/// The notice's text: "37 TIFF files can get previews now" (formats with fillable placeholders, most first);
/// `None` when nothing can be filled in.
pub fn fill_notice_text(rows: &[ps_model::PreviewStatusRow]) -> Option<String> {
    let mut by_format: Vec<(String, u64)> = Vec::new();
    for r in rows.iter().filter(|r| r.fillable > 0) {
        let label = ps_model::Format::from_storage(&r.format)
            .map_or_else(|| r.format.to_uppercase(), |f| f.label().to_owned());
        match by_format.iter_mut().find(|(l, _)| *l == label) {
            Some((_, n)) => *n += r.fillable,
            None => by_format.push((label, r.fillable)),
        }
    }
    if by_format.is_empty() {
        return None;
    }
    by_format.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total: u64 = by_format.iter().map(|(_, n)| n).sum();
    let parts: Vec<String> = by_format
        .iter()
        .map(|(l, n)| format!("{} {l}", format_count(*n)))
        .collect();
    let list = match parts.as_slice() {
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
        [] => String::new(),
    };
    Some(format!(
        "{list} {} can get previews now",
        if total == 1 { "file" } else { "files" }
    ))
}

impl LibraryView {
    pub fn new(
        model: Entity<GalleryModel>,
        mode: LibraryMode,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let grid = cx.new(|cx| GalleryGrid::new(model.clone(), cx));
        let loupe = cx.new(|cx| LoupeView::new(model.clone(), cx));
        let subs = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &model,
                window,
                |this, _, e: &GalleryEvent, window, cx| match e {
                    GalleryEvent::OpenLoupe(i) => this.open_loupe(*i, window, cx),
                    GalleryEvent::PlaceholderInfo(id) => open_preview_help(*id, window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &loupe,
                window,
                |this, _, e: &LoupeEvent, window, cx| match e {
                    LoupeEvent::Close(i) => this.close_loupe(*i, window, cx),
                    LoupeEvent::PlaceholderInfo(id) => open_preview_help(*id, window, cx),
                    LoupeEvent::Moved(_) => {}
                },
            ),
        ];
        let mut this = Self {
            model,
            grid,
            loupe,
            mode: LibraryMode::Grid,
            filters: StartupFilters::get(cx),
            cameras: Vec::new(),
            previews: Vec::new(),
            filling: false,
            live_task: None,
            _subscriptions: subs,
        };
        this.load_cameras(cx);
        this.load_previews(cx);
        this.start_live_updates(window, cx);
        if let Some(i) = crate::startup::StartupGrace::select(cx) {
            this.model.update(cx, |m, cx| {
                if i < m.len() {
                    let ids = m.index.ids.clone();
                    m.selection.click(&ids, i);
                    m.selection_changed(cx);
                }
            });
        }
        this.set_mode(mode, window, cx);
        this
    }

    pub fn grid(&self) -> &Entity<GalleryGrid> {
        &self.grid
    }

    pub fn loupe(&self) -> &Entity<LoupeView> {
        &self.loupe
    }

    pub fn model(&self) -> &Entity<GalleryModel> {
        &self.model
    }

    pub fn mode(&self) -> LibraryMode {
        self.mode
    }

    pub fn cameras(&self) -> &[(String, u64)] {
        &self.cameras
    }

    fn load_cameras(&mut self, cx: &mut Context<'_, Self>) {
        let source = self.model.read(cx).source().clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.camera_models().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let cams = task.await;
            let _ = this.update(cx, |this, cx| {
                this.cameras = cams;
                cx.notify();
            });
        })
        .detach();
    }

    /// Read the placeholder counts again (start-up, and after a scan or a Fill in previews job ends).
    pub fn load_previews(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(deadline) = crate::startup::StartupGrace::deadline(cx) {
            // Within the start-up grace (scripted screenshots): on a helper thread, waited for until the deadline.
            let source = self.model.read(cx).source().clone();
            let rows = crate::startup::StartupGrace::within(deadline, move || {
                source.preview_status().unwrap_or_default()
            });
            if let Some(rows) = rows {
                self.previews = rows;
                return;
            }
        }
        let source = self.model.read(cx).source().clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.preview_status().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                this.previews = rows;
                this.filling = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Placeholders by format and reason, as last read.
    pub fn previews(&self) -> &[ps_model::PreviewStatusRow] {
        &self.previews
    }

    /// The Fill in previews notice's text, when it is shown.
    pub fn fill_notice(&self, cx: &App) -> Option<String> {
        if self.filling
            || cx
                .try_global::<PreviewNoticeDismissed>()
                .is_some_and(|d| d.0)
        {
            return None;
        }
        fill_notice_text(&self.previews)
    }

    /// Fill in previews of every waiting format (the notice's button).
    pub fn fill_in_previews(&mut self, cx: &mut Context<'_, Self>) {
        let source = self.model.read(cx).source().clone();
        self.filling = true;
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = source.fill_previews(ps_model::FillScope::all()) {
                    log::warn!("fill in previews: {e:#}");
                }
            })
            .detach();
        cx.notify();
    }

    /// A Fill in previews job was started elsewhere (the dialog): hide the notice until the counts change.
    pub fn fill_started(&mut self, cx: &mut Context<'_, Self>) {
        self.filling = true;
        cx.notify();
    }

    /// Dismiss the notice for this session.
    pub fn dismiss_fill_notice(&mut self, cx: &mut Context<'_, Self>) {
        cx.set_global(PreviewNoticeDismissed(true));
        cx.notify();
    }

    /// Show only the items without a preview of `reason` and `format` ("Show them" in the dialog): the No
    /// preview filter on an otherwise unfiltered Library.
    pub fn show_no_preview(
        &mut self,
        reason: Option<ps_model::PreviewReason>,
        format: Option<ps_model::Format>,
        cx: &mut Context<'_, Self>,
    ) {
        let f = FilterBar {
            sort: self.filters.sort,
            no_preview: Some(ps_model::NoPreviewFilter { reason, format }),
            ..FilterBar::default()
        };
        self.set_filters(f, cx);
    }

    /// While the shell reports activity (a scan), re-query every 2 s keeping the top item in place; when the
    /// activity ends, read the placeholder counts again.
    fn start_live_updates(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.live_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut was_active = false;
            loop {
                cx.background_executor().timer(LIVE_REFRESH).await;
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        let scanning = Shell::for_window(window, cx)
                            .is_some_and(|s| s.read(cx).activity_snapshot().is_some());
                        if scanning {
                            this.grid.update(cx, |g, cx| g.refresh_keeping_anchor(cx));
                        } else if was_active {
                            this.load_previews(cx);
                        }
                        was_active = scanning;
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    /// Apply a filter bar change.
    pub fn set_filters(&mut self, filters: FilterBar, cx: &mut Context<'_, Self>) {
        if filters == self.filters {
            return;
        }
        self.filters = filters;
        let q = self.filters.query();
        self.model.update(cx, |m, cx| m.set_query(q, cx));
        self.grid.update(cx, |g, cx| g.set_scroll(0.0, cx));
        cx.notify();
    }

    pub fn set_mode(&mut self, mode: LibraryMode, window: &mut Window, cx: &mut Context<'_, Self>) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        match mode {
            LibraryMode::Loupe => {
                let (focus, len) = {
                    let m = self.model.read(cx);
                    (m.selection.focus, m.len())
                };
                let start = focus.unwrap_or_else(|| self.grid.read(cx).top_index(cx).unwrap_or(0));
                if len > 0 {
                    let grace = crate::startup::StartupGrace::remaining(cx).is_some();
                    self.loupe.update(cx, |l, cx| {
                        l.open(start, cx);
                        if grace {
                            l.prime(cx);
                        }
                    });
                }
                let f = self.loupe.read(cx).focus_handle().clone();
                window.focus(&f, cx);
            }
            LibraryMode::Grid => {
                let f = self.grid.read(cx).focus_handle().clone();
                window.focus(&f, cx);
            }
        }
        cx.notify();
    }

    fn open_loupe(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            m.selection.click(&ids, index);
            m.selection_changed(cx);
        });
        window.defer(cx, |window, cx| {
            if let Some(shell) = Shell::for_window(window, cx) {
                shell.update(cx, |s, cx| s.show_library(LibraryMode::Loupe, window, cx));
            }
        });
        self.set_mode(LibraryMode::Loupe, window, cx);
    }

    fn close_loupe(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            m.selection.click(&ids, index);
            m.selection_changed(cx);
        });
        self.set_mode(LibraryMode::Grid, window, cx);
        self.grid.update(cx, |g, cx| g.reveal(index, cx));
        window.defer(cx, |window, cx| {
            if let Some(shell) = Shell::for_window(window, cx) {
                shell.update(cx, |s, cx| s.show_library(LibraryMode::Grid, window, cx));
            }
        });
    }

    fn toolbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let m = self.model.read(cx);
        let count = m.len();
        let f = self.filters.clone();
        // The navigator counts files; with stacks on and nothing filtered, say how many the items stand for.
        let unfiltered = f.camera.is_none()
            && f.gps.is_none()
            && f.insight == InsightCriteria::default()
            && f.dispositions.is_empty()
            && f.no_preview.is_none();
        let files = match (f.stacks_off, unfiltered, f.kind) {
            (false, true, None) => cx.try_global::<LibraryTotals>().map_or(0, |t| t.files),
            _ => 0,
        };
        let loading = matches!(m.state, ui_gallery::model::ListState::Loading);
        let sort_label = match (f.sort.field, f.sort.descending) {
            (SortField::CaptureDate, false) => "Oldest first",
            (SortField::CaptureDate, true) => "Newest first",
            (SortField::FileName, false) => "Name A–Z",
            (SortField::FileName, true) => "Name Z–A",
            (SortField::FileSize, false) => "Smallest first",
            (SortField::FileSize, true) => "Largest first",
            (SortField::Folder | SortField::Category, _) => "Name A–Z",
        };
        let weak = cx.weak_entity();
        let sort_menu = {
            let weak = weak.clone();
            let f = f.clone();
            Button::new("library-sort")
                .label(sort_label)
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (label, field, desc) in [
                        ("Capture date · oldest first", SortField::CaptureDate, false),
                        ("Capture date · newest first", SortField::CaptureDate, true),
                        ("File name · A–Z", SortField::FileName, false),
                        ("File name · Z–A", SortField::FileName, true),
                        ("File size · largest first", SortField::FileSize, true),
                        ("File size · smallest first", SortField::FileSize, false),
                    ] {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.sort = Sort {
                            field,
                            descending: desc,
                        };
                        let on = f.sort == next.sort;
                        menu = menu.item(PopupMenuItem::new(label).checked(on).on_click(
                            move |_, _, cx| {
                                let next = next.clone();
                                let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                            },
                        ));
                    }
                    menu
                })
        };
        let camera_menu = {
            let weak = weak.clone();
            let f = f.clone();
            let cams = self.cameras.clone();
            Button::new("library-camera")
                .label(f.camera.clone().unwrap_or_else(|| "Camera".into()))
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    let mut any = f.clone();
                    any.camera = None;
                    {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Any camera")
                                .checked(f.camera.is_none())
                                .on_click(move |_, _, cx| {
                                    let any = any.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filters(any, cx));
                                }),
                        );
                    }
                    menu = menu.separator();
                    for (name, n) in &cams {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.camera = Some(name.clone());
                        let on = f.camera.as_deref() == Some(name.as_str());
                        menu = menu.item(
                            PopupMenuItem::new(format!("{name}  ·  {}", format_count(*n)))
                                .checked(on)
                                .on_click(move |_, _, cx| {
                                    let next = next.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                                }),
                        );
                    }
                    menu
                })
        };
        let ai_menu = {
            let weak = weak.clone();
            let f = f.clone();
            let n = f
                .active_extras()
                .iter()
                .filter(|(id, _, _)| {
                    matches!(id.as_str(), "children" | "pets" | "people" | "category")
                })
                .count();
            Button::new("library-ai")
                .label(match n {
                    0 => "AI insight".to_owned(),
                    n => format!("AI insight ({n})"),
                })
                .icon(Icon::new(IconName::Sparkles))
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.label("Contains");
                    let toggles: [(&'static str, fn(&mut InsightCriteria) -> &mut bool); 3] = [
                        ("Children", |c| &mut c.children),
                        ("Pets", |c| &mut c.pets),
                        ("People", |c| &mut c.people),
                    ];
                    for (label, field) in toggles {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        let on = *field(&mut next.insight);
                        *field(&mut next.insight) = !on;
                        menu = menu.item(PopupMenuItem::new(label).checked(on).on_click(
                            move |_, _, cx| {
                                let next = next.clone();
                                let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                            },
                        ));
                    }
                    menu = menu.separator().label("Category");
                    let any = std::iter::once(None).chain(SceneCategory::ALL.into_iter().map(Some));
                    for c in any {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.insight.category = c;
                        let label =
                            c.map_or("Any category", crate::views::ai_review::category_label);
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(f.insight.category == c)
                                .on_click(move |_, _, cx| {
                                    let next = next.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                                }),
                        );
                    }
                    menu
                })
        };
        let status_menu = {
            let weak = weak.clone();
            let f = f.clone();
            Button::new("library-status")
                .label(match f.dispositions.as_slice() {
                    [] => "Export status".to_owned(),
                    [d] => crate::views::kit::disposition_label(*d).to_owned(),
                    many => format!("Export status ({})", many.len()),
                })
                .icon(Icon::new(IconName::FolderOutput))
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.dispositions.clear();
                        menu = menu.item(
                            PopupMenuItem::new("Any status")
                                .checked(f.dispositions.is_empty())
                                .on_click(move |_, _, cx| {
                                    let next = next.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                                }),
                        );
                    }
                    menu = menu.separator();
                    for d in Disposition::ALL {
                        let weak = weak.clone();
                        let next = f.clone().toggle_disposition(d);
                        menu = menu.item(
                            PopupMenuItem::new(crate::views::kit::disposition_label(d))
                                .checked(f.dispositions.contains(&d))
                                .on_click(move |_, _, cx| {
                                    let next = next.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                                }),
                        );
                    }
                    menu
                })
        };
        let preview_menu = {
            let weak = weak.clone();
            let f = f.clone();
            let rows = self.previews.clone();
            let total: u64 = rows.iter().map(|r| r.count).sum();
            Button::new("library-no-preview")
                .label(match &f.no_preview {
                    None => "Preview".to_owned(),
                    Some(np) => no_preview_label(np),
                })
                .icon(Icon::new(IconName::Image))
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    let item = |label: String, on: bool, np: Option<ps_model::NoPreviewFilter>| {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.no_preview = np;
                        PopupMenuItem::new(label)
                            .checked(on)
                            .on_click(move |_, _, cx| {
                                let next = next.clone();
                                let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                            })
                    };
                    menu = menu.item(item("Any".into(), f.no_preview.is_none(), None));
                    let all = ps_model::NoPreviewFilter::default();
                    menu = menu.item(item(
                        format!("No preview  ·  {}", format_count(total)),
                        f.no_preview.as_ref() == Some(&all),
                        Some(all),
                    ));
                    if !rows.is_empty() {
                        menu = menu.separator();
                    }
                    for r in &rows {
                        let np = ps_model::NoPreviewFilter {
                            reason: Some(r.reason),
                            format: ps_model::Format::from_storage(&r.format),
                        };
                        let on = f.no_preview.as_ref() == Some(&np);
                        menu = menu.item(item(
                            format!("{}  ·  {}", no_preview_label(&np), format_count(r.count)),
                            on,
                            Some(np),
                        ));
                    }
                    menu
                })
        };
        let chip = |id: &'static str, label: &'static str, on: bool, next: FilterBar| {
            let weak = weak.clone();
            div()
                .id(id)
                .test_support()
                .role(Role::Button)
                .aria_label(label)
                .flex_none()
                .whitespace_nowrap()
                .px(Tokens::space(2.0))
                .h(Tokens::control_height())
                .flex()
                .items_center()
                .rounded(Tokens::RADIUS_MD)
                .text_size(Tokens::text_sm())
                .cursor_pointer()
                .text_color(if on { colors.text } else { colors.text_muted })
                .when(on, |d| {
                    d.bg(colors.accent_soft).text_color(colors.accent_text)
                })
                .hover(|d| d.bg(colors.hover))
                .on_click(move |_, _, cx| {
                    let next = next.clone();
                    let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                })
                .child(label)
        };
        let with = |g: &dyn Fn(&mut FilterBar)| {
            let mut n = f.clone();
            g(&mut n);
            n
        };
        let count_label = if loading {
            "Loading…".to_owned()
        } else {
            let noun = match f.kind {
                Some(MediaKind::Video) => "videos",
                Some(MediaKind::Image) => "photos",
                None => "items",
            };
            if files > count as u64 {
                format!(
                    "{} {noun} · {} files",
                    format_count(count as u64),
                    format_count(files)
                )
            } else {
                format!("{} {noun}", format_count(count as u64))
            }
        };
        h_flex()
            .id("library-toolbar")
            .test_support()
            .flex_none()
            .min_h(Tokens::size(44.0))
            .py(Tokens::space(1.0))
            .px(Tokens::space(4.0))
            .gap(Tokens::space(1.0))
            .items_center()
            .bg(colors.background)
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .id("library-count")
                    .test_support()
                    .aria_label(SharedString::from(count_label.clone()))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .mr(Tokens::space(1.5))
                    .text_size(Tokens::text_sm())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child(count_label),
            )
            // The filters take what is left and wrap onto more lines when narrow (inspector open), so the count
            // and the sort menu always stay visible. They wrap cluster by cluster (kind; camera, location and
            // stacks; AI and export status): clusters are separated by a wider gap instead of divider lines,
            // which a wrap could leave alone at the start or end of a line.
            .child(
                h_flex()
                    .id("library-filter-chips")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .flex_wrap()
                    .items_center()
                    .gap_x(Tokens::space(FILTER_CLUSTER_GAP))
                    .gap_y(Tokens::space(1.0))
                    .child(
                        filter_cluster("library-filters-kind")
                            .child(chip(
                                "filter-all",
                                "All",
                                f.kind.is_none(),
                                with(&|n| n.kind = None),
                            ))
                            .child(chip(
                                "filter-photos",
                                "Photos",
                                f.kind == Some(MediaKind::Image),
                                with(&|n| n.kind = Some(MediaKind::Image)),
                            ))
                            .child(chip(
                                "filter-videos",
                                "Videos",
                                f.kind == Some(MediaKind::Video),
                                with(&|n| n.kind = Some(MediaKind::Video)),
                            )),
                    )
                    .child(
                        filter_cluster("library-filters-meta")
                            .child(camera_menu)
                            .child(chip(
                                "filter-gps",
                                "Has location",
                                f.gps == Some(true),
                                with(&|n| {
                                    n.gps = if n.gps == Some(true) {
                                        None
                                    } else {
                                        Some(true)
                                    }
                                }),
                            ))
                            .child(chip(
                                "filter-stacks",
                                "Stacks",
                                !f.stacks_off,
                                with(&|n| n.stacks_off = !n.stacks_off),
                            )),
                    )
                    .child(
                        filter_cluster("library-filters-ai")
                            .child(ai_menu)
                            .child(status_menu)
                            .child(preview_menu),
                    ),
            )
            .child(div().flex_shrink_0().child(sort_menu))
            .into_any_element()
    }
}

/// The gap between the toolbar's filter clusters, in `Tokens::space` units; within a cluster it is
/// [`FILTER_GAP`].
pub const FILTER_CLUSTER_GAP: f32 = 3.0;
/// The gap between the filters of one cluster, in `Tokens::space` units.
pub const FILTER_GAP: f32 = 1.0;

/// One cluster of Library filters: on one line when it fits, so the toolbar wraps between clusters; never wider
/// than the filter area (a very narrow centre wraps inside the cluster too).
fn filter_cluster(id: &'static str) -> impl ParentElement + IntoElement + Styled {
    h_flex()
        .id(id)
        .test_support()
        .flex_none()
        .max_w_full()
        .flex_wrap()
        .items_center()
        .gap(Tokens::space(FILTER_GAP))
}

impl LibraryView {
    /// The AI and export-status filters in force, as removable chips under the toolbar.
    fn active_filters(&self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let extras = self.filters.active_extras();
        if extras.is_empty() {
            return None;
        }
        let colors = *ui_theme::colors(cx);
        let weak = cx.weak_entity();
        let mut row = h_flex()
            .id("library-active-filters")
            .test_support()
            .flex_none()
            .h(Tokens::size(36.0))
            .px(Tokens::space(4.0))
            .gap(Tokens::space(1.5))
            .items_center()
            .bg(colors.background)
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .mr(Tokens::space(1.0))
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child("Filtered by"),
            );
        for (id, label, next) in extras {
            let weak = weak.clone();
            let label = SharedString::from(label);
            row = row.child(
                h_flex()
                    .id(SharedString::from(format!("library-active-{id}")))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(SharedString::from(format!("Remove {label}")))
                    .h(Tokens::size(24.0))
                    .pl(Tokens::space(2.5))
                    .pr(Tokens::space(1.5))
                    .gap(Tokens::space(1.0))
                    .items_center()
                    .rounded(Tokens::RADIUS_LG)
                    .border_1()
                    .border_color(colors.accent)
                    .bg(colors.accent_soft)
                    .text_color(colors.accent_text)
                    .text_size(Tokens::text_xs())
                    .cursor_pointer()
                    .hover(|d| d.opacity(0.85))
                    .on_click(move |_, _, cx| {
                        let next = next.clone();
                        let _ = weak.update(cx, |v, cx| v.set_filters(next, cx));
                    })
                    .child(label)
                    .child(Icon::new(IconName::X).size(Tokens::icon_sm())),
            );
        }
        let clear = {
            let mut next = self.filters.clone();
            next.insight = InsightCriteria::default();
            next.dispositions.clear();
            next.no_preview = None;
            next
        };
        row = row.child(
            div()
                .id("library-clear-filters")
                .test_support()
                .role(Role::Button)
                .aria_label("Clear AI and status filters")
                .ml(Tokens::space(1.0))
                .text_size(Tokens::text_xs())
                .text_color(colors.text_muted)
                .cursor_pointer()
                .hover(|d| d.text_color(colors.text))
                .on_click(move |_, _, cx| {
                    let clear = clear.clone();
                    let _ = weak.update(cx, |v, cx| v.set_filters(clear, cx));
                })
                .child("Clear"),
        );
        if self.filters.insight != InsightCriteria::default() {
            row = row.child(div().flex_1()).child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child("AI filters use your corrections; they are suggestions, not decisions"),
            );
        }
        Some(row.into_any_element())
    }
}

impl Render for LibraryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let body: AnyElement = match self.mode {
            LibraryMode::Grid => {
                let m = self.model.read(cx);
                if m.state == ui_gallery::model::ListState::Ready && m.is_empty() {
                    empty_state(cx).into_any_element()
                } else {
                    self.grid.clone().into_any_element()
                }
            }
            LibraryMode::Loupe => self.loupe.clone().into_any_element(),
        };
        v_flex()
            .id("view-library")
            .test_support()
            .size_full()
            .bg(colors.gallery)
            .when(self.mode == LibraryMode::Grid, |d| {
                let active = self.active_filters(cx);
                let notice = self.preview_notice(cx);
                d.child(self.toolbar(cx)).children(active).children(notice)
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// Open "Why no preview?" for `id` in this window's shell (deferred: the caller may be updating a view).
pub fn open_preview_help(id: ItemId, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, cx| {
        if let Some(shell) = Shell::for_window(window, cx) {
            shell.update(cx, |s, cx| {
                s.open_preview_help(id, window, cx);
            });
        }
    });
}

impl LibraryView {
    /// "37 TIFF files can get previews now · Fill in previews · Dismiss" (ADR-0109).
    fn preview_notice(&self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let text = self.fill_notice(cx)?;
        let colors = *ui_theme::colors(cx);
        Some(
            h_flex()
                .id("library-preview-notice")
                .test_support()
                .aria_label(SharedString::from(text.clone()))
                .flex_none()
                .w_full()
                .min_h(Tokens::size(40.0))
                .px(Tokens::space(4.0))
                .py(Tokens::space(1.0))
                .gap(Tokens::space(2.0))
                .items_center()
                .bg(ui_theme::Colors::chip_bg(colors.success))
                .border_b_1()
                .border_color(colors.border)
                .child(
                    Icon::new(IconName::RefreshCw)
                        .size(Tokens::icon_sm())
                        .text_color(colors.tone_text(colors.success)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text)
                        .child(text),
                )
                .child(crate::views::kit::button(
                    "library-preview-fill",
                    "Fill in previews",
                    Some(IconName::RefreshCw),
                    crate::views::kit::Tone::Primary,
                    crate::views::kit::on(cx.listener(|this, _, _, cx| this.fill_in_previews(cx))),
                    cx,
                ))
                .child(crate::views::kit::button(
                    "library-preview-dismiss",
                    "Dismiss",
                    None,
                    crate::views::kit::Tone::Ghost,
                    crate::views::kit::on(
                        cx.listener(|this, _, _, cx| this.dismiss_fill_notice(cx)),
                    ),
                    cx,
                ))
                .into_any_element(),
        )
    }
}

fn empty_state(cx: &App) -> impl IntoElement {
    let colors = ui_theme::colors(cx);
    v_flex()
        .id("library-empty")
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(Tokens::space(2.0))
        .child(
            Icon::new(IconName::Images)
                .size(Tokens::icon_lg())
                .text_color(colors.text_faint),
        )
        .child(
            div()
                .text_size(Tokens::text_md())
                .text_color(colors.text_muted)
                .child("Nothing matches these filters"),
        )
}

// ---- Excluded view ---------------------------------------------------------------------------------

/// Photos and videos in the Library (the navigator's count), from the shell's catalog summary.
#[derive(Clone, Copy, Debug, Default)]
pub struct LibraryTotals {
    pub files: u64,
}

impl gpui_kit::Global for LibraryTotals {}

/// Width of the Excluded list's File column (name and path, both shortened in the middle).
const EXCLUDED_FILE_COLUMN: f32 = 360.0;
/// Width of the Excluded list's Category column.
const EXCLUDED_CATEGORY_COLUMN: f32 = 150.0;
/// Height of an Excluded list row (a [`Tokens::thumb_sm`] thumbnail and its padding).
const EXCLUDED_ROW_HEIGHT: f32 = 44.0;
/// Rows of the Excluded list read per page (only pages that come on screen are read, ADR-0103).
pub const EXCLUDED_PAGE: usize = 128;
/// Rows kept in the Excluded list's page cache; the oldest pages are dropped beyond it.
const EXCLUDED_ROWS_KEPT: usize = 4096;
/// The search box applies what was typed after this pause.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// Putting back more files than this shows a progress toast.
pub const PUT_BACK_TOAST_ABOVE: usize = 10_000;

/// A category's chip label: "Thumbnail caches · 12".
pub fn chip_label(label: &str, n: u64) -> String {
    format!("{label} · {}", format_count(n))
}

/// The Excluded view (ADR-0102): one chip per exclusion category with live counts ("not a supported file" off
/// until asked for), a search box, a sort menu, List and Grid modes, and put back for the selection or for every
/// file shown. It reads the counts and the ordered id list in one pass and the rows a page at a time as they
/// come on screen, like the Library grid.
pub struct ExcludedView {
    source: std::sync::Arc<dyn ui_gallery::source::GallerySource>,
    /// The excluded list, its selection and thumbnails (the Grid mode's model; the List mode reads it too).
    model: Entity<GalleryModel>,
    grid: Entity<GalleryGrid>,
    state: ExcludedViewState,
    search: Entity<InputState>,
    /// The applied search text (after the debounce).
    text: String,
    search_task: Option<Task<()>>,
    counts: Option<ExclusionCounts>,
    loaded: bool,
    request: u64,
    rows: HashMap<ItemId, std::rc::Rc<ExcludedRow>>,
    /// Pages read or being read, by list generation; oldest first.
    pages: std::collections::VecDeque<((u64, usize), Vec<ItemId>)>,
    pending: HashSet<(u64, usize)>,
    /// Page reads so far (the paging test counts them).
    pub page_reads: usize,
    /// "Put back all shown" waits for this confirmation: the exact files it will put back (the list as it was
    /// when asked, or as it became while the confirmation was open; the confirm button carries this set).
    confirm_all: Option<std::rc::Rc<Vec<ItemId>>>,
    /// Each listed file's category, by list position (the Grid's cell labels).
    tags: std::rc::Rc<std::cell::RefCell<Vec<(ItemId, ExclusionCategory, Option<AiKind>)>>>,
    /// The List mode's keyboard focus (arrows, Shift, select all, put back).
    list_focus: FocusHandle,
    /// The category chips' focus handles (tab stops; Space or Enter toggles).
    chip_focus: Vec<(ExclusionCategory, FocusHandle)>,
    /// Files were put back but an accepted AI suggestion among them could not be marked rejected.
    pub warning: Option<String>,
    /// Files being put back.
    busy: Option<usize>,
    /// The file the inspector shows: the last one the user selected, kept after it leaves the list (a put back),
    /// so the inspector goes on showing it with its new reason.
    inspected: Option<ItemId>,
    pub last_error: Option<String>,
    scroll: UniformListScrollHandle,
    live_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ExcludedView {
    pub fn new(
        source: std::sync::Arc<dyn ui_gallery::source::GallerySource>,
        state: ExcludedViewState,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let mut state = state;
        state.chips.text = None;
        let query = Self::query_for(&state, "");
        let model = cx.new(|_| GalleryModel::new(source.clone(), query.clone()));
        let grid = cx.new(|cx| GalleryGrid::new(model.clone(), cx));
        let tags: std::rc::Rc<
            std::cell::RefCell<Vec<(ItemId, ExclusionCategory, Option<AiKind>)>>,
        > = Default::default();
        {
            let tags = tags.clone();
            grid.update(cx, |g, cx| {
                g.set_cell_label(
                    Some(Box::new(move |i, id| {
                        let tags = tags.borrow();
                        tags.get(i)
                            .filter(|t| t.0 == id)
                            .map(|t| SharedString::from(category_badge(t.1, t.2)))
                    })),
                    cx,
                )
            });
        }
        let chip_focus = ExclusionCategory::ALL
            .into_iter()
            .filter(|c| *c != ExclusionCategory::Ai)
            .map(|c| (c, cx.focus_handle().tab_stop(true)))
            .collect();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search name or folder"));
        let subs = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&model, |this, model, e: &GalleryEvent, cx| {
                if matches!(e, GalleryEvent::SelectionChanged) {
                    let m = model.read(cx);
                    let focused = m
                        .selection
                        .focus
                        .and_then(|i| m.index.id(i))
                        .filter(|id| m.selection.contains(*id))
                        .or_else(|| m.selected_ids(1).first().copied());
                    if focused.is_some() {
                        this.inspected = focused;
                    }
                    cx.notify();
                }
            }),
            cx.subscribe(&search, |this, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.search_changed(cx);
                }
            }),
        ];
        let mut this = Self {
            source,
            model,
            grid,
            state,
            search,
            text: String::new(),
            search_task: None,
            counts: None,
            loaded: false,
            request: 0,
            rows: HashMap::new(),
            pages: Default::default(),
            pending: HashSet::new(),
            page_reads: 0,
            confirm_all: None,
            tags,
            list_focus: cx.focus_handle(),
            chip_focus,
            warning: None,
            busy: None,
            inspected: None,
            last_error: None,
            scroll: UniformListScrollHandle::new(),
            live_task: None,
            _subscriptions: subs,
        };
        // Start-up grace (a scripted launch straight into this view): the counts, the list, the first rows and
        // their thumbnails before the first frame, on a helper thread that the UI waits for at most until the
        // grace ends.
        let primed = crate::startup::StartupGrace::deadline(cx)
            .is_some_and(|deadline| this.prime(query, deadline, cx));
        if !primed {
            this.reload(cx);
        }
        this.start_live_updates(window, cx);
        this
    }

    fn query_for(state: &ExcludedViewState, text: &str) -> GalleryQuery {
        let mut chips = state.chips.clone();
        let text = text.trim();
        chips.text = (!text.is_empty()).then(|| text.to_owned());
        GalleryQuery {
            filter: Filter {
                exclusion: Some(chips),
                ..Filter::default()
            },
            sort: state.sort,
            stacks: false,
            ..GalleryQuery::default()
        }
    }

    /// The query behind the list: the chips, the search text and the sort.
    pub fn query(&self) -> GalleryQuery {
        Self::query_for(&self.state, &self.text)
    }

    fn prime(
        &mut self,
        query: GalleryQuery,
        deadline: std::time::Instant,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let source = self.source.clone();
        let rows = 40;
        let job = move || -> anyhow::Result<_> {
            let listing = source.excluded_open(&query)?;
            let first: Vec<ItemId> = listing
                .list
                .ids
                .iter()
                .take(EXCLUDED_PAGE)
                .copied()
                .collect();
            let page = source.excluded_page(&first)?;
            let summaries = source.summaries(&first)?;
            let mut thumbs = Vec::new();
            for r in page.iter().take(rows) {
                if let Some(t) = r.thumb
                    && let Ok(Some(bytes)) = source.thumbnail(&t)
                    && let Ok(bgra) = ui_gallery::decode::decode_jpeg_bgra(&bytes, 1)
                {
                    thumbs.push((t, bgra));
                }
            }
            Ok((query, listing, first, page, summaries, thumbs))
        };
        let Some(Ok((query, listing, first, page, summaries, thumbs))) =
            crate::startup::StartupGrace::within(deadline, job)
        else {
            return false;
        };
        self.install(query, listing, cx);
        let generation = self.model.read(cx).index.generation;
        self.insert_page((generation, 0), first, page);
        self.model.update(cx, |m, _| {
            m.insert_summaries(summaries);
            for (thumb, bgra) in thumbs {
                if let Some(image) = ui_gallery::model::render_image(bgra) {
                    m.insert_primed_texture(ui_gallery::model::TexKey { thumb, level: 1 }, image);
                }
            }
        });
        true
    }

    /// Read the counts and the list again (a new filter, a decision, a scan's progress), keeping the rows shown
    /// until their pages are read again.
    pub fn reload(&mut self, cx: &mut Context<'_, Self>) {
        self.request += 1;
        let request = self.request;
        let query = self.query();
        let source = self.source.clone();
        let q = query.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.excluded_open(&q).map_err(|e| format!("{e:#}")) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                match result {
                    Ok(listing) => this.install(query, listing, cx),
                    Err(e) => {
                        this.last_error = Some(e);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn install(
        &mut self,
        query: GalleryQuery,
        listing: ExcludedListing,
        cx: &mut Context<'_, Self>,
    ) {
        let ExcludedListing {
            counts,
            list,
            categories,
        } = listing;
        self.counts = Some(counts);
        self.loaded = true;
        self.pending.clear();
        self.pages.clear();
        // A pending "Put back all shown" stays while the list is the same (a scan's 2 s refresh reads it again).
        // When the list changed, the confirmation shows the new list and its count, and only a confirmation of
        // that list puts it back: the button carries the set it showed.
        if let Some(pending) = &self.confirm_all
            && pending.as_slice() != list.ids.as_slice()
        {
            self.confirm_all = (!list.ids.is_empty()).then(|| std::rc::Rc::new(list.ids.clone()));
        }
        *self.tags.borrow_mut() = list
            .ids
            .iter()
            .zip(categories)
            .map(|(id, (c, k))| (*id, c, k))
            .collect();
        let first = self.inspected.is_none();
        self.model.update(cx, |m, cx| {
            m.set_query_and_list(query, list, cx);
            // With nothing selected yet the first file is, so the inspector shows a file and its reasons.
            if first && m.selection.is_empty() && !m.is_empty() {
                let ids = m.index.ids.clone();
                m.selection.click(&ids, 0);
                m.selection_changed(cx);
            }
        });
        // The inspector reads this on its next render. Set it here, not only from the selection event (delivered
        // after this update): a scripted launch into the view primes it inside the start-up grace, and the inspector
        // reads the details then, so its first frame shows them instead of "Loading…".
        if first {
            self.inspected = self.model.read(cx).selected_ids(1).first().copied();
        }
        cx.notify();
    }

    /// While the shell reports activity (a scan), read the counts and the list again every 2 s.
    fn start_live_updates(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.live_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(LIVE_REFRESH).await;
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        let scanning = Shell::for_window(window, cx)
                            .is_some_and(|s| s.read(cx).activity_snapshot().is_some());
                        if scanning {
                            this.reload(cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    // ---- state ---------------------------------------------------------------------------------------

    pub fn state(&self) -> &ExcludedViewState {
        &self.state
    }

    pub fn counts(&self) -> Option<&ExclusionCounts> {
        self.counts.as_ref()
    }

    pub fn model(&self) -> &Entity<GalleryModel> {
        &self.model
    }

    pub fn grid(&self) -> &Entity<GalleryGrid> {
        &self.grid
    }

    pub fn search_input(&self) -> &Entity<InputState> {
        &self.search
    }

    /// The list has been read at least once.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Files the chips hide (the search text applies to both sides).
    pub fn hidden(&self) -> u64 {
        self.counts
            .as_ref()
            .map_or(0, |c| c.hidden(&self.state.chips))
    }

    /// Change the chips, the sort or the mode; remembered per catalog.
    pub fn set_state(
        &mut self,
        state: ExcludedViewState,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let mut state = state;
        state.chips.text = None;
        if state == self.state {
            return;
        }
        let relist = state.chips != self.state.chips || state.sort != self.state.sort;
        let mode = state.mode != self.state.mode;
        self.state = state.clone();
        if relist {
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.grid.update(cx, |g, cx| g.set_scroll(0.0, cx));
            self.reload(cx);
        }
        if mode && state.mode == ExcludedMode::Grid {
            let f = self.grid.read(cx).focus_handle().clone();
            window.focus(&f, cx);
        } else if mode {
            window.focus(&self.list_focus, cx);
        }
        window.defer(cx, move |window, cx| {
            if let Some(shell) = Shell::for_window(window, cx) {
                shell.update(cx, |s, cx| s.set_excluded_state(state, window, cx));
            }
        });
        cx.notify();
    }

    fn with_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut ExcludedViewState),
    ) {
        let mut next = self.state.clone();
        f(&mut next);
        self.set_state(next, window, cx);
    }

    /// Turn a category's chip on or off (the AI chip turns every kind).
    pub fn set_category(
        &mut self,
        c: ExclusionCategory,
        on: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.with_state(window, cx, |s| {
            s.chips = s.chips.clone().with_category(c, on)
        });
    }

    /// Turn one AI kind on or off.
    pub fn set_ai_kind(
        &mut self,
        k: AiKind,
        on: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.with_state(window, cx, |s| {
            s.chips = s.chips.clone().with_ai_kind(k, on)
        });
    }

    /// Every chip on ("Show all").
    pub fn show_all(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.with_state(window, cx, |s| s.chips = ExclusionSelection::all());
    }

    pub fn set_sort(&mut self, sort: Sort, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.with_state(window, cx, |s| s.sort = sort);
    }

    pub fn set_mode(
        &mut self,
        mode: ExcludedMode,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.with_state(window, cx, |s| s.mode = mode);
    }

    fn search_changed(&mut self, cx: &mut Context<'_, Self>) {
        let text = self.search.read(cx).value().to_string();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.apply_search(text, cx));
        }));
    }

    /// Apply a search text now (the search box does this after its debounce).
    pub fn apply_search(&mut self, text: String, cx: &mut Context<'_, Self>) {
        if text.trim() == self.text.trim() {
            return;
        }
        self.text = text;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.grid.update(cx, |g, cx| g.set_scroll(0.0, cx));
        self.reload(cx);
    }

    // ---- rows ----------------------------------------------------------------------------------------

    /// The loaded row of the file at `index`, if its page has been read.
    pub fn row(&self, index: usize, cx: &App) -> Option<std::rc::Rc<ExcludedRow>> {
        let id = self.model.read(cx).index.id(index)?;
        self.rows.get(&id).cloned()
    }

    /// Every loaded row of the list, in list order (tests; rows whose page has not been read are left out).
    pub fn loaded_rows(&self, cx: &App) -> Vec<ExcludedRow> {
        let m = self.model.read(cx);
        m.index
            .ids
            .iter()
            .filter_map(|id| self.rows.get(id).map(|r| (**r).clone()))
            .collect()
    }

    /// The ids the list shows, in order.
    pub fn ids(&self, cx: &App) -> Vec<ItemId> {
        self.model.read(cx).index.ids.to_vec()
    }

    fn insert_page(&mut self, key: (u64, usize), ids: Vec<ItemId>, rows: Vec<ExcludedRow>) {
        for r in rows {
            self.rows.insert(r.id, std::rc::Rc::new(r));
        }
        self.pages.push_back((key, ids));
        let kept: usize = self.pages.iter().map(|(_, ids)| ids.len()).sum();
        if kept > EXCLUDED_ROWS_KEPT
            && let Some((_, old)) = self.pages.pop_front()
        {
            let keep: HashSet<ItemId> = self
                .pages
                .iter()
                .flat_map(|(_, ids)| ids.iter().copied())
                .collect();
            for id in old {
                if !keep.contains(&id) {
                    self.rows.remove(&id);
                }
            }
        }
    }

    /// Make sure the pages covering `range` are read or on their way.
    pub fn ensure_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<'_, Self>) {
        if range.is_empty() {
            return;
        }
        let (generation, len) = {
            let m = self.model.read(cx);
            (m.index.generation, m.len())
        };
        for page in range.start / EXCLUDED_PAGE..=(range.end - 1) / EXCLUDED_PAGE {
            let key = (generation, page);
            if self.pending.contains(&key) || self.pages.iter().any(|(k, _)| *k == key) {
                continue;
            }
            let start = page * EXCLUDED_PAGE;
            let end = ((page + 1) * EXCLUDED_PAGE).min(len);
            if start >= end {
                continue;
            }
            let ids: Vec<ItemId> = self.model.read(cx).index.ids[start..end].to_vec();
            self.pending.insert(key);
            self.page_reads += 1;
            let source = self.source.clone();
            let task = cx.background_executor().spawn(async move {
                let r = source.excluded_page(&ids);
                (ids, r)
            });
            cx.spawn(async move |this, cx| {
                let (ids, result) = task.await;
                let _ = this.update(cx, |this, cx| {
                    if !this.pending.remove(&key) {
                        return;
                    }
                    match result {
                        Ok(rows) => this.insert_page(key, ids, rows),
                        Err(e) => log::warn!("excluded: page {page} failed: {e:#}"),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    // ---- selection and put back ----------------------------------------------------------------------

    /// The file the inspector shows: the one last selected, even after it left the list.
    pub fn selected_id(&self, _: &App) -> Option<ItemId> {
        self.inspected
    }

    /// Selected files, in list order.
    pub fn selected_ids(&self, cx: &App) -> Vec<ItemId> {
        self.model.read(cx).selected_ids(usize::MAX)
    }

    /// Click semantics on a list row (Shift extends, Ctrl/Cmd toggles).
    pub fn click(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<'_, Self>) {
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            if modifiers.shift {
                m.selection.select_range(&ids, ix, modifiers.secondary());
            } else if modifiers.secondary() {
                m.selection.toggle(&ids, ix);
            } else {
                m.selection.click(&ids, ix);
            }
            m.selection_changed(cx);
        });
        cx.notify();
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        self.click(ix, Modifiers::none(), cx);
    }

    /// The List mode's focus handle (arrows, Shift+arrows, select all, P, Shift+P, Enter, Escape).
    pub fn list_focus(&self) -> &FocusHandle {
        &self.list_focus
    }

    /// The focus handle of a category's chip (every chip but the AI menu, which is a button).
    pub fn chip_focus(&self, c: ExclusionCategory) -> Option<&FocusHandle> {
        self.chip_focus
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, f)| f)
    }

    /// Move the keyboard focus to `to` (clamped), selecting it, or extending the selection from the anchor.
    pub fn move_to(&mut self, to: isize, extend: bool, cx: &mut Context<'_, Self>) {
        let len = self.model.read(cx).len();
        if len == 0 {
            return;
        }
        let ix = to.clamp(0, len as isize - 1) as usize;
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            if extend {
                if m.selection.anchor.is_none() {
                    m.selection.anchor = m.selection.focus.or(Some(ix));
                }
                m.selection.select_range(&ids, ix, false);
                m.selection.focus = Some(ix);
            } else {
                m.selection.click(&ids, ix);
            }
            m.selection_changed(cx);
        });
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Move the keyboard focus by `delta` rows (from nothing: the first row).
    pub fn step(&mut self, delta: isize, extend: bool, cx: &mut Context<'_, Self>) {
        let focus = self.model.read(cx).selection.focus;
        let to = match focus {
            Some(f) => f as isize + delta,
            None => 0,
        };
        self.move_to(to, extend, cx);
    }

    /// Select every file shown.
    pub fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            m.selection.select_all(&ids);
            m.selection_changed(cx);
        });
        cx.notify();
    }

    /// Escape: cancel a pending confirmation, else clear the selection.
    pub fn cancel_or_clear(&mut self, cx: &mut Context<'_, Self>) {
        if self.confirm_all.is_some() {
            self.cancel_put_back_all(cx);
            return;
        }
        self.model.update(cx, |m, cx| {
            m.selection.clear();
            m.selection_changed(cx);
        });
        cx.notify();
    }

    /// Ask before putting back every file shown.
    pub fn ask_put_back_all(&mut self, cx: &mut Context<'_, Self>) {
        let ids = self.ids(cx);
        if !ids.is_empty() {
            self.confirm_all = Some(std::rc::Rc::new(ids));
            cx.notify();
        }
    }

    /// The pending "Put back all shown" confirmation's file count.
    pub fn confirming(&self) -> Option<usize> {
        self.confirm_all.as_ref().map(|s| s.len())
    }

    /// The files the pending confirmation will put back.
    pub fn confirming_set(&self) -> Option<std::rc::Rc<Vec<ItemId>>> {
        self.confirm_all.clone()
    }

    pub fn cancel_put_back_all(&mut self, cx: &mut Context<'_, Self>) {
        self.confirm_all = None;
        cx.notify();
    }

    /// Confirm what the confirmation shows now.
    pub fn confirm_put_back_all(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(set) = self.confirm_all.clone() {
            self.confirm_put_back(&set, window, cx);
        }
    }

    /// Confirm `set`, the files a rendered confirmation showed: put back only while that is still the pending
    /// confirmation. A list that changed in between shows its own count and needs its own confirmation.
    pub fn confirm_put_back(
        &mut self,
        set: &std::rc::Rc<Vec<ItemId>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self
            .confirm_all
            .as_ref()
            .is_some_and(|p| std::rc::Rc::ptr_eq(p, set))
        {
            return;
        }
        self.confirm_all = None;
        self.put_back(set.to_vec(), window, cx);
    }

    pub fn put_back_selected(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let ids = self.selected_ids(cx);
        self.put_back(ids, window, cx);
    }

    fn put_back(&mut self, ids: Vec<ItemId>, window: &mut Window, cx: &mut Context<'_, Self>) {
        if ids.is_empty() || self.busy.is_some() {
            return;
        }
        if self.source.cleanup_running() {
            self.last_error = Some(crate::gallery_inspector::DECISIONS_WAIT.to_owned());
            cx.notify();
            return;
        }
        let n = ids.len();
        self.busy = Some(n);
        self.last_error = None;
        self.warning = None;
        if n > PUT_BACK_TOAST_ABOVE
            && let Some(shell) = Shell::for_window(window, cx)
        {
            shell.update(cx, |s, cx| {
                s.show_toast(
                    crate::shell::ToastKind::Info,
                    format!("Putting back {} files", format_count(n as u64)),
                    "They return to the Library as included by you.",
                    true,
                    cx,
                )
            });
        }
        let source = self.source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.put_back(&ids) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match r {
                    Ok(done) => {
                        if let Some(e) = done.rejection_error {
                            this.warning = Some(format!(
                                "Put back {} in the Library. The AI's suggestions for {} could not be marked \
                                 rejected ({e}), so the AI may suggest {} again.",
                                files(done.files as u64),
                                if done.files == 1 { "it" } else { "them" },
                                if done.files == 1 { "it" } else { "them" },
                            ));
                        }
                    }
                    Err(e) => this.last_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
            cx.update(refresh_after_decision);
        })
        .detach();
        cx.notify();
    }
}

impl ExcludedView {
    #[allow(clippy::too_many_arguments)]
    fn chip(
        &self,
        id: &'static str,
        category: ExclusionCategory,
        label: String,
        on: bool,
        window: &Window,
        cx: &mut Context<'_, Self>,
        toggle: impl Fn(&mut Self, &mut Window, &mut Context<'_, Self>) + 'static,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let label = SharedString::from(label);
        let focus = self.chip_focus(category).cloned();
        let focused = focus.as_ref().is_some_and(|f| f.is_focused(window));
        h_flex()
            .id(id)
            .test_support()
            .role(Role::CheckBox)
            .aria_label(label.clone())
            .when_some(focus, |d, f| d.track_focus(&f))
            .when(focused, |d| d.focus_ring_style(window, cx))
            .flex_none()
            .whitespace_nowrap()
            .h(Tokens::size(24.0))
            .px(Tokens::space(2.5))
            .gap(Tokens::space(1.0))
            .items_center()
            .rounded(Tokens::RADIUS_LG)
            .border_1()
            .text_size(Tokens::text_xs())
            .cursor_pointer()
            .when(on, |d| {
                d.bg(colors.accent_soft)
                    .border_color(colors.accent)
                    .text_color(colors.accent_text)
            })
            .when(!on, |d| {
                d.bg(colors.raised)
                    .border_color(colors.border)
                    .text_color(colors.text_muted)
                    .hover(|d| d.text_color(colors.text))
            })
            .on_click(cx.listener(move |this, _, window, cx| toggle(this, window, cx)))
            .when(on, |d| {
                d.child(Icon::new(IconName::Check).size(Tokens::icon_sm()))
            })
            .child(label)
            .into_any_element()
    }

    fn toolbar(&self, window: &Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let counts = self.counts.clone().unwrap_or_default();
        let chips = self.state.chips.clone();
        let n = |c: ExclusionCategory| counts.of(c);
        // The chips take the toolbar's width and wrap cluster by cluster (the search, sort and mode controls are
        // in the header row, the put back buttons in their own row below).
        let mut categories = h_flex()
            .id("excluded-chips")
            .test_support()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .items_center()
            .gap_x(Tokens::space(FILTER_CLUSTER_GAP))
            .gap_y(Tokens::space(1.0));
        // Clusters: yours and the AI's decisions; the classifier's; files PhotoSquirrel cannot read at all.
        let mut decided = filter_cluster("excluded-chips-decided").child(self.chip(
            "excluded-chip-you",
            ExclusionCategory::You,
            chip_label(ExclusionCategory::You.label(), n(ExclusionCategory::You)),
            chips.is_on(ExclusionCategory::You),
            window,
            cx,
            |this, w, cx| {
                let on = this.state.chips.is_on(ExclusionCategory::You);
                this.set_category(ExclusionCategory::You, !on, w, cx)
            },
        ));
        decided = decided.child(self.ai_menu(&counts, cx));
        let mut classifier = filter_cluster("excluded-chips-classifier");
        for (id, c) in [
            ("excluded-chip-cache", ExclusionCategory::Cache),
            ("excluded-chip-graphic", ExclusionCategory::Graphic),
            ("excluded-chip-small", ExclusionCategory::Small),
            ("excluded-chip-unreadable", ExclusionCategory::Unreadable),
        ] {
            classifier = classifier.child(self.chip(
                id,
                c,
                chip_label(c.label(), n(c)),
                chips.is_on(c),
                window,
                cx,
                move |this, w, cx| {
                    let on = this.state.chips.is_on(c);
                    this.set_category(c, !on, w, cx)
                },
            ));
        }
        let unsupported = filter_cluster("excluded-chips-unsupported").child(self.chip(
            "excluded-chip-unsupported",
            ExclusionCategory::Unsupported,
            chip_label(
                ExclusionCategory::Unsupported.label(),
                n(ExclusionCategory::Unsupported),
            ),
            chips.is_on(ExclusionCategory::Unsupported),
            window,
            cx,
            |this, w, cx| {
                let on = this.state.chips.is_on(ExclusionCategory::Unsupported);
                this.set_category(ExclusionCategory::Unsupported, !on, w, cx)
            },
        ));
        categories = categories
            .child(decided)
            .child(classifier)
            .child(unsupported);

        // The put back buttons have their own row under the chips, right-aligned, with the selection's size on
        // the left: they never sit between wrapped chip rows.
        let actions = self.actions(cx);
        v_flex()
            .id("excluded-toolbar")
            .test_support()
            .flex_none()
            .py(Tokens::space(1.5))
            .px(Tokens::space(4.0))
            .gap(Tokens::space(1.5))
            .bg(colors.background)
            .border_b_1()
            .border_color(colors.border)
            .child(categories)
            .child(actions)
            .into_any_element()
    }

    /// The AI suggestions chip: a menu of its kinds, each with its count.
    fn ai_menu(&self, counts: &ExclusionCounts, cx: &mut Context<'_, Self>) -> AnyElement {
        let chips = self.state.chips.clone();
        let total = counts.of(ExclusionCategory::Ai);
        let on: Vec<AiKind> = AiKind::ALL
            .into_iter()
            .filter(|k| chips.shows(ExclusionCategory::Ai, Some(*k)))
            .collect();
        let label = match on.as_slice() {
            [] => chip_label("AI suggestions", total),
            [k] if on.len() < AiKind::ALL.len() => {
                chip_label(&format!("AI: {}", k.label()), counts.ai(*k))
            }
            _ if on.len() == AiKind::ALL.len() => chip_label("AI suggestions", total),
            _ => chip_label(
                &format!("AI suggestions ({} of 4)", on.len()),
                on.iter().map(|k| counts.ai(*k)).sum(),
            ),
        };
        let counts = counts.clone();
        let weak = cx.weak_entity();
        let any = !on.is_empty();
        let colors = *ui_theme::colors(cx);
        // The same outlined chip as the others: accent outline and tint when on, a hairline when off.
        Button::new("excluded-chip-ai")
            .label(label)
            .icon(Icon::new(IconName::Sparkles).size(Tokens::icon_sm()))
            .compact()
            .h(Tokens::size(24.0))
            .px(Tokens::space(2.5))
            .rounded(Tokens::RADIUS_LG)
            .border_1()
            .text_size(Tokens::text_xs())
            .when(any, |b| {
                b.bg(colors.accent_soft)
                    .border_color(colors.accent)
                    .text_color(colors.accent_text)
            })
            .when(!any, |b| {
                b.bg(colors.raised)
                    .border_color(colors.border)
                    .text_color(colors.text_muted)
            })
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let all = AiKind::ALL
                    .iter()
                    .all(|k| chips.shows(ExclusionCategory::Ai, Some(*k)));
                let mut menu = {
                    let weak = weak.clone();
                    menu.item(
                        PopupMenuItem::new(chip_label(
                            "Every AI suggestion",
                            counts.of(ExclusionCategory::Ai),
                        ))
                        .checked(all)
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.set_category(ExclusionCategory::Ai, !all, window, cx)
                            });
                        }),
                    )
                    .separator()
                };
                for k in AiKind::ALL {
                    let weak = weak.clone();
                    let on = chips.shows(ExclusionCategory::Ai, Some(k));
                    menu = menu.item(
                        PopupMenuItem::new(chip_label(k.label(), counts.ai(k)))
                            .checked(on)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |v, cx| v.set_ai_kind(k, !on, window, cx));
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// The search box, the sort menu and the List/Grid toggle (wrapping as a group).
    fn controls(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let sort = self.state.sort;
        let sort_label = excluded_sort_label(sort);
        let weak = cx.weak_entity();
        let sort_menu = Button::new("excluded-sort")
            .label(sort_label)
            .compact()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (field, descending) in EXCLUDED_SORTS {
                    let next = Sort { field, descending };
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(excluded_sort_label(next))
                            .checked(next == sort)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |v, cx| v.set_sort(next, window, cx));
                            }),
                    );
                }
                menu
            });
        let weak = cx.weak_entity();
        let mode = crate::widgets::segmented(
            "excluded-mode",
            &["List", "Grid"],
            usize::from(self.state.mode == ExcludedMode::Grid),
            move |ix, window, cx| {
                let mode = if ix == 1 {
                    ExcludedMode::Grid
                } else {
                    ExcludedMode::List
                };
                let _ = weak.update(cx, |v, cx| v.set_mode(mode, window, cx));
            },
            cx,
        );
        let controls = h_flex()
            .id("excluded-controls")
            .test_support()
            .flex_none()
            .max_w_full()
            .flex_wrap()
            .items_center()
            .gap(Tokens::space(2.0))
            .child(
                div()
                    .id("excluded-search")
                    .test_support()
                    .flex_none()
                    .w(Tokens::size(200.0))
                    .max_w_full()
                    .child(
                        Input::new(&self.search)
                            .small()
                            .aria_label("Search name or folder"),
                    ),
            )
            .child(div().flex_none().child(sort_menu))
            .child(
                div()
                    .id("excluded-mode-toggle")
                    .test_support()
                    .flex_none()
                    .child(mode),
            );
        controls.into_any_element()
    }

    /// Put back the selection, or everything shown (after a confirmation): a row of its own, right-aligned, with
    /// the selection's size on the left.
    fn actions(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let shown = self.model.read(cx).len();
        let selected = self.selected_ids(cx).len();
        let busy = self.busy;
        let note = match (busy, selected) {
            (Some(n), _) => format!("Putting back {}…", files(n as u64)),
            (None, 0) => "Select files to put them back, or put back all shown".to_owned(),
            (None, n) => format!("{} selected", files(n as u64)),
        };
        h_flex()
            .id("excluded-actions")
            .test_support()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(Tokens::space(2.0))
            .child(
                div()
                    .id("excluded-selected")
                    .test_support()
                    .aria_label(SharedString::from(note.clone()))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_muted)
                    .child(note),
            )
            .child(crate::views::kit::button(
                "excluded-put-back",
                match (busy, selected) {
                    (Some(n), _) => format!("Putting back {}…", format_count(n as u64)),
                    (None, n) => put_back_label(n),
                },
                Some(IconName::Undo2),
                crate::views::kit::Tone::Secondary,
                (selected > 0 && busy.is_none()).then(|| {
                    let weak = cx.weak_entity();
                    std::rc::Rc::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = weak.update(cx, |v, cx| v.put_back_selected(window, cx));
                    }) as crate::views::kit::ClickHandler
                }),
                cx,
            ))
            .child(crate::views::kit::button(
                "excluded-put-back-all",
                "Put back all shown…",
                None,
                crate::views::kit::Tone::Ghost,
                (shown > 0 && busy.is_none()).then(|| {
                    let weak = cx.weak_entity();
                    std::rc::Rc::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = weak.update(cx, |v, cx| v.ask_put_back_all(cx));
                    }) as crate::views::kit::ClickHandler
                }),
                cx,
            ))
            .into_any_element()
    }

    /// The title, the count line, the hidden notice and the search, sort and mode controls.
    fn header(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let shown = self.model.read(cx).len();
        let hidden = self.hidden();
        let total = self.counts.as_ref().map_or(0, ExclusionCounts::total);
        let count = header_count(self.loaded, shown as u64, total, hidden, self.text.trim());
        h_flex()
            .id("excluded-header")
            .test_support()
            .py(Tokens::space(1.0))
            .flex_none()
            .min_h(Tokens::size(44.0))
            .px(Tokens::space(4.0))
            .gap(Tokens::space(2.0))
            .flex_wrap()
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex_none()
                    .text_size(Tokens::text_lg())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Excluded"),
            )
            .child(
                div()
                    .id("excluded-count")
                    .test_support()
                    .aria_label(SharedString::from(count.clone()))
                    .flex_none()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text_muted)
                    .child(count),
            )
            .when(hidden > 0, |d| {
                let label = format!("{} hidden by filters", format_count(hidden));
                d.child(
                    h_flex()
                        .flex_none()
                        .gap(Tokens::space(1.0))
                        .items_center()
                        .text_size(Tokens::text_sm())
                        .child(
                            div()
                                .id("excluded-hidden")
                                .test_support()
                                .aria_label(SharedString::from(label.clone()))
                                .text_color(colors.text_muted)
                                .child(format!("· {label}")),
                        )
                        .child(
                            div()
                                .id("excluded-show-all")
                                .test_support()
                                .role(Role::Button)
                                .aria_label("Show all")
                                .text_color(colors.accent_text)
                                .cursor_pointer()
                                .hover(|d| d.underline())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.show_all(window, cx)),
                                )
                                .child("Show all"),
                        ),
                )
            })
            .child(div().flex_1())
            .child(self.controls(cx))
            .into_any_element()
    }

    /// What "Put back all shown" will do, and its two buttons.
    fn confirmation(
        &self,
        set: std::rc::Rc<Vec<ItemId>>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let n = set.len();
        let what = describe_selection(&self.state.chips, &self.text);
        let weak = cx.weak_entity();
        let weak2 = weak.clone();
        let (they, them) = if n == 1 {
            ("It is", "it")
        } else {
            ("They are", "them")
        };
        h_flex()
            .id("excluded-confirm")
            .test_support()
            .flex_none()
            .flex_wrap()
            .px(Tokens::space(4.0))
            .py(Tokens::space(2.0))
            .gap(Tokens::space(2.0))
            .items_center()
            .bg(colors.accent_soft)
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text)
                    .child(format!(
                        "Put back {} ({what}) in the Library? {they} included by you from now on; rescans keep \
                         that, and you can exclude {them} again. Files are not touched.",
                        files(n as u64)
                    )),
            )
            .child(crate::views::kit::button(
                "excluded-confirm-put-back",
                format!("Put back {}", files(n as u64)),
                Some(IconName::Undo2),
                crate::views::kit::Tone::Primary,
                crate::views::kit::on(move |_, window, cx| {
                    let _ = weak.update(cx, |v, cx| v.confirm_put_back(&set, window, cx));
                }),
                cx,
            ))
            .child(crate::views::kit::button(
                "excluded-confirm-cancel",
                "Cancel",
                None,
                crate::views::kit::Tone::Ghost,
                crate::views::kit::on(move |_, _, cx| {
                    let _ = weak2.update(cx, |v, cx| v.cancel_put_back_all(cx));
                }),
                cx,
            ))
            .into_any_element()
    }

    fn columns(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        h_flex()
            .flex_none()
            .px(Tokens::space(4.0))
            .py(Tokens::space(1.5))
            .gap(Tokens::space(3.0))
            .border_b_1()
            .border_color(colors.border)
            .child(div().w(Tokens::thumb_sm()).flex_none())
            .child(
                div()
                    .w(Tokens::size(EXCLUDED_FILE_COLUMN))
                    .flex_none()
                    .child(eyebrow("File", cx)),
            )
            .child(
                div()
                    .w(Tokens::size(EXCLUDED_CATEGORY_COLUMN))
                    .flex_none()
                    .child(eyebrow("Category", cx)),
            )
            .child(div().flex_1().child(eyebrow("Reasons", cx)))
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        self.ensure_rows(range.clone(), cx);
        let visible: Vec<usize> = range.clone().collect();
        let rows: Vec<Option<std::rc::Rc<ExcludedRow>>> = {
            let m = self.model.read(cx);
            visible
                .iter()
                .map(|i| m.index.id(*i).and_then(|id| self.rows.get(&id).cloned()))
                .collect()
        };
        // Thumbnails through the gallery model's decode queue and texture cache (as the grid does).
        let textures: Vec<Option<std::sync::Arc<RenderImage>>> = self.model.update(cx, |m, cx| {
            m.request_pages(range.clone(), cx);
            m.want(&visible, &[], &[], 1, cx);
            visible
                .iter()
                .map(|i| {
                    m.summary(*i)
                        .and_then(|s| s.thumb)
                        .and_then(|t| m.any_texture(t))
                })
                .collect()
        });
        // Files with a placeholder show the same tile as the grid, at list size (ADR-0109).
        let placeholders: Vec<Option<(ps_model::PlaceholderBadge, bool)>> = {
            let m = self.model.read(cx);
            visible
                .iter()
                .map(|i| {
                    m.summary(*i)
                        .and_then(|s| s.placeholder.clone().map(|p| (p, s.video)))
                })
                .collect()
        };
        let colors = *ui_theme::colors(cx);
        let m = self.model.read(cx);
        let selected: Vec<bool> = visible
            .iter()
            .map(|i| m.index.id(*i).is_some_and(|id| m.selection.contains(id)))
            .collect();
        let mut out = Vec::with_capacity(visible.len());
        for (k, ix) in visible.into_iter().enumerate() {
            let row = rows[k].clone();
            let tex = textures[k].clone();
            let thumb = div()
                .id(SharedString::from(format!("excluded-thumb-{ix}")))
                .test_support()
                .aria_label(if tex.is_some() {
                    "thumbnail"
                } else {
                    "placeholder"
                })
                .flex_none()
                .size(Tokens::thumb_sm())
                .rounded(Tokens::RADIUS_SM)
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .child(match (&tex, &placeholders[k]) {
                    (None, Some((badge, video))) => ui_gallery::placeholder::placeholder_element(
                        SharedString::from(format!("excluded-placeholder-{ix}")),
                        badge,
                        *video,
                        Tokens::size(24.0),
                        ui_gallery::placeholder::Lod::Tiny,
                        cx,
                    ),
                    _ => crate::views::review_kit::photo(tex, false, cx).into_any_element(),
                });
            let (name, path, category, reasons) = match &row {
                Some(r) => (
                    crate::views::kit::middle_ellipsis(&r.name, 48),
                    crate::views::kit::middle_ellipsis(
                        &crate::views::kit::display_path(std::path::Path::new(&r.path)),
                        64,
                    ),
                    category_text(r.category, r.ai_kind),
                    r.reasons.join(" · "),
                ),
                None => ("…".to_owned(), String::new(), String::new(), String::new()),
            };
            let reasons = SharedString::from(reasons);
            let tip = reasons.clone();
            out.push(
                h_flex()
                    .id(SharedString::from(format!("excluded-row-{ix}")))
                    .test_support()
                    .w_full()
                    .h(Tokens::size(EXCLUDED_ROW_HEIGHT))
                    .px(Tokens::space(4.0))
                    .gap(Tokens::space(3.0))
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .when(selected[k], |d| d.bg(colors.accent_soft))
                    .hover(|d| d.bg(colors.hover))
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        window.focus(&this.list_focus, cx);
                        this.click(ix, e.modifiers(), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            let id = this.model.read(cx).index.id(ix);
                            let selected =
                                id.is_some_and(|id| this.model.read(cx).selection.contains(id));
                            if !selected {
                                this.select(ix, cx);
                            }
                        }),
                    )
                    .child(thumb)
                    .child(
                        v_flex()
                            .w(Tokens::size(EXCLUDED_FILE_COLUMN))
                            .flex_none()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(Tokens::text_sm())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_faint)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(path),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("excluded-category-{ix}")))
                            .test_support()
                            .aria_label(SharedString::from(category.clone()))
                            .w(Tokens::size(EXCLUDED_CATEGORY_COLUMN))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child(category),
                    )
                    .child(
                        // Long reasons are cut at the column edge; the tooltip has them in full.
                        div()
                            .id(SharedString::from(format!("excluded-reasons-{ix}")))
                            .flex_1()
                            .min_w_0()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            })
                            .child(reasons),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn empty(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let hidden = self.hidden();
        v_flex()
            .id("excluded-empty")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(Tokens::space(2.0))
            .child(
                Icon::new(IconName::EyeOff)
                    .size(Tokens::icon_lg())
                    .text_color(colors.text_faint),
            )
            .child(
                div()
                    .text_size(Tokens::text_md())
                    .text_color(colors.text_muted)
                    .child(if hidden > 0 || !self.text.trim().is_empty() {
                        "Nothing matches these filters"
                    } else {
                        "Nothing is set aside"
                    }),
            )
            .into_any_element()
    }
}

/// The Excluded view's sort choices, in menu order.
pub const EXCLUDED_SORTS: [(SortField, bool); 6] = [
    (SortField::FileName, false),
    (SortField::FileName, true),
    (SortField::Folder, false),
    (SortField::FileSize, true),
    (SortField::FileSize, false),
    (SortField::Category, false),
];

fn excluded_sort_label(s: Sort) -> &'static str {
    match (s.field, s.descending) {
        (SortField::FileName, false) => "Name A–Z",
        (SortField::FileName, true) => "Name Z–A",
        (SortField::Folder, false) => "Folder A–Z",
        (SortField::Folder, true) => "Folder Z–A",
        (SortField::FileSize, true) => "Largest first",
        (SortField::FileSize, false) => "Smallest first",
        (SortField::Category, _) => "Category",
        (SortField::CaptureDate, false) => "Oldest first",
        (SortField::CaptureDate, true) => "Newest first",
    }
}

/// "1 file", "12 files", "1,204 files".
pub fn files(n: u64) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{} files", format_count(n))
    }
}

/// The selection's put back label: "Put back in Library" for none or one file, "Put back 3 in Library".
pub fn put_back_label(selected: usize) -> String {
    if selected > 1 {
        format!("Put back {} in Library", format_count(selected as u64))
    } else {
        "Put back in Library".to_owned()
    }
}

/// The Excluded header's count line: "129 files set aside", "Showing 126 of 129 set aside" when chips hide
/// some, "3 files match “scan”" while searching.
pub fn header_count(loaded: bool, shown: u64, total: u64, hidden: u64, search: &str) -> String {
    if !loaded {
        "Loading…".to_owned()
    } else if !search.is_empty() {
        format!(
            "{} {} \u{201c}{search}\u{201d}",
            files(shown),
            if shown == 1 { "matches" } else { "match" }
        )
    } else if hidden > 0 {
        format!(
            "Showing {} of {} set aside",
            format_count(shown),
            format_count(total)
        )
    } else {
        format!("{} set aside", files(total))
    }
}

/// A file's category on its Grid tile, short ("AI: Document", "Cache", "Too small").
pub fn category_badge(c: ExclusionCategory, kind: Option<AiKind>) -> &'static str {
    match (c, kind) {
        (ExclusionCategory::You, _) => "By you",
        (ExclusionCategory::Ai, Some(AiKind::Screenshot)) => "AI: Screenshot",
        (ExclusionCategory::Ai, Some(AiKind::Document)) => "AI: Document",
        (ExclusionCategory::Ai, Some(AiKind::Graphic)) => "AI: Graphic",
        (ExclusionCategory::Ai, Some(AiKind::NotPhoto)) => "AI: Not a photo",
        (ExclusionCategory::Ai, None) => "AI",
        (ExclusionCategory::Cache, _) => "Cache",
        (ExclusionCategory::Graphic, _) => "Graphic",
        (ExclusionCategory::Small, _) => "Too small",
        (ExclusionCategory::Unreadable, _) => "Unreadable",
        (ExclusionCategory::Unsupported, _) => "Unsupported",
    }
}

/// A file's category as the list shows it ("AI: Documents", "Thumbnail caches").
pub fn category_text(c: Option<ExclusionCategory>, kind: Option<AiKind>) -> String {
    match (c, kind) {
        (Some(ExclusionCategory::Ai), Some(k)) => format!("AI: {}", k.label()),
        (Some(c), _) => c.label().to_owned(),
        (None, _) => "In the Library".to_owned(),
    }
}

/// The chips and search in words, for the put back confirmation ("AI: Documents, matching “scan”").
pub fn describe_selection(sel: &ExclusionSelection, text: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in ExclusionCategory::ALL {
        if c == ExclusionCategory::Ai {
            if sel.is_on(c) {
                parts.push(c.label().to_owned());
            } else {
                parts.extend(
                    AiKind::ALL
                        .into_iter()
                        .filter(|k| sel.shows(c, Some(*k)))
                        .map(|k| format!("AI: {}", k.label())),
                );
            }
        } else if sel.is_on(c) {
            parts.push(c.label().to_owned());
        }
    }
    let mut out = if parts.len() == ExclusionCategory::ALL.len() {
        "every category".to_owned()
    } else {
        parts.join(", ")
    };
    let text = text.trim();
    if !text.is_empty() {
        out.push_str(&format!(", matching \u{201c}{text}\u{201d}"));
    }
    out
}

impl Render for ExcludedView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let n = self.model.read(cx).len();
        let header = self.header(cx);
        let toolbar = self.toolbar(window, cx);
        let confirm = self
            .confirm_all
            .clone()
            .map(|set| self.confirmation(set, cx));
        let warning = self.warning.clone().map(|w| {
            div()
                .id("excluded-warning")
                .test_support()
                .aria_label(SharedString::from(w.clone()))
                .flex_none()
                .px(Tokens::space(4.0))
                .py(Tokens::space(1.5))
                .text_size(Tokens::text_sm())
                .text_color(colors.warning)
                .child(w)
        });
        let error = self.last_error.clone().map(|e| {
            div()
                .id("excluded-error")
                .test_support()
                .flex_none()
                .px(Tokens::space(4.0))
                .py(Tokens::space(1.5))
                .text_size(Tokens::text_sm())
                .text_color(colors.danger)
                .child(e)
        });
        let body: AnyElement = if self.loaded && n == 0 {
            self.empty(cx)
        } else {
            match self.state.mode {
                ExcludedMode::Grid => div()
                    .id("excluded-grid")
                    .test_support()
                    .size_full()
                    .bg(colors.gallery)
                    .child(self.grid.clone())
                    .into_any_element(),
                ExcludedMode::List => {
                    let list = uniform_list(
                        "excluded-rows",
                        n,
                        cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                            this.render_rows(range, cx)
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .flex_1();
                    let focused = self.list_focus.is_focused(window);
                    v_flex()
                        .id("excluded-list")
                        .test_support()
                        .key_context(EXCLUDED_LIST_CONTEXT)
                        .track_focus(&self.list_focus)
                        .on_action(cx.listener(|this, _: &NextRow, _, cx| this.step(1, false, cx)))
                        .on_action(cx.listener(|this, _: &PrevRow, _, cx| this.step(-1, false, cx)))
                        .on_action(
                            cx.listener(|this, _: &ExtendNext, _, cx| this.step(1, true, cx)),
                        )
                        .on_action(
                            cx.listener(|this, _: &ExtendPrev, _, cx| this.step(-1, true, cx)),
                        )
                        .on_action(
                            cx.listener(|this, _: &FirstRow, _, cx| this.move_to(0, false, cx)),
                        )
                        .on_action(cx.listener(|this, _: &LastRow, _, cx| {
                            this.move_to(isize::MAX, false, cx)
                        }))
                        .on_action(
                            cx.listener(|this, _: &SelectAllRows, _, cx| this.select_all(cx)),
                        )
                        .on_action(cx.listener(|this, _: &PutBackSelection, window, cx| {
                            this.put_back_selected(window, cx)
                        }))
                        .on_action(
                            cx.listener(|this, _: &AskPutBackAll, _, cx| this.ask_put_back_all(cx)),
                        )
                        .on_action(cx.listener(|this, _: &ConfirmPutBack, window, cx| {
                            this.confirm_put_back_all(window, cx)
                        }))
                        .on_action(
                            cx.listener(|this, _: &CancelOrClear, _, cx| this.cancel_or_clear(cx)),
                        )
                        .size_full()
                        .when(focused, |d| d.border_t_1().border_color(colors.accent))
                        .child(self.columns(cx))
                        .child(list)
                        .into_any_element()
                }
            }
        };
        let weak = cx.weak_entity();
        let _ = window;
        v_flex()
            .id("view-excluded")
            .test_support()
            .size_full()
            .bg(colors.background)
            .child(header)
            .child(toolbar)
            .children(confirm)
            .children(warning)
            .children(error)
            .child(
                div()
                    .id("excluded-body")
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .context_menu(move |menu, _, cx| {
                        // The same set the action puts back: the selection among the files shown.
                        let n = weak
                            .upgrade()
                            .map_or(0, |v| v.read(cx).selected_ids(cx).len());
                        let weak = weak.clone();
                        menu.item(
                            PopupMenuItem::new(put_back_label(n))
                                .disabled(n == 0)
                                .on_click(move |_, window, cx| {
                                    let _ =
                                        weak.update(cx, |v, cx| v.put_back_selected(window, cx));
                                }),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// Used by the inspector to refresh both views after a decision.
pub fn refresh_after_decision(cx: &mut App) {
    if let Some(m) = cx.try_global::<LibraryHub>().and_then(|h| h.model.clone()) {
        m.update(cx, |m, cx| m.refresh_keeping(None, cx));
    }
    if let Some(x) = LibraryHub::excluded(cx) {
        x.update(cx, |x, cx| x.reload(cx));
    }
    let _ = excluded_filter;
}
