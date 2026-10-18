//! The shell's chrome: title bar, navigator, splitters, central area (banner, loading, onboarding, views),
//! status bar, overlays and toasts. Kept apart from `shell.rs`, which holds state and behaviour.

use std::time::Duration;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ui_theme::{Motion, Tokens};

use crate::actions::*;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::keymap;
use crate::service::{CatalogSummary, Loadable};
use crate::shell::{BannerState, DraggedSplitter, Overlay, Shell, Side, ToastKind};
use crate::views::{LibraryMode, NavGroup, ViewId};
use crate::widgets::kbd;

fn icon_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    active: bool,
    cx: &mut Context<'_, Shell>,
    on_click: impl Fn(&mut Shell, &mut Window, &mut Context<'_, Shell>) + 'static,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label(label)
        .size(Tokens::size(28.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(Tokens::RADIUS_MD)
        .cursor_pointer()
        .text_color(if active {
            colors.text
        } else {
            colors.text_muted
        })
        .when(active, |d| d.bg(colors.raised))
        .hover(|d| d.bg(colors.hover).text_color(colors.text))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx)
        })
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .child(Icon::new(icon).size(Tokens::icon_md()))
        .into_any_element()
}

pub(crate) fn title_bar(
    shell: &Shell,
    window: &mut Window,
    cx: &mut Context<'_, Shell>,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let layout = shell.layout().clone();
    let palette_key = keymap::shortcut_for(&OpenCommandPalette).unwrap_or_default();
    let title = match (layout.active_view, layout.library_mode) {
        (ViewId::Library, LibraryMode::Loupe) => "Loupe",
        (view, _) => view.label(),
    };
    h_flex()
        .id("title-bar")
        .test_support()
        .flex_none()
        .h(Tokens::title_bar_height())
        .px(Tokens::space(3.0))
        .gap(Tokens::space(2.0))
        .bg(colors.chrome)
        .border_b_1()
        .border_color(colors.border)
        .child(
            h_flex()
                .w(px(layout.navigator_width) - Tokens::space(3.0).to_pixels(window.rem_size()))
                .when(!layout.navigator_open, |d| d.w_auto())
                .flex_none()
                .gap(Tokens::space(2.0))
                .child(crate::widgets::brand_mark(Tokens::size(22.0), cx))
                .child(
                    div()
                        .text_size(Tokens::text_md())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child("PhotoSquirrel"),
                )
                .child(div().flex_1())
                .child(icon_button(
                    "title-toggle-navigator",
                    IconName::PanelLeft,
                    "Toggle navigator",
                    layout.navigator_open,
                    cx,
                    |this, w, cx| this.toggle_panel(Side::Navigator, w, cx),
                )),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(Tokens::space(3.0))
                .child(
                    div()
                        .id("title-view-name")
                        .test_support()
                        .pl(Tokens::space(2.0))
                        .text_size(Tokens::text_md())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .child(title),
                )
                .child(div().flex_1())
                .child(
                    h_flex()
                        .id("title-search")
                        .test_support()
                        .role(Role::Button)
                        .aria_label("Search commands")
                        .w(Tokens::search_width())
                        .h(Tokens::size(28.0))
                        .px(Tokens::space(2.5))
                        .gap(Tokens::space(2.0))
                        .rounded(Tokens::RADIUS_MD)
                        .bg(colors.background)
                        .border_1()
                        .border_color(colors.border)
                        .cursor_pointer()
                        .hover(|d| d.border_color(colors.border_strong).bg(colors.panel))
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_faint)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_palette(window, cx);
                        }))
                        .child(Icon::new(IconName::Search).size(Tokens::icon_sm()))
                        .child(div().flex_1().child("Search commands and views"))
                        .child(kbd(palette_key, cx)),
                )
                .child(div().flex_1()),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(Tokens::space(1.0))
                .child(icon_button(
                    "title-rescan",
                    IconName::RotateCw,
                    "Rescan all sources",
                    false,
                    cx,
                    |this, w, cx| this.rescan(w, cx),
                ))
                .child(icon_button(
                    "title-settings",
                    IconName::Settings,
                    "Settings",
                    false,
                    cx,
                    |this, w, cx| {
                        this.open_settings(w, cx);
                    },
                ))
                .child(icon_button(
                    "title-toggle-inspector",
                    IconName::PanelRight,
                    "Toggle inspector",
                    layout.inspector_open,
                    cx,
                    |this, w, cx| this.toggle_panel(Side::Inspector, w, cx),
                )),
        )
        .into_any_element()
}

fn nav_count(view: ViewId, summary: &Loadable<CatalogSummary>) -> Option<u64> {
    let s = summary.ready()?;
    let n = match view {
        ViewId::Sources => u64::from(s.sources),
        ViewId::Library => s.photos + s.videos,
        ViewId::Duplicates => s.duplicate_groups,
        ViewId::Excluded => s.excluded,
        _ => return None,
    };
    (n > 0).then_some(n)
}

fn navigator(shell: &Shell, cx: &mut Context<'_, Shell>) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let active = shell.active_view();
    let summary = shell.summary().clone();
    let mut list = v_flex().w_full().gap(Tokens::space(0.5));
    let mut group: Option<NavGroup> = None;
    for view in ViewId::ALL {
        if group != Some(view.group()) {
            group = Some(view.group());
            list = list.child(
                div()
                    .pt(Tokens::space(if view == ViewId::Sources {
                        1.0
                    } else {
                        4.0
                    }))
                    .pb(Tokens::space(1.0))
                    .px(Tokens::space(2.5))
                    .child(crate::widgets::eyebrow(view.group().label(), cx)),
            );
        }
        let is_active = view == active;
        let count = nav_count(view, &summary);
        list =
            list.child(
                h_flex()
                    .id(SharedString::from(format!("nav-{}", view.key())))
                    .test_support()
                    .role(Role::Tab)
                    .aria_label(view.label())
                    .relative()
                    .h(Tokens::nav_row_height())
                    .px(Tokens::space(2.5))
                    .gap(Tokens::space(2.5))
                    .rounded(Tokens::RADIUS_MD)
                    .cursor_pointer()
                    .text_color(if is_active {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .when(is_active, |d| {
                        d.bg(colors.pressed).font_weight(FontWeight::MEDIUM)
                    })
                    .when(!is_active, |d| {
                        d.hover(|d| d.bg(colors.hover).text_color(colors.text))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| match view {
                        ViewId::Library => this.show_library(LibraryMode::Grid, window, cx),
                        v => this.show_view(v, window, cx),
                    }))
                    .when(is_active, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .top(Tokens::space(1.5))
                                .bottom(Tokens::space(1.5))
                                .w(Tokens::OUTLINE)
                                .rounded_full()
                                .bg(colors.accent),
                        )
                    })
                    .child(Icon::new(view.icon()).size(Tokens::icon_md()).text_color(
                        if is_active {
                            colors.accent_text
                        } else {
                            colors.text_faint
                        },
                    ))
                    .child(div().flex_1().min_w_0().child(view.label()))
                    .children(count.map(|n| {
                        div()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_faint)
                            .child(format_count(n))
                    })),
            );
    }
    v_flex()
        .id("navigator")
        .test_support()
        .size_full()
        .bg(colors.panel)
        .px(Tokens::space(2.0))
        .py(Tokens::space(2.0))
        .child(list)
        .child(div().flex_1())
        .child(
            h_flex()
                .id("navigator-read-only")
                .test_support()
                .gap(Tokens::space(2.0))
                .p(Tokens::space(2.5))
                .rounded(Tokens::RADIUS_MD)
                .bg(colors.background)
                .border_1()
                .border_color(colors.border)
                .child(
                    Icon::new(IconName::ShieldCheck)
                        .size(Tokens::icon_md())
                        .text_color(colors.success),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(Tokens::text_xs())
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.text)
                                .child("Sources are read-only"),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_2xs())
                                .text_color(colors.text_faint)
                                .child("Nothing is moved, changed or deleted"),
                        ),
                ),
        )
        .into_any_element()
}

fn splitter(side: Side, cx: &mut Context<'_, Shell>) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let id = match side {
        Side::Navigator => "splitter-navigator",
        Side::Inspector => "splitter-inspector",
    };
    // A hairline with a wider invisible grip centred on it.
    div()
        .flex_none()
        .w(Tokens::BORDER)
        .h_full()
        .relative()
        .bg(colors.border)
        .child(
            div()
                .id(id)
                .test_support()
                .absolute()
                .top_0()
                .bottom_0()
                .left(-(Tokens::SPLITTER_GRIP / 2.0))
                .w(Tokens::SPLITTER_GRIP)
                .cursor_col_resize()
                .hover(|d| d.bg(colors.accent.opacity(0.35)))
                .on_drag(DraggedSplitter(side), |drag, _, _, cx| cx.new(|_| *drag)),
        )
        .into_any_element()
}

fn banner(shell: &Shell, cx: &mut Context<'_, Shell>) -> Option<impl IntoElement + use<>> {
    let colors = *ui_theme::colors(cx);
    let state = shell.banner()?.clone();
    let (run, resuming) = match &state {
        BannerState::Offered(run) => (run.clone(), false),
        BannerState::Resuming(run) => (run.clone(), true),
    };
    let progress = match (run.percent(), run.files_total) {
        (Some(p), Some(total)) => format!(
            "stopped at {p}% ({} of {} files)",
            format_count(run.files_done),
            format_count(total)
        ),
        _ => format!("stopped after {} files", format_count(run.files_done)),
    };
    Some(
        h_flex()
            .id("interrupted-banner")
            .test_support()
            .flex_none()
            .mx(Tokens::space(3.0))
            .mt(Tokens::space(3.0))
            .px(Tokens::space(3.0))
            .py(Tokens::space(2.0))
            .gap(Tokens::space(3.0))
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.accent_soft)
            .border_1()
            .border_color(colors.accent.opacity(0.4))
            .child(
                Icon::new(IconName::TriangleAlert)
                    .size(Tokens::icon_md())
                    .text_color(colors.accent_text),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("interrupted-banner-title")
                            .test_support()
                            .aria_label(SharedString::from(format!(
                                "Scan of {} was interrupted",
                                run.label
                            )))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(format!("Scan of “{}” was interrupted", run.label)),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child(format!(
                                "It {progress}. Resume picks up where it left off; nothing is scanned twice."
                            )),
                    ),
            )
            .child(
                h_flex()
                    .id("banner-resume")
                    .test_support()
                    .role(Role::Button)
                    .aria_label("Resume")
                    .h(Tokens::control_height())
                    .px(Tokens::space(3.5))
                    .gap(Tokens::space(1.5))
                    .rounded(Tokens::RADIUS_MD)
                    .bg(colors.accent)
                    .hover(|d| d.bg(colors.accent_hover))
                    .text_color(colors.on_accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(Tokens::text_sm())
                    .cursor_pointer()
                    .when(!resuming, |d| {
                        d.on_click(cx.listener(|this, _, window, cx| this.resume(window, cx)))
                    })
                    .child(Icon::new(IconName::Play).size(Tokens::icon_sm()))
                    .child(if resuming { "Resuming…" } else { "Resume" }),
            ),
    )
}

fn loading(cx: &mut Context<'_, Shell>) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let reduce = cx.reduce_motion();
    let tile = |i: usize| {
        let base = div()
            .id(("loading-tile", i))
            .h(Tokens::size(132.0))
            .flex_1()
            .rounded(Tokens::RADIUS_SM)
            .bg(colors.raised);
        if reduce {
            base.into_any_element()
        } else {
            base.with_animation(
                ("loading-pulse", i),
                Animation::new(Duration::from_millis(Motion::Pulse.nominal_millis()))
                    .repeat()
                    .with_easing(pulsating_between(0.45, 0.9)),
                |d, delta| d.opacity(delta),
            )
            .into_any_element()
        }
    };
    v_flex()
        .id("center-loading")
        .test_support()
        .size_full()
        .bg(colors.gallery)
        .p(Tokens::space(4.0))
        .gap(Tokens::space(2.0))
        .child(
            div()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child("Opening your catalog…"),
        )
        .children((0..3).map(move |row| {
            h_flex()
                .w_full()
                .gap(Tokens::space(2.0))
                .children((0..6).map(move |c| tile(row * 6 + c)))
        }))
        .into_any_element()
}

fn failed(message: &str, cx: &mut Context<'_, Shell>) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    v_flex()
        .id("center-failed")
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(Tokens::space(2.0))
        .child(
            Icon::new(IconName::CircleAlert)
                .size(Tokens::icon_lg())
                .text_color(colors.danger),
        )
        .child(
            div()
                .text_color(colors.text)
                .child("The catalog could not be opened"),
        )
        .child(
            div()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child(message.to_owned()),
        )
        .into_any_element()
}

pub(crate) fn body(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<'_, Shell>,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let layout = shell.layout().clone();
    let center: AnyElement = match shell.summary().clone() {
        Loadable::Loading => loading(cx).into_any_element(),
        Loadable::Failed(message) => failed(&message, cx).into_any_element(),
        Loadable::Ready(_) if shell.shows_onboarding() => {
            crate::onboarding::render(cx).into_any_element()
        }
        Loadable::Ready(_) => shell.central_view(window, cx).into_any_element(),
    };
    let banner = banner(shell, cx);
    let inspector = layout
        .inspector_open
        .then(|| shell.inspector_view(window, cx));

    h_flex()
        .id("body")
        .flex_1()
        .min_h_0()
        .w_full()
        .when(layout.navigator_open, |d| {
            d.child(
                div()
                    .w(px(layout.navigator_width))
                    .h_full()
                    .flex_none()
                    .child(navigator(shell, cx)),
            )
            .child(splitter(Side::Navigator, cx))
        })
        .child(
            v_flex()
                .id("center")
                .test_support()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(colors.background)
                .children(banner)
                .child(div().flex_1().min_h_0().child(center)),
        )
        .when_some(inspector, |d, inspector| {
            d.child(splitter(Side::Inspector, cx)).child(
                div()
                    .id("inspector-panel")
                    .test_support()
                    .w(px(layout.inspector_width))
                    .h_full()
                    .flex_none()
                    .child(inspector),
            )
        })
        .into_any_element()
}

pub(crate) fn status_bar(shell: &Shell, cx: &mut Context<'_, Shell>) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let item = || {
        h_flex()
            .gap(Tokens::space(1.5))
            .items_center()
            .text_size(Tokens::text_xs())
            .text_color(colors.text_muted)
    };
    let reduce = cx.reduce_motion();
    let (activity_label, active) = match (&shell.activity, shell.summary()) {
        (Some(a), _) => {
            let progress = match a.total {
                Some(t) if t > 0 => format!(
                    " · {} of {} files · {}%",
                    format_count(a.done),
                    format_count(t),
                    a.done.min(t) * 100 / t
                ),
                _ => format!(" · {} files", format_count(a.done)),
            };
            let label = if a.paused { "Paused" } else { a.label.as_str() };
            (format!("{label}{progress}"), !a.paused)
        }
        (None, Loadable::Loading) => ("Opening catalog".to_owned(), false),
        (None, _) => ("Ready".to_owned(), false),
    };
    let dot = div()
        .id("status-activity-dot")
        .size(Tokens::dot_size())
        .rounded_full()
        .bg(if active || shell.activity.is_some() {
            colors.accent
        } else {
            colors.success
        });
    let dot = if active && !reduce {
        dot.with_animation(
            "status-pulse",
            Animation::new(Duration::from_millis(Motion::Pulse.nominal_millis()))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            |d, delta| d.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    };

    let counts = shell.summary().ready().cloned();
    let photo_label = match &counts {
        Some(s) => format!("{} photos", format_count(s.photos)),
        None => "— photos".to_owned(),
    };
    let zoom = (zoom_percent(cx) != 100).then(|| item().child(format!("{}%", zoom_percent(cx))));

    h_flex()
        .id("status-bar")
        .test_support()
        .flex_none()
        .h(Tokens::status_bar_height())
        .px(Tokens::space(3.0))
        .gap(Tokens::space(4.0))
        .bg(colors.chrome)
        .border_t_1()
        .border_color(colors.border)
        .child(
            item()
                .id("status-activity")
                .test_support()
                .aria_label(SharedString::from(activity_label.clone()))
                .child(dot)
                .child(div().text_color(colors.text).child(activity_label)),
        )
        .child(
            item()
                .id("status-photo-count")
                .test_support()
                .aria_label(SharedString::from(photo_label.clone()))
                .child(Icon::new(IconName::Images).size(Tokens::icon_sm()))
                .child(photo_label),
        )
        .children(counts.as_ref().map(|s| {
            item()
                .id("status-video-count")
                .child(format!("{} videos", format_count(s.videos)))
        }))
        .children(counts.as_ref().map(|s| {
            item().id("status-source-count").child(format!(
                "{} {}",
                s.sources,
                if s.sources == 1 { "source" } else { "sources" }
            ))
        }))
        .child(div().flex_1())
        .children(zoom)
        .child(
            item()
                .id("status-local")
                .child(Icon::new(IconName::ShieldCheck).size(Tokens::icon_sm()))
                .child("Local only"),
        )
        .child(item().child(concat!("v", env!("CARGO_PKG_VERSION"))))
        .into_any_element()
}

fn zoom_percent(cx: &App) -> i32 {
    let size = crate::settings::AppSettings::get(cx)
        .appearance
        .ui_font_size;
    (size / ui_theme::DEFAULT_UI_FONT_SIZE * 100.0).round() as i32
}

pub(crate) fn overlay(
    shell: &Shell,
    cx: &mut Context<'_, Shell>,
) -> Option<impl IntoElement + use<>> {
    let colors = *ui_theme::colors(cx);
    let content: AnyElement = match shell.overlay.as_ref()? {
        Overlay::Palette(p) => p.clone().into_any_element(),
        Overlay::Settings(s) => s.clone().into_any_element(),
    };
    let fade = Duration::from_millis(Motion::Normal.nominal_millis());
    let reduce = cx.reduce_motion();
    // Only the scrim fades in; the panel is opaque from its first frame.
    let scrim = div().absolute().inset_0().bg(colors.scrim);
    let scrim = if reduce {
        scrim.into_any_element()
    } else {
        scrim
            .with_animation(
                "overlay-scrim-in",
                Animation::new(fade).with_easing(ease_out_quint()),
                |d, t| d.opacity(t),
            )
            .into_any_element()
    };
    let layer = div()
        .id("overlay")
        .test_support()
        .key_context(keymap::OVERLAY)
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .pt(Tokens::overlay_top())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_overlay(window, cx)),
        )
        .child(scrim)
        .child(
            div()
                .id("overlay-content")
                .relative()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(content),
        );
    Some(deferred(layer.into_any_element()).with_priority(1))
}

pub(crate) fn toast(
    shell: &Shell,
    cx: &mut Context<'_, Shell>,
) -> Option<impl IntoElement + use<>> {
    let colors = *ui_theme::colors(cx);
    let toast = shell.toast()?.clone();
    let accent = match toast.kind {
        ToastKind::Info => colors.info,
        ToastKind::Warning => colors.warning,
    };
    Some(
        deferred(
            h_flex()
                .id("toast")
                .test_support()
                .absolute()
                .right(Tokens::space(4.0))
                .bottom(Tokens::size(40.0))
                .max_w(Tokens::size(420.0))
                .items_start()
                .gap(Tokens::space(3.0))
                .p(Tokens::space(3.0))
                .rounded(Tokens::RADIUS_LG)
                .bg(colors.overlay)
                .border_1()
                .border_color(colors.border_strong)
                .shadow_lg()
                .child(
                    Icon::new(match toast.kind {
                        ToastKind::Info => IconName::Info,
                        ToastKind::Warning => IconName::TriangleAlert,
                    })
                    .size(Tokens::icon_md())
                    .text_color(accent),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(Tokens::space(0.5))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .child(toast.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text_muted)
                                .child(toast.body.clone()),
                        ),
                )
                .child(
                    div()
                        .id("toast-dismiss")
                        .test_support()
                        .role(Role::Button)
                        .aria_label("Dismiss")
                        .cursor_pointer()
                        .text_color(colors.text_faint)
                        .hover(|d| d.text_color(colors.text))
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_toast(cx)))
                        .child(Icon::new(IconName::X).size(Tokens::icon_sm())),
                ),
        )
        .with_priority(2),
    )
}
