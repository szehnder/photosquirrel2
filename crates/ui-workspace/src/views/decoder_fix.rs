//! "How to enable HEIC previews" (ADR-0089): the reason a decoder is missing and the fix for this system, shown in
//! the scan summary and the inspector. Commands sit in a monospace block with a Copy button; a Microsoft Store
//! step shows its plain web URL and opens the Store link only when its button is clicked.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::Fix;
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::views::kit::{self, Tone};

/// "How to enable HEIC previews" (the disclosure's label).
pub fn how_to(label: &str) -> String {
    format!("How to enable {label} previews")
}

/// The reason and every step of `fix`. Ids: `{id}`, `{id}-reason`, `{id}-step-{i}`, `{id}-copy-{i}`,
/// `{id}-open-{i}`.
pub fn fix_panel(id: &str, reason: Option<&str>, fix: &Fix, cx: &App) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let mut col = v_flex()
        .id(SharedString::from(id.to_owned()))
        .test_support()
        .aria_label(SharedString::from(fix.summary.clone()))
        .w_full()
        .gap(Tokens::space(1.5))
        .text_size(Tokens::text_sm());
    if let Some(r) = reason {
        col = col.child(
            div()
                .id(SharedString::from(format!("{id}-reason")))
                .test_support()
                .aria_label(SharedString::from(r.to_owned()))
                .text_color(colors.text_muted)
                .child(capitalized(r)),
        );
    }
    for (i, s) in fix.steps.iter().enumerate() {
        let mut label = s.text.clone();
        if let Some(c) = &s.command {
            label.push_str(": ");
            label.push_str(c);
        }
        if let Some(u) = &s.url {
            label.push_str(" (");
            label.push_str(u);
            label.push(')');
        }
        let mut row = v_flex()
            .id(SharedString::from(format!("{id}-step-{i}")))
            .test_support()
            .aria_label(SharedString::from(label))
            .w_full()
            .gap(Tokens::space(1.0))
            .child(
                div()
                    .min_w_0()
                    .text_color(colors.text)
                    .child(s.text.clone()),
            );
        if let Some(c) = s.command.clone() {
            let copy = c.clone();
            row = row.child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .items_center()
                    .p(Tokens::space(1.5))
                    .rounded(Tokens::RADIUS_SM)
                    // A recessed, outlined well: visible against the tinted notice in both themes.
                    .bg(colors.inset())
                    .border_1()
                    .border_color(colors.border_strong)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text)
                            .child(c),
                    )
                    .child(kit::button(
                        SharedString::from(format!("{id}-copy-{i}")),
                        "Copy",
                        Some(IconName::Copy),
                        Tone::Ghost,
                        kit::on(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        }),
                        cx,
                    )),
            );
        }
        if s.url.is_some() || s.open.is_some() {
            let open = s.open.clone().or_else(|| s.url.clone());
            row = row.child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .items_center()
                    .when_some(s.url.clone(), |d, u| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                .text_size(Tokens::text_mono())
                                .text_color(colors.text_muted)
                                .child(u),
                        )
                    })
                    .when_some(open, |d, link| {
                        let label = if link.starts_with("ms-windows-store:") {
                            "Open in Microsoft Store"
                        } else {
                            "Open link"
                        };
                        d.child(kit::button(
                            SharedString::from(format!("{id}-open-{i}")),
                            label,
                            Some(IconName::Link),
                            Tone::Secondary,
                            // Opened only on this explicit click.
                            kit::on(move |_, _, cx| cx.open_url(&link)),
                            cx,
                        ))
                    }),
            );
        }
        col = col.child(row);
    }
    col.into_any_element()
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
