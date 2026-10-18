//! Small shared pieces of the shell's visual language: key chips, segmented controls, metadata rows, eyebrow
//! labels. Later views reuse these so the app reads as one system.

use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ui_theme::Tokens;
use ui_theme::fonts::{MONO_FONT_FAMILY, UI_FONT_FAMILY};

/// A key hint chip ("Ctrl+K", "G").
pub fn kbd(text: impl Into<SharedString>, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    h_flex()
        .flex_none()
        .h(Tokens::kbd_height())
        .px(Tokens::space(1.5))
        .rounded(Tokens::RADIUS_SM)
        .bg(colors.raised)
        .border_1()
        .border_color(colors.border)
        .text_size(Tokens::text_2xs())
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors.text_muted)
        .child(text.into())
}

/// Small upper-case section label ("CATALOG").
pub fn eyebrow(text: impl Into<SharedString>, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    div()
        .text_size(Tokens::text_2xs())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_faint)
        .child(text.into().to_uppercase())
}

/// How a metadata value is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueStyle {
    /// Interface text (Inter).
    Text,
    /// Technical metadata: EXIF values, hashes, paths (JetBrains Mono, ADR-0028).
    Mono,
}

impl ValueStyle {
    pub fn font_family(self) -> &'static str {
        match self {
            ValueStyle::Text => UI_FONT_FAMILY,
            ValueStyle::Mono => MONO_FONT_FAMILY,
        }
    }
}

/// One `label  value` row in the inspector.
pub fn metadata_row(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    style: ValueStyle,
    cx: &App,
) -> AnyElement {
    let colors = ui_theme::colors(cx);
    let value = value.into();
    h_flex()
        .id(ElementId::Name(id.into()))
        .test_support()
        .w_full()
        .items_start()
        .gap(Tokens::space(3.0))
        .py(Tokens::space(1.0))
        .child(
            div()
                .w(Tokens::size(88.0))
                .flex_none()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child(label.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(style.font_family())
                .text_size(match style {
                    ValueStyle::Text => Tokens::text_sm(),
                    ValueStyle::Mono => Tokens::text_mono(),
                })
                .text_color(colors.text)
                .overflow_hidden()
                .text_ellipsis()
                .child(value),
        )
        .into_any_element()
}

/// A segmented control. `on_select` gets the chosen option's index.
pub fn segmented(
    id: impl Into<SharedString>,
    options: &[&'static str],
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let id: SharedString = id.into();
    let on_select = std::rc::Rc::new(on_select);
    h_flex()
        .id(ElementId::Name(id.clone()))
        .flex_wrap()
        .p(Tokens::space(0.5))
        .gap(Tokens::space(0.5))
        .rounded(Tokens::RADIUS_MD)
        .bg(colors.background)
        .border_1()
        .border_color(colors.border)
        .children(options.iter().enumerate().map(|(ix, label)| {
            let on_select = on_select.clone();
            let active = ix == selected;
            div()
                .id(SharedString::from(format!("{id}-{ix}")))
                .test_support()
                .px(Tokens::space(2.5))
                .h(Tokens::size(24.0))
                .flex()
                .items_center()
                .rounded(Tokens::RADIUS_SM)
                .text_size(Tokens::text_sm())
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(colors.raised)
                        .text_color(colors.text)
                        .font_weight(FontWeight::MEDIUM)
                        .shadow_sm()
                })
                .when(!active, |d| {
                    d.text_color(colors.text_muted)
                        .hover(|d| d.text_color(colors.text))
                })
                .on_click(move |_, window, cx| on_select(ix, window, cx))
                .child(*label)
        }))
        .into_any_element()
}

/// The app mark: the squirrel on an acorn tile. Vector, so it paints in the first frame at any size.
pub fn brand_mark(size: Rems, cx: &App) -> AnyElement {
    let colors = ui_theme::colors(cx);
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(size * 0.28)
        .bg(colors.accent)
        .child(
            gpui_kit::component::Icon::new(crate::assets::IconName::Squirrel)
                .size(size * 0.66)
                .text_color(colors.on_accent),
        )
        .into_any_element()
}
