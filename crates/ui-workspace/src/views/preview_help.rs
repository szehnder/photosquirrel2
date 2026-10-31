//! The "Why no preview?" dialog (`preview-placeholders`, ADR-0109): for one file with a placeholder, its format
//! and reason in plain words, what still works, and what to do, chosen by reason: the decoder report's install
//! steps (commands with Copy, Store links opened only on click), Fill in previews when a preview is possible now,
//! "a later version may", or "the file may be damaged". A footer offers every file with the same reason through
//! the Library's No preview filter.
//!
//! The content comes from `ps_service::previews::explain` on a background task (never the UI thread); it is shown
//! as a shell overlay, closed with Esc or a click outside.

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::*;
use ps_model::{FillScope, ItemId, PreviewAction, PreviewHelp, PreviewReason};
use ui_gallery::source::SharedSource;
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::views::kit::{self, Tone};

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq)]
pub enum HelpState {
    Loading,
    Ready(Box<PreviewHelp>),
    /// The file has a preview (or is gone).
    NoPlaceholder,
    Failed(String),
}

/// Something the dialog asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewHelpEvent {
    Close,
    /// Show the Library filtered to these placeholders.
    ShowThem {
        reason: PreviewReason,
        format: Option<ps_model::Format>,
    },
    /// Fill in previews was started for this scope.
    FillStarted(FillScope),
}

pub struct PreviewHelpDialog {
    item: ItemId,
    source: SharedSource,
    state: HelpState,
    /// Fill in previews was asked for (the button is unavailable until the dialog closes).
    filling: bool,
    fill_error: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<PreviewHelpEvent> for PreviewHelpDialog {}

/// "Then restart PhotoSquirrel and fill in previews" (after installing a decoder).
pub const RESTART_TEXT: &str = "Then restart PhotoSquirrel and fill in previews.";
/// What the dialog says when a preview can be made now.
pub const FILL_NOW_TEXT: &str = "A preview can be made now.";
pub const LATER_VERSION_TEXT: &str =
    "A later version may; PhotoSquirrel fills these in after an update.";
/// A video placeholder made without a poster-frame tool, when one is set up now.
pub const POSTER_TOOL_SET_TEXT: &str = "A poster-frame tool is set up now: Fill in previews, or the next rescan of \
     this video's source, gives it its preview frame.";
/// A video whose poster frame failed (ADR-0114): what Try again does.
pub const POSTER_RETRY_TEXT: &str = "Try again runs ffmpeg once more on each video whose poster frame failed, for \
     example after updating ffmpeg.";
pub const DAMAGED_TEXT: &str =
    "The file may be damaged. Try opening it in another app. If it opens there, please report it.";

impl PreviewHelpDialog {
    /// A dialog for `item`, loading its content in the background.
    pub fn new(item: ItemId, source: SharedSource, cx: &mut Context<'_, Self>) -> Self {
        let mut this = Self {
            item,
            source,
            state: HelpState::Loading,
            filling: false,
            fill_error: None,
            focus: cx.focus_handle(),
        };
        this.load(cx);
        this
    }

    /// A dialog whose content is already known (the start-up grace of scripted screenshots).
    pub fn with_state(
        item: ItemId,
        source: SharedSource,
        state: HelpState,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self {
            item,
            source,
            state,
            filling: false,
            fill_error: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn item(&self) -> ItemId {
        self.item
    }

    pub fn state(&self) -> &HelpState {
        &self.state
    }

    pub fn help(&self) -> Option<&PreviewHelp> {
        match &self.state {
            HelpState::Ready(h) => Some(h),
            _ => None,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The content of `item`, read on the calling thread (for the start-up grace; elsewhere use [`Self::new`]).
    pub fn read(source: &SharedSource, item: ItemId) -> HelpState {
        match source.explain(item) {
            Ok(Some(h)) => HelpState::Ready(Box::new(h)),
            Ok(None) => HelpState::NoPlaceholder,
            Err(e) => HelpState::Failed(format!("{e:#}")),
        }
    }

    fn load(&mut self, cx: &mut Context<'_, Self>) {
        let source = self.source.clone();
        let item = self.item;
        let task = cx
            .background_executor()
            .spawn(async move { Self::read(&source, item) });
        cx.spawn(async move |this, cx| {
            let state = task.await;
            let _ = this.update(cx, |this, cx| {
                this.state = state;
                cx.notify();
            });
        })
        .detach();
    }

    /// Start Fill in previews for the dialog's format (the "Fill in previews" button).
    pub fn fill_in(&mut self, cx: &mut Context<'_, Self>) {
        let Some(PreviewAction::FillIn { format, .. }) = self.help().map(|h| h.action.clone())
        else {
            return;
        };
        self.start_fill(
            FillScope {
                format,
                reason: None,
            },
            cx,
        );
    }

    /// Try again (ADR-0114): a Fill in previews job for the reason "poster frame failed", which runs the tool once
    /// more on each such video.
    pub fn try_again(&mut self, cx: &mut Context<'_, Self>) {
        if !matches!(
            self.help().map(|h| &h.action),
            Some(PreviewAction::PosterFailed { .. })
        ) {
            return;
        }
        self.start_fill(
            FillScope {
                format: None,
                reason: Some(PreviewReason::PosterFailed),
            },
            cx,
        );
    }

    fn start_fill(&mut self, scope: FillScope, cx: &mut Context<'_, Self>) {
        if self.filling {
            return;
        }
        self.filling = true;
        let source = self.source.clone();
        let s2 = scope.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.fill_previews(s2) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(()) => cx.emit(PreviewHelpEvent::FillStarted(scope)),
                    Err(e) => {
                        this.filling = false;
                        this.fill_error = Some(format!("{e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Show them": the Library filtered to every file with the same reason and format.
    pub fn show_them(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(h) = self.help() {
            cx.emit(PreviewHelpEvent::ShowThem {
                reason: h.reason,
                format: h.format,
            });
        }
    }

    fn text(id: &str, text: impl Into<SharedString>, color: Hsla) -> Stateful<Div> {
        let text: SharedString = text.into();
        div()
            .id(SharedString::from(id.to_owned()))
            .aria_label(text.clone())
            .w_full()
            .text_color(color)
            .child(text)
    }

    fn action(&self, h: &PreviewHelp, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let mut col = v_flex().w_full().gap(Tokens::space(2.0));
        match &h.action {
            PreviewAction::Install { key, reason, fix } => {
                col = match fix {
                    Some(fix) => col.child(crate::views::decoder_fix::fix_panel(
                        "preview-help-fix",
                        reason.as_deref(),
                        fix,
                        cx,
                    )),
                    None => col.child(
                        Self::text(
                            "preview-help-fix-none",
                            format!(
                                "Install a {} decoder for this system (see `photosquirrel-cli doctor`).",
                                key.label()
                            ),
                            colors.text,
                        )
                        .test_support(),
                    ),
                };
                // One closing line: the report's own "restart and rescan" step is left out of `fix` by the
                // service (`ps_service::previews::without_rescan_step`).
                if fix.as_ref().is_none_or(|f| f.restart) {
                    col = col.child(
                        Self::text("preview-help-restart", RESTART_TEXT, colors.text_muted)
                            .test_support(),
                    );
                }
            }
            PreviewAction::FillIn { count, .. } => {
                let label = if *count == 1 {
                    format!("Fill in previews ({} file)", h.format_label)
                } else {
                    format!("Fill in previews ({count} {} files)", h.format_label)
                };
                col = col
                    .child(
                        Self::text("preview-help-fill-now", FILL_NOW_TEXT, colors.text)
                            .test_support(),
                    )
                    .child(
                        h_flex().child(kit::button(
                            "preview-help-fill",
                            label,
                            Some(IconName::RefreshCw),
                            Tone::Primary,
                            (!self.filling)
                                .then(|| kit::on(cx.listener(|this, _, _, cx| this.fill_in(cx))))
                                .flatten(),
                            cx,
                        )),
                    );
                if let Some(e) = &self.fill_error {
                    col = col.child(
                        Self::text("preview-help-fill-error", e.clone(), colors.danger)
                            .test_support(),
                    );
                }
            }
            PreviewAction::UpdateLater { detail } => {
                col = col
                    .child(
                        Self::text(
                            "preview-help-unsupported",
                            format!("This version can't read {detail} files."),
                            colors.text,
                        )
                        .test_support(),
                    )
                    .child(
                        Self::text("preview-help-update", LATER_VERSION_TEXT, colors.text_muted)
                            .test_support(),
                    );
            }
            PreviewAction::Damaged { message } => {
                col = col
                    .child(
                        div()
                            .id("preview-help-error")
                            .test_support()
                            .aria_label(SharedString::from(message.clone()))
                            .w_full()
                            .p(Tokens::space(1.5))
                            .rounded(Tokens::RADIUS_SM)
                            .bg(colors.inset())
                            .border_1()
                            .border_color(colors.border_strong)
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text)
                            .child(message.clone()),
                    )
                    .child(
                        Self::text("preview-help-damaged", DAMAGED_TEXT, colors.text)
                            .test_support(),
                    )
                    .child(
                        Self::text(
                            "preview-help-report",
                            ps_model::ISSUES_URL,
                            colors.text_muted,
                        )
                        .test_support()
                        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                        .text_size(Tokens::text_mono()),
                    );
            }
            PreviewAction::PosterTool { fix } => {
                col = match fix {
                    Some(fix) => col.child(crate::views::decoder_fix::fix_panel(
                        "preview-help-fix",
                        Some("videos get their preview frame from ffmpeg"),
                        fix,
                        cx,
                    )),
                    None => col.child(
                        Self::text("preview-help-poster", POSTER_TOOL_SET_TEXT, colors.text)
                            .test_support(),
                    ),
                };
            }
            PreviewAction::PosterFailed { message } => {
                col = col
                    .child(
                        div()
                            .id("preview-help-error")
                            .test_support()
                            .aria_label(SharedString::from(message.clone()))
                            .w_full()
                            .p(Tokens::space(1.5))
                            .rounded(Tokens::RADIUS_SM)
                            .bg(colors.inset())
                            .border_1()
                            .border_color(colors.border_strong)
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text)
                            .child(message.clone()),
                    )
                    .child(
                        Self::text(
                            "preview-help-retry-note",
                            POSTER_RETRY_TEXT,
                            colors.text_muted,
                        )
                        .test_support(),
                    )
                    .child(
                        h_flex().child(kit::button(
                            "preview-help-retry",
                            "Try again",
                            Some(IconName::RefreshCw),
                            Tone::Primary,
                            (!self.filling)
                                .then(|| kit::on(cx.listener(|this, _, _, cx| this.try_again(cx))))
                                .flatten(),
                            cx,
                        )),
                    );
                if let Some(e) = &self.fill_error {
                    col = col.child(
                        Self::text("preview-help-fill-error", e.clone(), colors.danger)
                            .test_support(),
                    );
                }
            }
            PreviewAction::Sequence => {
                col = col.child(
                    Self::text(
                        "preview-help-sequence",
                        "Image sequences show their first frame in a later version.",
                        colors.text,
                    )
                    .test_support(),
                );
            }
            PreviewAction::NewerVersion => {
                col = col.child(
                    Self::text(
                        "preview-help-newer",
                        "Open this catalog with the newer version of PhotoSquirrel to see why.",
                        colors.text,
                    )
                    .test_support(),
                );
            }
        }
        col.into_any_element()
    }
}

/// "12 other files have no preview for this reason".
pub fn others_text(n: u64) -> String {
    match n {
        1 => "1 other file has no preview for this reason".to_owned(),
        n => format!("{n} other files have no preview for this reason"),
    }
}

/// [`others_text`] for `reason`: "videos" for the poster-frame reasons (ADR-0114).
pub fn others_text_for(n: u64, reason: PreviewReason) -> String {
    let video = matches!(
        reason,
        PreviewReason::NoPosterTool | PreviewReason::PosterFailed
    );
    match (n, video) {
        (_, false) => others_text(n),
        (1, true) => "1 other video has no preview for this reason".to_owned(),
        (n, true) => format!("{n} other videos have no preview for this reason"),
    }
}

impl Focusable for PreviewHelpDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PreviewHelpDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let mut body = v_flex()
            .id("preview-help-body")
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(Tokens::space(3.0))
            .px(Tokens::space(5.0))
            .py(Tokens::space(4.0))
            .text_size(Tokens::text_sm());
        let mut footer = None;
        match self.state.clone() {
            HelpState::Loading => {
                body = body.child(Self::text(
                    "preview-help-loading",
                    "Reading…",
                    colors.text_muted,
                ));
            }
            HelpState::NoPlaceholder => {
                body = body.child(
                    Self::text("preview-help-none", "This file has a preview.", colors.text)
                        .test_support(),
                );
            }
            HelpState::Failed(e) => {
                body =
                    body.child(Self::text("preview-help-failed", e, colors.danger).test_support());
            }
            HelpState::Ready(h) => {
                body = body
                    .child(
                        h_flex()
                            .w_full()
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .child(
                                div()
                                    .id("preview-help-file")
                                    .test_support()
                                    .aria_label(SharedString::from(format!(
                                        "{} · {}",
                                        h.name, h.format_label
                                    )))
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(colors.text)
                                    .child(h.name.clone()),
                            )
                            .child(kit::chip(
                                h.format_label.clone(),
                                colors.text_muted,
                                ui_theme::Colors::tint(colors.text, 0.1),
                            )),
                    )
                    .child(
                        Self::text("preview-help-reason", h.sentence.clone(), colors.text)
                            .test_support(),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .child(
                                Icon::new(IconName::CircleCheck)
                                    .size(Tokens::icon_sm())
                                    .text_color(colors.success),
                            )
                            .child(
                                Self::text(
                                    "preview-help-still",
                                    h.still_works.clone(),
                                    colors.text_muted,
                                )
                                .test_support(),
                            ),
                    )
                    .child(
                        div()
                            .pt(Tokens::space(1.0))
                            .text_size(Tokens::text_xs())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_faint)
                            .child("WHAT YOU CAN DO"),
                    )
                    .child(self.action(&h, cx));
                if h.others > 0 {
                    let text = others_text_for(h.others, h.reason);
                    footer = Some(
                        h_flex()
                            .w_full()
                            .flex_none()
                            .px(Tokens::space(5.0))
                            .py(Tokens::space(3.0))
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .border_t_1()
                            .border_color(colors.border)
                            .child(
                                Self::text("preview-help-others", text, colors.text_muted)
                                    .test_support()
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(kit::button(
                                "preview-help-show",
                                "Show them",
                                Some(IconName::ArrowRight),
                                Tone::Secondary,
                                kit::on(cx.listener(|this, _, _, cx| this.show_them(cx))),
                                cx,
                            )),
                    );
                }
            }
        }
        v_flex()
            .id("preview-help")
            .test_support()
            .key_context("PreviewHelp")
            .track_focus(&self.focus)
            .w(Tokens::size(560.0))
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
                    .items_center()
                    .gap(Tokens::space(2.0))
                    .child(
                        Icon::new(IconName::Image)
                            .size(Tokens::icon_lg())
                            .text_color(colors.text_faint),
                    )
                    .child(
                        div()
                            .id("preview-help-title")
                            .test_support()
                            .flex_1()
                            .text_size(Tokens::text_lg())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child("Why no preview?"),
                    )
                    .child(kit::button(
                        "preview-help-close",
                        "Close",
                        None,
                        Tone::Ghost,
                        kit::on(cx.listener(|_, _, _, cx| cx.emit(PreviewHelpEvent::Close))),
                        cx,
                    )),
            )
            .child(body)
            .children(footer)
    }
}
