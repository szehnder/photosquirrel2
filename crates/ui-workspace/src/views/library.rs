//! The Library (grid and loupe) and Excluded views of the `gallery` change, and the shared gallery state
//! the inspector reads ([`LibraryHub`]).

use std::time::Duration;

use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{Icon, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{Filter, MediaKind, Sort, SortField};
use ui_gallery::loupe::LoupeEvent;
use ui_gallery::model::GalleryEvent;
use ui_gallery::source::{ExcludedRow, GalleryQuery};
use ui_gallery::{GalleryGrid, GalleryModel, LoupeView};
use ui_theme::Tokens;

use crate::ShellGlobals;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::library_backend::{class_name, excluded_filter, library_filter};
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
        let model = cx.new(|cx| {
            let query = default_query();
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

pub fn default_query() -> GalleryQuery {
    GalleryQuery {
        filter: library_filter(),
        sort: Sort::default(),
        stacks: true,
    }
}

/// Register the Library and Excluded views and the gallery inspector.
pub fn register(cx: &mut App) {
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
    ViewRegistry::register(cx, ViewId::Excluded, |_, window, cx| {
        let source = cx.global::<ShellGlobals>().gallery.clone();
        let view = cx.new(|cx| ExcludedView::new(source, window, cx));
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
}

impl FilterBar {
    pub fn query(&self) -> GalleryQuery {
        GalleryQuery {
            filter: Filter {
                kind: self.kind,
                camera_model: self.camera.clone(),
                has_gps: self.gps,
                ..library_filter()
            },
            sort: self.sort,
            stacks: !self.stacks_off,
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
    live_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
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
            cx.subscribe_in(&model, window, |this, _, e: &GalleryEvent, window, cx| {
                if let GalleryEvent::OpenLoupe(i) = e {
                    this.open_loupe(*i, window, cx);
                }
            }),
            cx.subscribe_in(&loupe, window, |this, _, e: &LoupeEvent, window, cx| {
                if let LoupeEvent::Close(i) = e {
                    this.close_loupe(*i, window, cx);
                }
            }),
        ];
        let mut this = Self {
            model,
            grid,
            loupe,
            mode: LibraryMode::Grid,
            filters: FilterBar::default(),
            cameras: Vec::new(),
            live_task: None,
            _subscriptions: subs,
        };
        this.load_cameras(cx);
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

    /// While the shell reports activity (a scan), re-query every 2 s keeping the top item in place.
    fn start_live_updates(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.live_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(LIVE_REFRESH).await;
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        let scanning = Shell::for_window(window, cx)
                            .is_some_and(|s| s.read(cx).activity_snapshot().is_some());
                        if scanning {
                            this.grid.update(cx, |g, cx| g.refresh_keeping_anchor(cx));
                        }
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
        let loading = matches!(m.state, ui_gallery::model::ListState::Loading);
        let f = self.filters.clone();
        let sort_label = match (f.sort.field, f.sort.descending) {
            (SortField::CaptureDate, false) => "Oldest first",
            (SortField::CaptureDate, true) => "Newest first",
            (SortField::FileName, false) => "Name A–Z",
            (SortField::FileName, true) => "Name Z–A",
            (SortField::FileSize, false) => "Smallest first",
            (SortField::FileSize, true) => "Largest first",
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
                .label(f.camera.clone().unwrap_or_else(|| "Any camera".into()))
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
        let chip = |id: &'static str, label: &'static str, on: bool, next: FilterBar| {
            let weak = weak.clone();
            div()
                .id(id)
                .test_support()
                .role(Role::Button)
                .aria_label(label)
                .px(Tokens::space(2.5))
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
            format!("{} {noun}", format_count(count as u64))
        };
        h_flex()
            .id("library-toolbar")
            .flex_none()
            .h(Tokens::size(44.0))
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
                    .mr(Tokens::space(3.0))
                    .text_size(Tokens::text_sm())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child(count_label),
            )
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
            ))
            .child(
                div()
                    .w(Tokens::BORDER)
                    .h(Tokens::size(18.0))
                    .mx(Tokens::space(1.5))
                    .bg(colors.border),
            )
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
            ))
            .child(div().flex_1())
            .child(sort_menu)
            .into_any_element()
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
                d.child(self.toolbar(cx))
            })
            .child(div().flex_1().min_h_0().child(body))
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

pub struct ExcludedView {
    source: std::sync::Arc<dyn ui_gallery::source::GallerySource>,
    rows: Option<std::rc::Rc<Vec<ExcludedRow>>>,
    /// The selected row's item (kept by id across reloads).
    selected: Option<ps_model::ItemId>,
    scroll: UniformListScrollHandle,
}

impl ExcludedView {
    pub fn new(
        source: std::sync::Arc<dyn ui_gallery::source::GallerySource>,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let mut this = Self {
            source,
            rows: None,
            selected: None,
            scroll: UniformListScrollHandle::new(),
        };
        // Start-up grace (a scripted launch straight into this view): load before the first frame.
        if crate::startup::StartupGrace::remaining(cx).is_some()
            && let Ok(rows) = this.source.excluded()
        {
            this.rows = Some(std::rc::Rc::new(rows));
        } else {
            this.reload(cx);
        }
        this
    }

    pub fn reload(&mut self, cx: &mut Context<'_, Self>) {
        let source = self.source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.excluded().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                this.rows = Some(std::rc::Rc::new(rows));
                cx.notify();
            });
        })
        .detach();
    }

    pub fn rows(&self) -> Option<&[ExcludedRow]> {
        self.rows.as_ref().map(|r| r.as_slice())
    }

    pub fn selected(&self) -> Option<&ExcludedRow> {
        let id = self.selected?;
        self.rows.as_ref()?.iter().find(|r| r.id == id)
    }

    /// The item selected in this view, even after it left the list (the inspector keeps showing it).
    pub fn selected_id(&self) -> Option<ps_model::ItemId> {
        self.selected
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        self.selected = self.rows.as_ref().and_then(|r| r.get(ix)).map(|r| r.id);
        cx.notify();
    }
}

impl Render for ExcludedView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let rows = self.rows.clone();
        let n = rows.as_ref().map_or(0, |r| r.len());
        let selected = self
            .selected
            .and_then(|id| self.rows.as_ref()?.iter().position(|r| r.id == id));
        let weak = cx.weak_entity();
        let header = h_flex()
            .flex_none()
            .h(Tokens::size(44.0))
            .px(Tokens::space(4.0))
            .gap(Tokens::space(2.0))
            .items_baseline()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(Tokens::text_lg())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Excluded"),
            )
            .child(
                div()
                    .id("excluded-count")
                    .test_support()
                    .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_muted)
                    .child(match &rows {
                        Some(_) => format!("{} files set aside", format_count(n as u64)),
                        None => "Loading…".into(),
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child("Nothing is hidden silently: every file below has its reasons."),
            );
        let columns = h_flex()
            .flex_none()
            .px(Tokens::space(4.0))
            .py(Tokens::space(1.5))
            .gap(Tokens::space(3.0))
            .border_b_1()
            .border_color(colors.border)
            .child(div().w(Tokens::size(260.0)).child(eyebrow("File", cx)))
            .child(div().w(Tokens::size(120.0)).child(eyebrow("Class", cx)))
            .child(div().flex_1().child(eyebrow("Reasons", cx)));
        let list = uniform_list("excluded-rows", n, move |range, _, cx| {
            let colors = *ui_theme::colors(cx);
            let Some(rows) = rows.clone() else {
                return Vec::new();
            };
            range
                .map(|ix| {
                    let r = &rows[ix];
                    let weak = weak.clone();
                    h_flex()
                        .id(("excluded-row", ix))
                        .w_full()
                        .h(Tokens::size(34.0))
                        .px(Tokens::space(4.0))
                        .gap(Tokens::space(3.0))
                        .items_center()
                        .border_b_1()
                        .border_color(colors.border)
                        .when(selected == Some(ix), |d| d.bg(colors.accent_soft))
                        .hover(|d| d.bg(colors.hover))
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |v, cx| v.select(ix, cx));
                        })
                        .child(
                            v_flex()
                                .w(Tokens::size(260.0))
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(Tokens::text_sm())
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(r.name.clone()),
                                )
                                .child(
                                    div()
                                        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                        .text_size(Tokens::text_2xs())
                                        .text_color(colors.text_faint)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(r.path.clone()),
                                ),
                        )
                        .child(
                            div()
                                .w(Tokens::size(120.0))
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_muted)
                                .child(class_name(r.class)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(r.reasons.join(" · ")),
                        )
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();
        v_flex()
            .id("view-excluded")
            .test_support()
            .size_full()
            .bg(colors.background)
            .child(header)
            .child(columns)
            .child(list)
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
