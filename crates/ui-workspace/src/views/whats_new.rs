//! What's new (spec `app-updates` "Clear update notices", design D8): the release notes of an offered version,
//! shown in the app. The notes come from the signed manifest the update check already has, so opening the dialog
//! makes no request. They are Markdown; `ps_update::manifest::notes_blocks` turns them into plain blocks whose
//! bold headlines are drawn bold, so no `**` or `- ` ever reaches the screen. The footer opens the release page.
//!
//! It is shown as a shell overlay, closed with Esc, Close or a click outside.

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_service::updates::{Available, NotesBlock, NotesBlockKind};
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::views::kit::{self, Tone};

/// Something the dialog asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WhatsNewEvent {
    Close,
}

pub struct WhatsNewDialog {
    version: String,
    blocks: Vec<NotesBlock>,
    notes_url: String,
    focus: FocusHandle,
}

impl EventEmitter<WhatsNewEvent> for WhatsNewDialog {}

/// The dialog's title for `version`.
pub fn title(version: &str) -> String {
    format!("What's new in PhotoSquirrel {version}")
}

impl WhatsNewDialog {
    pub fn new(available: &Available, cx: &mut Context<'_, Self>) -> Self {
        let notes_url = if available.notes_url.is_empty() {
            crate::updates_ui::release_page(&available.version)
        } else {
            available.notes_url.clone()
        };
        Self {
            version: available.version.clone(),
            blocks: available.notes_blocks(),
            notes_url,
            focus: cx.focus_handle(),
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn blocks(&self) -> &[NotesBlock] {
        &self.blocks
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn block(ix: usize, b: &NotesBlock, cx: &App) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let id = SharedString::from(format!("whats-new-block-{ix}"));
        let label = SharedString::from(b.text.clone());
        if b.kind == NotesBlockKind::Heading {
            return div()
                .id(id)
                .test_support()
                .aria_label(label.clone())
                .pt(Tokens::space(1.0))
                .text_size(Tokens::text_xs())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_faint)
                .child(label)
                .into_any_element();
        }
        let headline = HighlightStyle {
            color: Some(colors.text),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let text = StyledText::new(label.clone())
            .with_highlights(b.bold.iter().map(|r| (r.clone(), headline)));
        h_flex()
            .id(id)
            .test_support()
            .aria_label(label)
            .w_full()
            .items_start()
            .gap(Tokens::space(2.5))
            .when(b.kind == NotesBlockKind::Bullet, |row| {
                row.child(
                    div()
                        .flex_none()
                        .mt(Tokens::space(2.0))
                        .size(Tokens::size(6.0))
                        .rounded_full()
                        .bg(colors.accent),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .line_height(relative(1.55))
                    .text_color(colors.text_muted)
                    .child(text),
            )
            .into_any_element()
    }
}

impl Focusable for WhatsNewDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WhatsNewDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let mut body = v_flex()
            .id("whats-new-body")
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(Tokens::space(3.5))
            .px(Tokens::space(5.0))
            .py(Tokens::space(4.0))
            .text_size(Tokens::text_sm());
        if self.blocks.is_empty() {
            body = body.child(
                div()
                    .id("whats-new-empty")
                    .test_support()
                    .text_color(colors.text_muted)
                    .child("This version has no notes here. The release page has the details."),
            );
        }
        for (ix, b) in self.blocks.iter().enumerate() {
            body = body.child(Self::block(ix, b, cx));
        }
        let link = self.notes_url.clone();
        v_flex()
            .id("whats-new")
            .test_support()
            .key_context("WhatsNew")
            .track_focus(&self.focus)
            .w(Tokens::size(560.0))
            .max_w(window.viewport_size().width * 0.9)
            .max_h(window.viewport_size().height * 0.8)
            .bg(colors.overlay)
            .border_1()
            .border_color(colors.border)
            .rounded(Tokens::RADIUS_LG)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .px(Tokens::space(5.0))
                    .pt(Tokens::space(4.0))
                    .pb(Tokens::space(3.0))
                    .items_center()
                    .gap(Tokens::space(3.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex_none()
                            .p(Tokens::space(2.0))
                            .rounded(Tokens::RADIUS_MD)
                            .bg(colors.accent_soft)
                            .child(
                                Icon::new(IconName::Sparkles)
                                    .size(Tokens::icon_md())
                                    .text_color(colors.accent_text),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .id("whats-new-title")
                                    .test_support()
                                    .aria_label(SharedString::from(title(&self.version)))
                                    .text_size(Tokens::text_lg())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .child(title(&self.version)),
                            )
                            .child(
                                div()
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_faint)
                                    .child("Release notes"),
                            ),
                    )
                    .child(kit::button(
                        "whats-new-close",
                        "Close",
                        None,
                        Tone::Ghost,
                        kit::on(cx.listener(|_, _, _, cx| cx.emit(WhatsNewEvent::Close))),
                        cx,
                    )),
            )
            .child(body)
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .px(Tokens::space(5.0))
                    .py(Tokens::space(3.0))
                    .gap(Tokens::space(3.0))
                    .items_center()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_faint)
                            .child("Downloads and every detail are on the release page."),
                    )
                    .child(kit::button(
                        "whats-new-release-page",
                        "Release page",
                        Some(IconName::ExternalLink),
                        Tone::Secondary,
                        kit::on(move |_, _, cx| cx.open_url(&link)),
                        cx,
                    )),
            )
    }
}
