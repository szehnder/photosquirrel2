//! The command palette: every registered command, fuzzy-searchable, with its shortcut.
//!
//! Ranking ([`rank`]) is pure and tested without a window. The view is an overlay owned by the shell, with
//! its own input and keyboard navigation (Up/Down/Enter; Esc closes).

use std::ops::Range;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ui_theme::Tokens;

use crate::actions::palette::{Confirm, SelectNext, SelectPrev};
use crate::assets::IconName;
use crate::keymap;
use crate::widgets::kbd;

/// Palette metadata for one action.
pub struct CommandMeta {
    pub category: &'static str,
    pub title: &'static str,
    pub keywords: &'static [&'static str],
    pub action: Box<dyn Action>,
}

impl CommandMeta {
    pub fn new(category: &'static str, title: &'static str, action: impl Action) -> Self {
        Self {
            category,
            title,
            keywords: &[],
            action: Box::new(action),
        }
    }

    pub fn keywords(mut self, keywords: &'static [&'static str]) -> Self {
        self.keywords = keywords;
        self
    }
}

/// All commands, in registration order (curated: most useful first).
#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<CommandMeta>,
}

impl Global for CommandRegistry {}

impl CommandRegistry {
    pub fn register(cx: &mut App, meta: CommandMeta) {
        cx.default_global::<CommandRegistry>().commands.push(meta);
    }

    pub fn commands(cx: &App) -> &[CommandMeta] {
        cx.try_global::<CommandRegistry>()
            .map(|r| r.commands.as_slice())
            .unwrap_or_default()
    }

    /// The registered command for an action name (`shell::Rescan`), for `--command`.
    pub fn find(cx: &App, name: &str) -> Option<Box<dyn Action>> {
        Self::commands(cx)
            .iter()
            .find(|c| c.action.name() == name)
            .map(|c| c.action.boxed_clone())
    }
}

/// The text a candidate is matched on.
pub struct Candidate<'a> {
    pub title: &'a str,
    pub category: &'a str,
    pub keywords: &'a [&'a str],
}

/// One ranked row: index into the candidates and the title's matched byte ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub index: usize,
    pub highlights: Vec<Range<usize>>,
}

/// Score `query` against `text`: a contiguous match beats a scattered one, a match at a word start beats
/// one inside a word. `None` when not every query character appears in order.
pub fn fuzzy(query: &str, text: &str) -> Option<(u32, Vec<Range<usize>>)> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let lower = text.to_lowercase();
    // Contiguous: best when at a word start.
    if lower.len() == text.len()
        && let Some(pos) = lower.find(&q.iter().collect::<String>())
    {
        let word_start = pos == 0 || !text.as_bytes()[pos - 1].is_ascii_alphanumeric();
        let qlen: usize = q.iter().map(|c| c.len_utf8()).sum();
        let score = 1000 + if word_start { 500 } else { 0 } - pos.min(200) as u32;
        #[allow(clippy::single_range_in_vec_init)] // one highlighted span, not a range of indices
        return Some((score, vec![pos..pos + qlen]));
    }
    // Scattered subsequence.
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut qi = 0;
    let mut score = 0u32;
    let mut prev_alnum = false;
    for (i, ch) in text.char_indices() {
        if qi < q.len() && ch.to_lowercase().eq(std::iter::once(q[qi])) {
            let end = i + ch.len_utf8();
            match ranges.last_mut() {
                Some(r) if r.end == i => {
                    r.end = end;
                    score += 8;
                }
                _ => ranges.push(i..end),
            }
            score += if prev_alnum { 1 } else { 10 };
            qi += 1;
        }
        prev_alnum = ch.is_alphanumeric();
    }
    (qi == q.len()).then_some((score, ranges))
}

/// Filter and order candidates. Empty query: registration order. Otherwise by score (title matches beat
/// keyword and category matches), ties by registration order.
pub fn rank(candidates: &[Candidate<'_>], query: &str) -> Vec<Ranked> {
    let query = query.trim();
    let mut rows: Vec<(u32, Ranked)> = Vec::new();
    for (index, c) in candidates.iter().enumerate() {
        if query.is_empty() {
            rows.push((
                0,
                Ranked {
                    index,
                    highlights: Vec::new(),
                },
            ));
            continue;
        }
        let title = fuzzy(query, c.title);
        let other = c
            .keywords
            .iter()
            .chain(std::iter::once(&c.category))
            .filter_map(|k| fuzzy(query, k).map(|(s, _)| s * 3 / 4))
            .max();
        let best = match (title, other) {
            (Some((s, h)), o) => Some((s.max(o.unwrap_or(0)), h)),
            (None, Some(o)) => Some((o, Vec::new())),
            (None, None) => None,
        };
        if let Some((score, highlights)) = best {
            rows.push((score, Ranked { index, highlights }));
        }
    }
    rows.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then(a.index.cmp(&b.index)));
    rows.into_iter().map(|(_, r)| r).collect()
}

/// One row as shown: category, title, highlights, shortcut.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteRow {
    pub category: &'static str,
    pub title: &'static str,
    pub highlights: Vec<Range<usize>>,
    pub shortcut: Option<String>,
    pub action_name: &'static str,
}

pub enum PaletteEvent {
    /// Run this action (the palette has closed itself in the shell's handler).
    Run(Box<dyn Action>),
    Dismiss,
}

/// The palette overlay.
pub struct CommandPalette {
    input: Entity<InputState>,
    rows: Vec<PaletteRow>,
    selected: usize,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command…"));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.rerank(cx);
            }
        });
        let mut this = Self {
            input,
            rows: Vec::new(),
            selected: 0,
            focus: cx.focus_handle(),
            _subscription: subscription,
        };
        this.rerank(cx);
        this
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn query(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    /// Rows in display order (tests read these).
    pub fn rows(&self) -> &[PaletteRow] {
        &self.rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Set the query programmatically (scripts and tests).
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(query.to_owned(), window, cx));
        self.rerank(cx);
    }

    fn rerank(&mut self, cx: &mut Context<'_, Self>) {
        let query = self.input.read(cx).value().to_string();
        let commands = CommandRegistry::commands(cx);
        let candidates: Vec<Candidate<'_>> = commands
            .iter()
            .map(|c| Candidate {
                title: c.title,
                category: c.category,
                keywords: c.keywords,
            })
            .collect();
        self.rows = rank(&candidates, &query)
            .into_iter()
            .map(|r| {
                let meta = &commands[r.index];
                PaletteRow {
                    category: meta.category,
                    title: meta.title,
                    highlights: r.highlights,
                    shortcut: keymap::shortcut_for(meta.action.as_ref()),
                    action_name: meta.action.name(),
                }
            })
            .collect();
        self.selected = 0;
        cx.notify();
    }

    fn confirm_index(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        if let Some(action) = CommandRegistry::find(cx, row.action_name) {
            cx.emit(PaletteEvent::Run(action));
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + 1) % self.rows.len();
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + self.rows.len() - 1) % self.rows.len();
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.confirm_index(self.selected, cx);
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let emphasis = HighlightStyle {
            color: Some(colors.accent_text),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let selected = ix == self.selected;
            let text = StyledText::new(row.title)
                .with_highlights(row.highlights.iter().map(|r| (r.clone(), emphasis)));
            h_flex()
                .id(SharedString::from(format!("palette-row-{ix}")))
                .test_support()
                .flex_none()
                .h(Tokens::size(34.0))
                .px(Tokens::space(3.0))
                .mx(Tokens::space(1.5))
                .gap(Tokens::space(3.0))
                .rounded(Tokens::RADIUS_MD)
                .when(selected, |d| d.bg(colors.accent_soft))
                .hover(|d| d.bg(colors.hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.confirm_index(ix, cx)))
                .child(
                    div()
                        .w(Tokens::size(84.0))
                        .flex_none()
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_faint)
                        .child(row.category),
                )
                .child(div().flex_1().min_w_0().text_color(colors.text).child(text))
                .children(row.shortcut.clone().map(|s| kbd(s, cx)))
        });
        v_flex()
            .id("command-palette")
            .test_support()
            .key_context(keymap::PALETTE)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::confirm))
            .w(Tokens::palette_width())
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.overlay)
            .border_1()
            .border_color(colors.border_strong)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .px(Tokens::space(3.0))
                    .py(Tokens::space(2.0))
                    .gap(Tokens::space(2.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        Icon::new(IconName::Command)
                            .size(Tokens::icon_md())
                            .text_color(colors.text_faint),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(&self.input)
                                .appearance(false)
                                .aria_label("Search commands"),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("palette-list")
                    .py(Tokens::space(1.5))
                    .max_h(Tokens::palette_list_height())
                    .overflow_y_scroll()
                    .children(rows)
                    .when(self.rows.is_empty(), |d| {
                        d.child(
                            div()
                                .id("palette-empty")
                                .test_support()
                                .px(Tokens::space(4.0))
                                .py(Tokens::space(3.0))
                                .text_color(colors.text_muted)
                                .child("No matching commands"),
                        )
                    }),
            )
            .child(
                h_flex()
                    .px(Tokens::space(3.0))
                    .py(Tokens::space(1.5))
                    .gap(Tokens::space(3.0))
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(
                        h_flex()
                            .gap(Tokens::space(1.0))
                            .child(kbd("↑↓", cx))
                            .child("navigate"),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(1.0))
                            .child(kbd("Enter", cx))
                            .child("run"),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(1.0))
                            .child(kbd("Esc", cx))
                            .child("close"),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    fn c<'a>(category: &'a str, title: &'a str) -> Candidate<'a> {
        Candidate {
            title,
            category,
            keywords: &[],
        }
    }

    #[test]
    fn contiguous_word_start_matches_rank_first() {
        let cands = [
            c("Library", "Rescan All Sources"),
            c("View", "Show Reports"),
            c("View", "Toggle Inspector"),
        ];
        let rows = rank(&cands, "rescan");
        assert_eq!(rows[0].index, 0);
        assert_eq!(rows[0].highlights, vec![0..6]);
        let rows = rank(&cands, "tgi");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 2);
        assert!(rank(&cands, "zzz").is_empty());
        assert_eq!(rank(&cands, "").len(), 3);
    }
}
