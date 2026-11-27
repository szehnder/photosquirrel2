//! The Stacks tab of the Duplicates view (best-of-stacks, ADR-0135): stacks of similar shots with their pick, the
//! user's thresholds, and the keys that pick, take out, review and move. Nothing here changes a file or a group.
//!
//! Stacks are computed on a background executor from the service's cached inputs; a generation counter drops the
//! answer of a request a newer one replaced (a burst of threshold changes keeps only the last).

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{Icon, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{ContentId, FileId, StackSimilarity, StackThresholds};
use ps_service::stacks::{PickReason, StacksView};
use ui_theme::Tokens;
use ui_theme::fonts::MONO_FONT_FAMILY;

use super::DuplicatesView;
use crate::assets::IconName;
use crate::settings::AppSettings;
use crate::views::review_kit::*;
use crate::widgets::{eyebrow, kbd, segmented};

/// Sharpness within this fraction of the stack's sharpest reads as "Sharp" (the pick rule's tie band).
const SHARPNESS_TIE: f32 = 0.10;

/// The windows the header offers, in seconds.
pub const WINDOWS: [u32; 7] = [1, 2, 5, 10, 15, 30, 60];

/// The Stacks tab's state.
#[derive(Default)]
pub struct StacksTab {
    pub view: Option<Arc<StacksView>>,
    /// The open stack (index into the view's stacks).
    pub selected: usize,
    /// The focused shot of the open stack.
    pub focused: usize,
    /// Anchors of the stacks marked reviewed.
    pub reviewed: HashSet<ContentId>,
    generation: u64,
    pub loading: bool,
    pub scroll: UniformListScrollHandle,
    /// The shot last taken out, for Put back.
    pub taken_out_last: Option<FileId>,
    /// The stack exclusion just made, for Undo.
    pub undo_exclusion: Option<ps_service::stacks::StackExclusion>,
}

/// A thumbnail drawn as the top card of a small pile (best-of-stacks): `layers` thin offset edges on its right and
/// bottom say it is one of several. `inner` fills the top card.
pub fn stacked(inner: AnyElement, layers: usize, cx: &App) -> Div {
    let colors = *ui_theme::colors(cx);
    let off = Tokens::space(0.75);
    let n = layers as f32;
    let mut d = div().relative().size_full();
    for k in (1..=layers).rev() {
        let k = k as f32;
        d = d.child(
            div()
                .absolute()
                .top(off * k)
                .left(off * k)
                .right(off * (n - k))
                .bottom(off * (n - k))
                .rounded(Tokens::RADIUS_SM)
                .bg(colors.raised)
                .border_1()
                .border_color(colors.border_strong),
        );
    }
    d.child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .right(off * n)
            .bottom(off * n)
            .rounded(Tokens::RADIUS_SM)
            .overflow_hidden()
            .border_1()
            .border_color(colors.border_strong)
            .child(inner),
    )
}

/// A shot's sharpness relative to its stack's sharpest, in words.
pub fn sharpness_words(sharpness: f32, sharpest: f32) -> &'static str {
    if sharpest <= 0.0 || sharpness >= sharpest * (1.0 - SHARPNESS_TIE) {
        "Sharp"
    } else if sharpness >= sharpest * 0.6 {
        "Slightly soft"
    } else {
        "Soft"
    }
}

/// "3% blown highlights", "1% crushed shadows", or `None` under 1%.
pub fn clipping_words(high: f32, low: f32) -> Option<String> {
    let pct = |x: f32| (x * 100.0).round() as u32;
    match (pct(high), pct(low)) {
        (h, _) if h >= 1 => Some(format!("{h}% blown highlights")),
        (_, l) if l >= 1 => Some(format!("{l}% crushed shadows")),
        _ => None,
    }
}

/// The progress card's figure and line for stacks: ("12 left", "3 of 15 reviewed"), or the done state.
pub fn stacks_progress_text(stacks: u64, reviewed: u64) -> (String, String) {
    let left = stacks.saturating_sub(reviewed);
    if stacks > 0 && left == 0 {
        (
            format!(
                "All {stacks} stack{} reviewed",
                if stacks == 1 { "" } else { "s" }
            ),
            "Nothing left to review".to_owned(),
        )
    } else {
        (
            format!("{left} left"),
            format!("{reviewed} of {stacks} reviewed"),
        )
    }
}

impl DuplicatesView {
    /// The thresholds the tab shows: the settings' `[review]`.
    pub fn stack_thresholds(&self, cx: &App) -> StackThresholds {
        AppSettings::try_get(cx)
            .map(|s| s.review.thresholds())
            .unwrap_or_default()
    }

    pub fn stacks_tab(&self) -> &StacksTab {
        &self.stacks
    }

    /// Compute the stacks again (after a threshold change, a pick or a take-out), keeping the open stack when it
    /// still exists.
    pub fn reload_stacks(&mut self, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        let t = self.stack_thresholds(cx);
        // Before the first frame (start-up grace), compute on a helper thread and wait for it, so a scripted
        // launch that opens the tab shows the stacks in its one frame (as `DuplicatesView::load_now` does).
        if let Some(deadline) = crate::startup::StartupGrace::deadline(cx) {
            let src2 = src.clone();
            if let Some(Ok((view, reviewed))) =
                crate::startup::StartupGrace::within(deadline, move || {
                    let view = src2.stacks(t)?;
                    let anchors: Vec<ContentId> = view
                        .stacks
                        .stacks
                        .iter()
                        .filter_map(|s| view.anchor(s))
                        .collect();
                    let reviewed = src2.stacks_reviewed(anchors)?;
                    anyhow::Ok((view, reviewed))
                })
            {
                for s in view.stacks.stacks.iter().take(12) {
                    for f in &s.shots {
                        if let Some(c) = view.contents.get(f)
                            && std::time::Instant::now() < deadline
                        {
                            self.images.prime_thumb(c.digest);
                        }
                    }
                }
                self.stacks.generation += 1;
                self.stacks.loading = false;
                self.stacks.view = Some(view);
                self.stacks.reviewed = reviewed;
                cx.notify();
                return;
            }
        }
        self.stacks.generation += 1;
        let generation = self.stacks.generation;
        self.stacks.loading = true;
        let keep = self.open_stack().map(|s| s.shots[0]);
        let task = cx.background_executor().spawn(async move {
            let view = src.stacks(t)?;
            let anchors: Vec<ContentId> = view
                .stacks
                .stacks
                .iter()
                .filter_map(|s| view.anchor(s))
                .collect();
            let reviewed = src.stacks_reviewed(anchors)?;
            anyhow::Ok((view, reviewed))
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.stacks.generation != generation {
                    return;
                }
                this.stacks.loading = false;
                match r {
                    Ok((view, reviewed)) => {
                        let at = keep
                            .and_then(|f| view.index.get(&f).copied())
                            .unwrap_or(0)
                            .min(view.stacks.stacks.len().saturating_sub(1));
                        this.stacks.selected = at;
                        this.stacks.focused = this.stacks.focused.min(
                            view.stacks
                                .stacks
                                .get(at)
                                .map_or(0, |s| s.shots.len().saturating_sub(1)),
                        );
                        this.stacks.view = Some(view);
                        this.stacks.reviewed = reviewed;
                    }
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The open stack.
    pub fn open_stack(&self) -> Option<&ps_service::stacks::Stack> {
        self.stacks
            .view
            .as_ref()
            .and_then(|v| v.stacks.stacks.get(self.stacks.selected))
    }

    fn stack_is_reviewed(&self, i: usize) -> bool {
        self.stacks.view.as_ref().is_some_and(|v| {
            v.stacks
                .stacks
                .get(i)
                .and_then(|s| v.anchor(s))
                .is_some_and(|a| self.stacks.reviewed.contains(&a))
        })
    }

    pub fn select_stack(&mut self, i: usize, cx: &mut Context<'_, Self>) {
        let n = self
            .stacks
            .view
            .as_ref()
            .map_or(0, |v| v.stacks.stacks.len());
        if i < n {
            self.stacks.selected = i;
            self.stacks.focused = 0;
            self.compare = None;
            self.stacks
                .scroll
                .scroll_to_item(i, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    pub fn stack_step(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let n = self
            .stacks
            .view
            .as_ref()
            .map_or(0, |v| v.stacks.stacks.len()) as isize;
        if n == 0 {
            return;
        }
        let i = (self.stacks.selected as isize + delta).clamp(0, n - 1) as usize;
        self.select_stack(i, cx);
    }

    pub fn stack_focus(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let n = self.open_stack().map_or(0, |s| s.shots.len()) as isize;
        if n > 0 {
            self.stacks.focused = (self.stacks.focused as isize + delta).rem_euclid(n) as usize;
            cx.notify();
        }
    }

    /// Make shot `n` (1-based, time order) the open stack's pick.
    pub fn stack_pick(&mut self, n: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self
            .open_stack()
            .and_then(|s| s.shots.get(n.wrapping_sub(1)))
            .copied()
        else {
            return;
        };
        if !self.may_edit(cx) || self.open_stack().is_some_and(|s| s.pick == file) {
            return;
        }
        let t = self.stack_thresholds(cx);
        self.run(
            cx,
            move |s| s.stack_pick(t, file),
            |this, (), cx| {
                this.notice = Some("Picked by you. No file was changed.".into());
                this.reload_stacks(cx);
                crate::views::library::refresh_after_decision(cx);
            },
        );
    }

    /// Take the focused shot out of the open stack.
    pub fn stack_take_out(&mut self, cx: &mut Context<'_, Self>) {
        let Some(file) = self
            .open_stack()
            .and_then(|s| s.shots.get(self.stacks.focused))
            .copied()
        else {
            return;
        };
        if !self.may_edit(cx) {
            return;
        }
        let t = self.stack_thresholds(cx);
        self.run(
            cx,
            move |s| s.stack_take_out(t, file),
            move |this, (), cx| {
                this.notice = Some(
                    "Taken out of the stack: it is never stacked with these shots again. No file was changed."
                        .into(),
                );
                this.stacks.taken_out_last = Some(file);
                this.reload_stacks(cx);
                crate::views::library::refresh_after_decision(cx);
            },
        );
    }

    /// Keep none of the open stack: every copy of every shot is excluded and the stack leaves the list; the
    /// notice offers Undo. Nothing on disk changes.
    pub fn stack_exclude(&mut self, cx: &mut Context<'_, Self>) {
        if !self.may_edit(cx) {
            return;
        }
        let Some(s) = self.open_stack() else { return };
        let n = s.shots.len();
        let index = self.stacks.selected;
        let t = self.stack_thresholds(cx);
        self.run(
            cx,
            move |src| src.stack_exclude(t, index),
            move |this, x, cx| {
                this.notice = Some(format!(
                    "Excluded the stack: none of its {n} shots (and their copies) goes to the library. Nothing on \
                     disk changed."
                ));
                this.stacks.undo_exclusion = Some(x);
                this.compare = None;
                this.reload_stacks(cx);
                this.reload(cx);
                crate::views::library::refresh_after_decision(cx);
            },
        );
    }

    /// Undo the stack exclusion the notice names.
    pub fn stack_undo_exclude(&mut self, cx: &mut Context<'_, Self>) {
        let Some(x) = self.stacks.undo_exclusion.take() else {
            return;
        };
        self.run(
            cx,
            move |src| src.stack_undo_exclude(x),
            |this, (), cx| {
                this.notice = Some("Exclusion undone: the stack is back.".into());
                this.reload_stacks(cx);
                this.reload(cx);
                crate::views::library::refresh_after_decision(cx);
            },
        );
    }

    /// Open the duplicate group of `keeper` in the Groups tab.
    pub fn open_group_of(&mut self, keeper: FileId, cx: &mut Context<'_, Self>) {
        let ix = self.rows().iter().position(|r| r.keeper == keeper);
        self.set_tab(super::Tab::Groups, cx);
        if let Some(ix) = ix {
            self.select_index(ix, cx);
        }
    }

    /// Put the last shot taken out back.
    pub fn stack_put_back(&mut self, cx: &mut Context<'_, Self>) {
        let Some(file) = self.stacks.taken_out_last.take() else {
            return;
        };
        self.run(
            cx,
            move |s| s.stack_put_back(file),
            |this, (), cx| {
                this.notice = Some("Put back.".into());
                this.reload_stacks(cx);
            },
        );
    }

    /// Mark the open stack reviewed (or not) and open the next unreviewed one.
    pub fn stack_mark_reviewed(&mut self, cx: &mut Context<'_, Self>) {
        let Some(view) = self.stacks.view.clone() else {
            return;
        };
        let i = self.stacks.selected;
        let Some(anchor) = view.stacks.stacks.get(i).and_then(|s| view.anchor(s)) else {
            return;
        };
        let reviewed = !self.stacks.reviewed.contains(&anchor);
        let n = view.stacks.stacks.len();
        let next = (1..=n)
            .map(|k| (i + k) % n)
            .find(|j| *j != i && !self.stack_is_reviewed(*j));
        self.run(
            cx,
            move |s| s.set_stack_reviewed(anchor, reviewed),
            move |this, (), cx| {
                if reviewed {
                    this.stacks.reviewed.insert(anchor);
                    if let Some(j) = next {
                        this.select_stack(j, cx);
                    }
                } else {
                    this.stacks.reviewed.remove(&anchor);
                }
                cx.notify();
            },
        );
    }

    /// Compare the pick with the focused shot.
    pub fn stack_compare(&mut self, cx: &mut Context<'_, Self>) {
        if self.compare.take().is_some() {
            cx.notify();
            return;
        }
        let Some(s) = self.open_stack() else { return };
        let Some(&b) = s.shots.get(self.stacks.focused) else {
            return;
        };
        let a = s.pick;
        if a != b {
            self.compare = Some(super::Compare {
                a,
                b,
                zoom: 1.0,
                center: (0.5, 0.5),
            });
            cx.notify();
        }
    }

    /// Save new thresholds and recompute.
    pub fn set_stack_thresholds(&mut self, t: StackThresholds, cx: &mut Context<'_, Self>) {
        if let Err(errors) = AppSettings::update(cx, |s| s.review.set_thresholds(t)) {
            self.error = errors.first().map(|e| e.message.clone());
        }
        self.reload_stacks(cx);
    }

    /// The thumbnail key and name of a stacked shot (for the compare loupe).
    pub(super) fn stack_shot_thumb(&self, f: FileId) -> Option<(Option<[u8; 32]>, String)> {
        let v = self.stacks.view.as_ref()?;
        let name = v.names.get(&f)?.clone();
        Some((v.contents.get(&f).map(|c| c.digest), name))
    }

    pub(super) fn render_stacks_list(
        &mut self,
        tabs: AnyElement,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let t = self.stack_thresholds(cx);
        let view = self.stacks.view.clone();
        let n = view.as_ref().map_or(0, |v| v.stacks.stacks.len());
        let reviewed = (0..n).filter(|i| self.stack_is_reviewed(*i)).count() as u64;
        let (main, sub) = if view.is_some() {
            stacks_progress_text(n as u64, reviewed)
        } else {
            ("…".to_owned(), "Computing stacks".to_owned())
        };
        let done = n > 0 && reviewed as usize >= n;
        let fraction = if n == 0 {
            0.0
        } else {
            reviewed as f32 / n as f32
        };
        let tone = if done { colors.success } else { colors.accent };
        let card = v_flex()
            .id("stacks-progress")
            .test_support()
            .p(Tokens::space(3.0))
            .gap(Tokens::space(1.0))
            .rounded(Tokens::RADIUS_LG)
            .bg(ui_theme::Colors::tint(tone, 0.12))
            .child(
                div()
                    .text_size(Tokens::text_xl())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.tone_text(tone))
                    .child(main),
            )
            .child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_muted)
                    .child(sub),
            )
            .child(progress_bar_on(
                fraction,
                ui_theme::Colors::tint(tone, 0.2),
                cx,
            ));
        let weak = cx.weak_entity();
        let window_button = Button::new("stacks-window")
            .label(format!("Within {} s", t.window_s))
            .compact()
            .dropdown_caret(true)
            .dropdown_menu({
                let weak = weak.clone();
                move |menu, _, _| {
                    let mut menu = menu.label("Stack shots taken within");
                    for w in WINDOWS {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(format!("{w} s")).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    let t = StackThresholds {
                                        window_s: w,
                                        ..v.stack_thresholds(cx)
                                    };
                                    v.set_stack_thresholds(t, cx);
                                });
                            },
                        ));
                    }
                    menu
                }
            });
        let similarity = segmented(
            "stacks-similarity",
            &["Strict", "Balanced", "Loose"],
            StackSimilarity::ALL
                .iter()
                .position(|s| *s == t.similarity)
                .unwrap_or(1),
            {
                let weak = weak.clone();
                move |ix, _, cx| {
                    let _ = weak.update(cx, |v, cx| {
                        let t = StackThresholds {
                            similarity: StackSimilarity::ALL[ix.min(2)],
                            ..v.stack_thresholds(cx)
                        };
                        v.set_stack_thresholds(t, cx);
                    });
                }
            },
            cx,
        );
        let count = match &view {
            Some(v) => format!(
                "{} stack{} of {} shots",
                v.stacks.stacks.len(),
                if v.stacks.stacks.len() == 1 { "" } else { "s" },
                v.stacks.stacked_shots()
            ),
            None => "Computing stacks…".to_owned(),
        };
        let header = v_flex()
            .flex_none()
            .px(Tokens::space(4.0))
            .pt(Tokens::space(4.0))
            .pb(Tokens::space(3.0))
            .gap(Tokens::space(2.5))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(Tokens::text_lg())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Duplicates"),
            )
            .child(card)
            .child(tabs)
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .flex_wrap()
                    .items_center()
                    .child(window_button)
                    .child(similarity),
            )
            .child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_muted)
                    .child(format!(
                        "Shots taken within {} s of each other that look alike. Similarity sets how alike.",
                        t.window_s
                    )),
            )
            .child(
                div()
                    .id("stacks-count")
                    .test_support()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(count),
            );
        let rows = uniform_list("stacks-list", n, {
            let view = view.clone();
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let colors = *ui_theme::colors(cx);
                let Some(v) = view.clone() else {
                    return Vec::new();
                };
                range
                    .map(|ix| {
                        let s = &v.stacks.stacks[ix];
                        let on = ix == this.stacks.selected;
                        let key = v.contents.get(&s.pick).map(|c| c.digest);
                        let img = this.images.thumb(key, cx);
                        let reviewed = this.stack_is_reviewed(ix);
                        h_flex()
                            .id(("stacks-row", ix))
                            .test_support()
                            .w_full()
                            .px(Tokens::space(4.0))
                            .py(Tokens::space(2.0))
                            .gap(Tokens::space(3.0))
                            .when(on, |d| d.bg(colors.accent_soft))
                            .hover(|d| d.bg(colors.hover))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.select_stack(ix, cx)))
                            .child(
                                div()
                                    .id(("stacks-row-pile", ix))
                                    .test_support()
                                    .size(Tokens::size(50.0))
                                    .flex_none()
                                    .child(stacked(
                                        photo(img, false, cx),
                                        s.shots.len().min(3) - 1,
                                        cx,
                                    )),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(Tokens::text_sm())
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(
                                                v.names.get(&s.pick).cloned().unwrap_or_default(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .font_family(MONO_FONT_FAMILY)
                                            .text_size(Tokens::text_2xs())
                                            .text_color(colors.text_muted)
                                            .child(format!(
                                                "{} shots · {:.1} s",
                                                s.shots.len(),
                                                s.span_ms() as f64 / 1000.0
                                            )),
                                    ),
                            )
                            .when(reviewed, |d| {
                                d.child(
                                    Icon::new(IconName::CircleCheck)
                                        .size(Tokens::icon_sm())
                                        .text_color(colors.success),
                                )
                            })
                            .into_any_element()
                    })
                    .collect()
            })
        })
        .track_scroll(&self.stacks.scroll)
        .flex_1();
        v_flex()
            .w(Tokens::size(320.0))
            .flex_none()
            .h_full()
            .bg(colors.panel)
            .border_r_1()
            .border_color(colors.border)
            .child(header)
            .when(n == 0 && view.is_some(), |d| {
                d.child(
                    div()
                        .id("stacks-empty")
                        .test_support()
                        .p(Tokens::space(6.0))
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child("No stacks at these settings. Try a longer window or Loose."),
                )
            })
            .child(rows)
            .into_any_element()
    }

    pub(super) fn render_stack_detail(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let Some(view) = self.stacks.view.clone() else {
            return div()
                .id("stacks-detail-status")
                .test_support()
                .flex_1()
                .p(Tokens::space(6.0))
                .text_color(if self.error.is_some() {
                    colors.danger
                } else {
                    colors.text_muted
                })
                .child(match &self.error {
                    Some(e) => format!("The stacks could not be computed: {e}"),
                    None => "Computing stacks…".to_owned(),
                })
                .into_any_element();
        };
        let Some(s) = view.stacks.stacks.get(self.stacks.selected).cloned() else {
            return div()
                .flex_1()
                .p(Tokens::space(6.0))
                .text_color(colors.text_muted)
                .child("No stacks at these settings.")
                .into_any_element();
        };
        let sharpest = s
            .shots
            .iter()
            .filter_map(|f| view.quality.get(f).map(|q| q.sharpness))
            .fold(0.0f32, f32::max);
        let reviewed = self.stack_is_reviewed(self.stacks.selected);
        let header = h_flex()
            .flex_none()
            .px(Tokens::space(5.0))
            .py(Tokens::space(3.0))
            .gap(Tokens::space(3.0))
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("stacks-title")
                            .test_support()
                            .text_size(Tokens::text_xl())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!(
                                "Stack {} · {} shots",
                                self.stacks.selected + 1,
                                s.shots.len()
                            )),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child(format!(
                                "{:.1} s from first to last · the pick goes to Best of exports",
                                s.span_ms() as f64 / 1000.0
                            )),
                    ),
            )
            .child(action_button(
                "stacks-exclude",
                "Exclude stack",
                Some(IconName::EyeOff),
                Some("E"),
                Tone::Caution,
                cx,
                |this, _, cx| this.stack_exclude(cx),
            ))
            .child(action_button(
                "stacks-compare",
                "Compare",
                Some(IconName::Columns2),
                Some("Space"),
                Tone::Normal,
                cx,
                |this, _, cx| this.stack_compare(cx),
            ))
            .child(action_button(
                "stacks-reviewed",
                if reviewed {
                    "Reviewed"
                } else {
                    "Mark reviewed"
                },
                Some(IconName::Check),
                Some("R"),
                if reviewed {
                    Tone::Normal
                } else {
                    Tone::Primary
                },
                cx,
                |this, _, cx| this.stack_mark_reviewed(cx),
            ));
        let cards: Vec<AnyElement> = s
            .shots
            .iter()
            .enumerate()
            .map(|(ix, f)| {
                let n = ix + 1;
                let pick = *f == s.pick;
                let focused = ix == self.stacks.focused;
                let key = view.contents.get(f).map(|c| c.digest);
                let img = self.images.thumb(key, cx);
                let q = view.quality.get(f);
                let words = match q {
                    Some(q) => {
                        let mut w = sharpness_words(q.sharpness, sharpest).to_owned();
                        if let Some(c) = clipping_words(q.clipped_high, q.clipped_low) {
                            w = format!("{w} · {c}");
                        }
                        w
                    }
                    None => "Scores pending".to_owned(),
                };
                let file = *f;
                v_flex()
                    .id(("stacks-shot", ix))
                    .test_support()
                    .w(Tokens::size(220.0))
                    .gap(Tokens::space(2.0))
                    .p(Tokens::space(2.0))
                    .rounded(Tokens::RADIUS_LG)
                    .bg(colors.raised)
                    .border_1()
                    .border_color(if pick {
                        colors.accent
                    } else if focused {
                        colors.border_strong
                    } else {
                        colors.border
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stacks.focused = ix;
                        cx.notify();
                    }))
                    .child(match view.copies.get(f).map(|c| c.len()) {
                        Some(c) => div()
                            .id(("stacks-shot-pile", ix))
                            .test_support()
                            .h(Tokens::size(156.0))
                            .child(stacked(photo(img, false, cx), (c - 1).min(2), cx))
                            .into_any_element(),
                        None => div()
                            .id(("stacks-shot-photo", ix))
                            .h(Tokens::size(150.0))
                            .rounded(Tokens::RADIUS_MD)
                            .overflow_hidden()
                            .child(photo(img, false, cx))
                            .into_any_element(),
                    })
                    .child(
                        h_flex()
                            .gap(Tokens::space(1.5))
                            .child(kbd(n.to_string(), cx))
                            .when(pick, |e| {
                                e.child(icon_chip(
                                    format!("stacks-best-of-{ix}"),
                                    IconName::Crown,
                                    "Best of",
                                    colors.on_accent,
                                    colors.accent,
                                    cx,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(view.names.get(f).cloned().unwrap_or_default()),
                    )
                    .when_some(view.copies.get(f).map(|c| c.len()), |e, c| {
                        e.child(
                            h_flex()
                                .gap(Tokens::space(1.5))
                                .flex_wrap()
                                .items_center()
                                .child(
                                    div()
                                        .id(("stacks-copies", ix))
                                        .test_support()
                                        .text_size(Tokens::text_xs())
                                        .text_color(colors.text_muted)
                                        .child(format!("{c} copies, kept as one shot")),
                                )
                                .child(action_button(
                                    format!("stacks-open-group-{n}"),
                                    "Open group",
                                    Some(IconName::Copy),
                                    None,
                                    Tone::Quiet,
                                    cx,
                                    move |this, _, cx| this.open_group_of(file, cx),
                                )),
                        )
                    })
                    .child(
                        div()
                            .id(("stacks-words", ix))
                            .test_support()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child(words),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(1.5))
                            .flex_wrap()
                            .when(!pick, |e| {
                                e.child(action_button(
                                    format!("stacks-pick-{n}"),
                                    "Pick",
                                    Some(IconName::Crown),
                                    (n <= 9).then(|| {
                                        ["1", "2", "3", "4", "5", "6", "7", "8", "9"][n - 1]
                                    }),
                                    Tone::Normal,
                                    cx,
                                    move |this, _, cx| this.stack_pick(n, cx),
                                ))
                            })
                            .child(action_button(
                                format!("stacks-take-out-{n}"),
                                "Take out of stack",
                                Some(IconName::Unlink),
                                focused.then_some("S"),
                                Tone::Normal,
                                cx,
                                move |this, _, cx| {
                                    this.stacks.focused = s_index_of(this, file).unwrap_or(0);
                                    this.stack_take_out(cx);
                                },
                            )),
                    )
                    .into_any_element()
            })
            .collect();
        let why = v_flex()
            .id("stacks-why")
            .test_support()
            .gap(Tokens::space(1.0))
            .child(eyebrow("Why this one", cx))
            .child(
                div()
                    .text_size(Tokens::text_sm())
                    .child(match s.reason {
                        PickReason::PickedByYou => "Picked by you".to_owned(),
                        PickReason::Sharpest => "Sharpest of the stack".to_owned(),
                        PickReason::LeastClipped => {
                            "As sharp as the others, with the fewest blown highlights or crushed shadows".to_owned()
                        }
                        PickReason::BestCopy => "Equally sharp and exposed; the best copy".to_owned(),
                        PickReason::FirstShot => "Tied on every score, so the earliest shot".to_owned(),
                    }),
            );
        let put_back = self.stacks.taken_out_last.is_some();
        let notice = self.notice.clone().map(|text| {
            h_flex()
                .id("stacks-notice")
                .test_support()
                .px(Tokens::space(3.0))
                .py(Tokens::space(2.0))
                .gap(Tokens::space(2.0))
                .rounded(Tokens::RADIUS_MD)
                .bg(ui_theme::Colors::tint(colors.success, 0.12))
                .text_size(Tokens::text_sm())
                .child(div().flex_1().child(text))
                .when(self.stacks.undo_exclusion.is_some(), |b| {
                    b.child(action_button(
                        "stacks-undo-exclude",
                        "Undo",
                        None,
                        None,
                        Tone::Normal,
                        cx,
                        |this, _, cx| this.stack_undo_exclude(cx),
                    ))
                })
        });
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .child(
                v_flex()
                    .id("stacks-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(Tokens::space(5.0))
                    .py(Tokens::space(4.0))
                    .gap(Tokens::space(4.0))
                    .children(notice)
                    .child(why)
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .items_baseline()
                            .child(eyebrow(format!("Shots in time order · {}", s.shots.len()), cx))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_faint)
                                    .child("1–9 pick · ←/→ focus · S take out · E keep none · Space compare · R reviewed · J/K next/previous"),
                            ),
                    )
                    .child(
                        h_flex()
                            .id("stacks-shots")
                            .test_support()
                            .flex_wrap()
                            .items_start()
                            .gap(Tokens::space(3.0))
                            .children(cards),
                    )
                    .when(put_back, |e| {
                        e.child(action_button(
                            "stacks-put-back",
                            "Put the shot taken out back",
                            Some(IconName::Undo2),
                            None,
                            Tone::Quiet,
                            cx,
                            |this, _, cx| this.stack_put_back(cx),
                        ))
                    }),
            )
            .into_any_element()
    }
}

fn s_index_of(this: &DuplicatesView, f: FileId) -> Option<usize> {
    this.open_stack()?.shots.iter().position(|x| *x == f)
}
