//! First launch: what PhotoSquirrel does, the read-only promise, and one primary action, "Add a source".

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::*;
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::keymap;
use crate::shell::Shell;
use crate::widgets::kbd;

/// The read-only promise, word for word (tests and docs refer to it).
pub const READ_ONLY_PROMISE: &str = "Your sources are only ever read. PhotoSquirrel never moves, changes or deletes a file in them.";

pub(crate) fn render(cx: &mut Context<'_, Shell>) -> impl IntoElement + use<> {
    let colors = *ui_theme::colors(cx);
    let step = |n: &'static str, icon: IconName, title: &'static str, body: &'static str| {
        v_flex()
            .flex_1()
            .min_w_0()
            .gap(Tokens::space(2.0))
            .p(Tokens::space(4.0))
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.panel)
            .border_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(
                        div()
                            .size(Tokens::size(22.0))
                            .rounded_full()
                            .bg(colors.raised)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(Tokens::text_xs())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.accent_text)
                            .child(n),
                    )
                    .child(
                        Icon::new(icon)
                            .size(Tokens::icon_md())
                            .text_color(colors.text_muted),
                    ),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(title),
            )
            .child(
                div()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text_muted)
                    .child(body),
            )
    };
    let palette_key = keymap::shortcut_for(&crate::actions::OpenCommandPalette).unwrap_or_default();

    div()
        .id("onboarding")
        .test_support()
        .size_full()
        .overflow_y_scroll()
        .bg(colors.background)
        .child(
            v_flex()
                .w_full()
                .min_h_full()
                .items_center()
                .justify_center()
                .py(Tokens::space(12.0))
                .px(Tokens::space(8.0))
                .child(
                    v_flex()
                        .max_w(Tokens::size(720.0))
                        .w_full()
                        .items_center()
                        .gap(Tokens::space(5.0))
                        .child(crate::widgets::brand_mark(Tokens::hero_size(), cx))
                        .child(
                            v_flex()
                                .items_center()
                                .gap(Tokens::space(2.0))
                                .child(
                                    div()
                                        .text_size(Tokens::text_display())
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text)
                                        .child("Every photo. Best copy. Once."),
                                )
                                .child(
                                    div()
                                        .max_w(Tokens::prose_width())
                                        .text_center()
                                        .text_size(Tokens::text_lg())
                                        .line_height(relative(1.5))
                                        .text_color(colors.text_muted)
                                        .child(
                                            "Point PhotoSquirrel at your drives, USB backups and NAS shares. \
                                             It finds the best copy of every photo across duplicates, \
                                             thumbnails and recompressed versions, and copies them into \
                                             one clean library.",
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .id("onboarding-read-only")
                                .test_support()
                                .w(Tokens::prose_width())
                                .max_w_full()
                                .gap(Tokens::space(3.0))
                                .px(Tokens::space(4.0))
                                .py(Tokens::space(3.0))
                                .rounded(Tokens::RADIUS_LG)
                                .bg(colors.accent_soft)
                                .border_1()
                                .border_color(colors.accent.opacity(0.35))
                                .child(
                                    Icon::new(IconName::ShieldCheck)
                                        .size(Tokens::icon_lg())
                                        .text_color(colors.accent_text),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(Tokens::text_sm())
                                        .text_color(colors.text)
                                        .child(READ_ONLY_PROMISE),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap(Tokens::space(3.0))
                                .child(
                                    h_flex()
                                        .id("onboarding-add-source")
                                        .test_support()
                                        .h(Tokens::size(40.0))
                                        .px(Tokens::space(5.0))
                                        .gap(Tokens::space(2.0))
                                        .rounded(Tokens::RADIUS_MD)
                                        .bg(colors.accent)
                                        .hover(|d| d.bg(colors.accent_hover))
                                        .active(|d| d.bg(colors.accent_active))
                                        .shadow_md()
                                        .cursor_pointer()
                                        .text_color(colors.on_accent)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.add_source(window, cx)
                                        }))
                                        .child(Icon::new(IconName::FolderPlus).size(Tokens::icon_md()))
                                        .child("Add a source"),
                                )
                                .child(
                                    h_flex()
                                        .gap(Tokens::space(1.5))
                                        .text_size(Tokens::text_sm())
                                        .text_color(colors.text_faint)
                                        .child("or press")
                                        .child(kbd(palette_key, cx))
                                        .child("for every command"),
                                ),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .mt(Tokens::space(4.0))
                                .gap(Tokens::space(3.0))
                                .items_stretch()
                                .child(step(
                                    "1",
                                    IconName::HardDrive,
                                    "Add sources",
                                    "Internal drives, USB backups, NAS shares. Add as many as you have.",
                                ))
                                .child(step(
                                    "2",
                                    IconName::Layers,
                                    "Scan & compare",
                                    "Exact copies, resized and recompressed versions are grouped; the best one is kept.",
                                ))
                                .child(step(
                                    "3",
                                    IconName::FolderOutput,
                                    "Copy out",
                                    "Into a library ready for Apple Photos, Google Photos or Lightroom.",
                                )),
                        ),
                ),
        )
}
