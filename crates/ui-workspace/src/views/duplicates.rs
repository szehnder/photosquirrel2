//! The Duplicates view (change `duplicate-review`, spec `duplicate-review`): a virtualised group list with
//! sorts, filters and review progress; a group compare with the keeper large, every copy side by side, the
//! "Why this copy" reasons and why each copy joined; keeper override, "not the same photo" separation
//! (ADR-0033, the safety valve), group merges; a synchronised compare loupe; and the similar-shot suggestions
//! panel, where bursts are shown side by side and never merged (ADR-0013, ADR-0035).
//!
//! Keyboard first: J/K (or ↓/↑) next/previous group, 1-9 choose keeper, ←/→ focus a copy, S separate the
//! focused copy, R mark reviewed (and open the next unreviewed group), Space compare, X dismiss a suggestion,
//! B switch between groups and similar shots. Every key is a GPUI action, so it is in the command palette.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{Icon, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::FileId;
use ps_service::groups::{GroupMember, MemberEdge};
use ps_service::review::{
    CLOSE_DHASH, CLOSE_PHASH, MemberCard, ReviewDetail, ReviewFilter, ReviewList, ReviewRow,
    ReviewSort, SuggestionCard,
};
use ui_gallery::source::SharedSource;
use ui_gallery::theme::lpx;
use ui_theme::Tokens;
use ui_theme::fonts::MONO_FONT_FAMILY;

use crate::ShellGlobals;
use crate::assets::IconName;
use crate::gallery_inspector::format_bytes;
use crate::inspector::format_count;
use crate::palette::{CommandMeta, CommandRegistry};
use crate::review_source::{ReviewGlobal, ReviewSource};
use crate::views::review_kit::*;
use crate::views::{ViewId, ViewRegistry};
use crate::widgets::{eyebrow, kbd, segmented};

/// Key context of the Duplicates view.
pub const CONTEXT: &str = "Duplicates";
const KEYS: &str = "Duplicates && !Input";

actions!(
    duplicates,
    [
        /// Next group (J).
        NextGroup,
        /// Previous group (K).
        PrevGroup,
        /// Mark the group reviewed and open the next unreviewed one (R).
        MarkReviewed,
        /// "Not the same photo": separate the focused copy (S).
        SeparateFocused,
        /// Compare the keeper and the focused copy side by side (Space).
        ToggleCompare,
        /// Dismiss the selected similar-shot suggestion (X).
        DismissSuggestion,
        FocusNextMember,
        FocusPrevMember,
        ConfirmPending,
        CancelPending,
        /// Switch between groups and similar shots (B).
        ToggleSuggestions,
        CompareZoomIn,
        CompareZoomOut,
        CompareZoomFit,
    ]
);

/// Choose the n-th copy (1-based, display order) as keeper.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = duplicates, no_json)]
pub struct PickKeeper(pub usize);

/// Register the view, its keys and palette commands.
pub fn register(cx: &mut App) {
    let c = Some(KEYS);
    let mut bindings = vec![
        KeyBinding::new("down", NextGroup, c),
        KeyBinding::new("up", PrevGroup, c),
        KeyBinding::new("j", NextGroup, c),
        KeyBinding::new("k", PrevGroup, c),
        KeyBinding::new("r", MarkReviewed, c),
        KeyBinding::new("s", SeparateFocused, c),
        KeyBinding::new("space", ToggleCompare, c),
        KeyBinding::new("x", DismissSuggestion, c),
        KeyBinding::new("right", FocusNextMember, c),
        KeyBinding::new("left", FocusPrevMember, c),
        KeyBinding::new("enter", ConfirmPending, c),
        KeyBinding::new("escape", CancelPending, c),
        KeyBinding::new("b", ToggleSuggestions, c),
        KeyBinding::new("=", CompareZoomIn, c),
        KeyBinding::new("-", CompareZoomOut, c),
        KeyBinding::new("0", CompareZoomFit, c),
    ];
    for (n, key) in ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
        .iter()
        .enumerate()
    {
        bindings.push(KeyBinding::new(key, PickKeeper(n + 1), c));
    }
    cx.bind_keys(bindings);
    {
        for meta in [
            CommandMeta::new("Duplicates", "Next Group", NextGroup).keywords(&["j", "review"]),
            CommandMeta::new("Duplicates", "Previous Group", PrevGroup).keywords(&["k"]),
            CommandMeta::new("Duplicates", "Mark Group Reviewed", MarkReviewed)
                .keywords(&["done", "ok"]),
            CommandMeta::new(
                "Duplicates",
                "Not the Same Photo (Separate Copy)",
                SeparateFocused,
            )
            .keywords(&["split", "different", "wrong"]),
            CommandMeta::new("Duplicates", "Compare Side by Side", ToggleCompare)
                .keywords(&["loupe", "zoom"]),
            CommandMeta::new("Duplicates", "Similar Shots", ToggleSuggestions)
                .keywords(&["burst", "suggestions"]),
            CommandMeta::new(
                "Duplicates",
                "Dismiss Similar-Shot Suggestion",
                DismissSuggestion,
            ),
        ] {
            CommandRegistry::register(cx, meta);
        }
    }
    ViewRegistry::register(cx, ViewId::Duplicates, |_, window, cx| {
        let source = ReviewGlobal::get(cx);
        let gallery = cx.global::<ShellGlobals>().gallery.clone();
        let view = cx.new(|cx| DuplicatesView::new(source, gallery, window, cx));
        cx.set_global(Current(view.downgrade()));
        view.into()
    });
}

struct Current(WeakEntity<DuplicatesView>);

impl Global for Current {}

/// The open Duplicates view (tests, the palette).
pub fn current(cx: &App) -> Option<Entity<DuplicatesView>> {
    cx.try_global::<Current>()?.0.upgrade()
}

/// "Keeper #1 4032×3024 · 3.1 MB   vs   next best #2 1024×768 · 210 KB": the keeper beside the copy the keeper
/// reasons were explained against (the group's runner-up, ADR-0074), so "Why this copy" and this line name the
/// same member. Groups stored before the runner-up was recorded fall back to the first copy that is not
/// byte-identical to the keeper. When every other copy is identical, the line says so.
pub fn why_compare_line(d: &ReviewDetail) -> Option<String> {
    let k = d.group.member(d.group.keeper)?;
    let others = d.members().iter().filter(|m| !m.in_keeper_set && !m.keeper);
    let identical = |m: &GroupMember| {
        m.edge == MemberEdge::Exact && d.matched_against(m.file) == Some(d.group.keeper)
    };
    let next = match d.group.runner_up.and_then(|f| d.group.member(f)) {
        Some(r) if !r.keeper && !r.in_keeper_set => (!identical(r)).then_some(r),
        _ => others.clone().find(|m| !identical(m)),
    };
    let Some(r) = next else {
        let n = others.count();
        if n == 0 {
            return None;
        }
        let rest = if n == 1 {
            "The other copy is identical, byte for byte".to_owned()
        } else {
            format!("The other {n} copies are identical, byte for byte")
        };
        return Some(format!(
            "Keeper #1: {} · {}\n{rest}",
            dims(k.width, k.height),
            format_bytes(k.size)
        ));
    };
    Some(format!(
        "Keeper #1: {} · {}\nNext best #{}: {} · {}",
        dims(k.width, k.height),
        format_bytes(k.size),
        d.number_of(r.file).unwrap_or(2),
        dims(r.width, r.height),
        format_bytes(r.size)
    ))
}

/// Which panel the view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Groups,
    Suggestions,
}

/// An action waiting for confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Separate this copy from the group.
    Separate(FileId),
}

/// The compare loupe: two copies with one shared zoom and pan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Compare {
    pub a: FileId,
    pub b: FileId,
    /// 1.0 = fitted.
    pub zoom: f32,
    /// The point both images centre on, as a fraction of the image (0..1).
    pub center: (f32, f32),
}

const ZOOMS: [f32; 6] = [1.0, 1.5, 2.0, 3.0, 4.0, 8.0];

const SORTS: [(&str, ReviewSort); 4] = [
    ("Size of copies", ReviewSort::Reclaimable),
    ("Copies", ReviewSort::MemberCount),
    ("Date", ReviewSort::CaptureDate),
    ("Needs attention", ReviewSort::NeedsAttention),
];

pub struct DuplicatesView {
    source: Option<Arc<dyn ReviewSource>>,
    images: ReviewImages,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    pub tab: Tab,
    pub sort: ReviewSort,
    pub filter: ReviewFilter,
    list: Option<Rc<ReviewList>>,
    selected: Option<FileId>,
    detail: Option<ReviewDetail>,
    /// The copy keys act on (display index).
    focused: usize,
    pending: Option<Pending>,
    compare: Option<Compare>,
    drag_from: Option<Point<Pixels>>,
    suggestions: Option<Rc<Vec<SuggestionCard>>>,
    selected_suggestion: usize,
    explain: Option<(FileId, FileId, Vec<String>)>,
    /// Feedback after an action ("… is now its own photo").
    notice: Option<String>,
    error: Option<String>,
    busy: bool,
    /// Bumped per list request; stale answers are dropped.
    generation: u64,
}

impl DuplicatesView {
    pub fn new(
        source: Option<Arc<dyn ReviewSource>>,
        gallery: SharedSource,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let mut this = Self {
            source,
            images: ReviewImages::new(gallery),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            tab: Tab::Groups,
            sort: ReviewSort::Reclaimable,
            filter: ReviewFilter::default(),
            list: None,
            selected: None,
            detail: None,
            focused: 0,
            pending: None,
            compare: None,
            drag_from: None,
            suggestions: None,
            selected_suggestion: 0,
            explain: None,
            notice: None,
            error: None,
            busy: false,
            generation: 0,
        };
        if std::env::var("PHOTOSQUIRREL_REVIEW_TAB").as_deref() == Ok("suggestions") {
            // Dev-only: scripted screenshots of the suggestions panel (KDE gives agents one frame).
            this.tab = Tab::Suggestions;
        }
        let primed = crate::startup::StartupGrace::deadline(cx).is_some_and(|d| this.load_now(d));
        if !primed {
            this.reload(cx);
            this.reload_suggestions(cx);
        }
        if this.source.is_some() {
            let focus = this.focus.clone();
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        this
    }

    /// Load the list, the first group, the suggestions and their thumbnails before the first frame (start-up
    /// grace only, so the first frame of a scripted launch already shows the review). The queries run on a
    /// helper thread and are given up at `deadline`; thumbnails are primed only while time is left. `false`
    /// when nothing arrived in time (the caller then loads asynchronously).
    fn load_now(&mut self, deadline: std::time::Instant) -> bool {
        let Some(src) = self.source.clone() else {
            return true;
        };
        let (filter, sort) = (self.filter, self.sort);
        // Dev-only: scripted screenshots of a particular group (list index; the first by default).
        let pick = std::env::var("PHOTOSQUIRREL_REVIEW_GROUP")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let Some((list, detail, suggestions)) =
            crate::startup::StartupGrace::within(deadline, move || {
                let list = src.groups(filter, sort).ok();
                let first = list
                    .as_ref()
                    .and_then(|l| l.rows.get(pick).or(l.rows.first()).map(|r| r.id));
                let detail = first.and_then(|id| src.group(id).ok());
                (list, detail, src.suggestions(500).ok())
            })
        else {
            return false;
        };
        let in_time = || std::time::Instant::now() < deadline;
        if let Some(list) = list {
            for r in list.rows.iter().skip(pick.saturating_sub(12)).take(16) {
                if let Some(t) = r.thumb
                    && in_time()
                {
                    self.images.prime_thumb(t);
                }
            }
            self.selected = list.rows.get(pick).or(list.rows.first()).map(|r| r.id);
            if pick > 0 && pick < list.rows.len() {
                self.scroll.scroll_to_item(pick, ScrollStrategy::Center);
            }
            self.list = Some(Rc::new(list));
        }
        if let Some(d) = detail {
            for c in &d.cards {
                if let Some(t) = c.thumb
                    && in_time()
                {
                    self.images.prime_thumb(t);
                }
            }
            self.focused = usize::from(d.members().len() > 1);
            self.detail = Some(d);
        }
        if let Some(s) = suggestions {
            for c in s.iter().take(12) {
                for t in [c.a.card.thumb, c.b.card.thumb].into_iter().flatten() {
                    if in_time() {
                        self.images.prime_thumb(t);
                    }
                }
            }
            self.suggestions = Some(Rc::new(s));
        }
        true
    }

    // ---- state for tests --------------------------------------------------------------------------------

    pub fn list(&self) -> Option<&ReviewList> {
        self.list.as_deref()
    }

    pub fn detail(&self) -> Option<&ReviewDetail> {
        self.detail.as_ref()
    }

    pub fn selected(&self) -> Option<FileId> {
        self.selected
    }

    pub fn focused_member(&self) -> usize {
        self.focused
    }

    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    pub fn compare(&self) -> Option<&Compare> {
        self.compare.as_ref()
    }

    pub fn suggestions(&self) -> Option<&[SuggestionCard]> {
        self.suggestions.as_ref().map(|s| s.as_slice())
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    pub fn explanation(&self) -> Option<&[String]> {
        self.explain.as_ref().map(|e| e.2.as_slice())
    }

    // ---- loading ----------------------------------------------------------------------------------------

    /// Re-query the list (keeping the selection when it is still listed).
    pub fn reload(&mut self, cx: &mut Context<'_, Self>) {
        self.reload_then(None, cx);
    }

    fn reload_then(&mut self, select: Option<FileId>, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let (filter, sort) = (self.filter, self.sort);
        let task = cx
            .background_executor()
            .spawn(async move { src.groups(filter, sort) });
        cx.spawn(async move |this, cx| {
            let list = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match list {
                    Ok(list) => {
                        let want = select.or(this.selected);
                        let keep = want.filter(|id| list.rows.iter().any(|r| r.id == *id));
                        let next = keep.or_else(|| list.rows.first().map(|r| r.id));
                        this.list = Some(Rc::new(list));
                        this.error = None;
                        if next != this.selected || this.detail.is_none() || keep.is_some() {
                            match next {
                                Some(id) => this.load_group(id, cx),
                                None => {
                                    this.selected = None;
                                    this.detail = None;
                                }
                            }
                        }
                    }
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_group(&mut self, id: FileId, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        if self.selected != Some(id) {
            self.compare = None;
            self.pending = None;
            self.explain = None;
        }
        self.selected = Some(id);
        let task = cx.background_executor().spawn(async move { src.group(id) });
        cx.spawn(async move |this, cx| {
            let d = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.selected != Some(id) {
                    return;
                }
                match d {
                    Ok(d) => this.show_detail(d),
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_detail(&mut self, d: ReviewDetail) {
        let n = d.members().len();
        let same = self
            .detail
            .as_ref()
            .is_some_and(|o| o.group.id == d.group.id);
        if !same || self.focused >= n {
            // Focus the first copy that is not in the keeper set.
            self.focused = d
                .members()
                .iter()
                .position(|m| !m.in_keeper_set && !m.keeper)
                .unwrap_or(0);
        }
        self.selected = Some(d.group.id);
        self.detail = Some(d);
    }

    pub fn reload_suggestions(&mut self, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { src.suggestions(2_000) });
        cx.spawn(async move |this, cx| {
            let s = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(s) = s {
                    this.selected_suggestion =
                        this.selected_suggestion.min(s.len().saturating_sub(1));
                    this.suggestions = Some(Rc::new(s));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run an engine call off the UI thread, then refresh.
    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<'_, Self>,
        job: impl FnOnce(&dyn ReviewSource) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<'_, Self>) + 'static,
    ) {
        let Some(src) = self.source.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { job(src.as_ref()) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match r {
                    Ok(v) => done(this, v, cx),
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- navigation -------------------------------------------------------------------------------------

    fn rows(&self) -> &[ReviewRow] {
        self.list.as_ref().map_or(&[], |l| l.rows.as_slice())
    }

    fn index(&self) -> Option<usize> {
        let id = self.selected?;
        self.rows().iter().position(|r| r.id == id)
    }

    pub fn select_index(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        if let Some(id) = self.rows().get(ix).map(|r| r.id) {
            self.notice = None;
            self.load_group(id, cx);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn step(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        if self.tab == Tab::Suggestions {
            let n = self.suggestions.as_ref().map_or(0, |s| s.len());
            if n > 0 {
                self.selected_suggestion =
                    (self.selected_suggestion as isize + delta).clamp(0, n as isize - 1) as usize;
                self.explain = None;
                cx.notify();
            }
            return;
        }
        let n = self.rows().len();
        if n == 0 {
            return;
        }
        let ix = self.index().map_or(0, |i| {
            (i as isize + delta).clamp(0, n as isize - 1) as usize
        });
        self.select_index(ix, cx);
    }

    pub fn set_sort(&mut self, sort: ReviewSort, cx: &mut Context<'_, Self>) {
        self.sort = sort;
        self.reload(cx);
        cx.notify();
    }

    pub fn set_filter(&mut self, filter: ReviewFilter, cx: &mut Context<'_, Self>) {
        self.filter = filter;
        self.reload(cx);
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<'_, Self>) {
        self.tab = tab;
        self.compare = None;
        self.pending = None;
        self.explain = None;
        if tab == Tab::Suggestions {
            self.reload_suggestions(cx);
        }
        cx.notify();
    }

    // ---- actions ----------------------------------------------------------------------------------------

    /// Choose the copy at display number `n` (1-based) as keeper.
    pub fn pick_keeper(&mut self, n: usize, cx: &mut Context<'_, Self>) {
        let Some(d) = &self.detail else { return };
        let Some(m) = d.members().get(n.wrapping_sub(1)) else {
            return;
        };
        if m.keeper {
            return;
        }
        let (group, file) = (d.group.id, m.file);
        self.run(
            cx,
            move |s| s.set_keeper(group, file),
            |this, d, cx| {
                this.notice = Some("Keeper changed: chosen by you. Saved.".into());
                this.focused = 0;
                this.show_detail(d);
                this.reload(cx);
            },
        );
    }

    /// Ask to separate the focused copy (confirmation first).
    pub fn ask_separate(&mut self, cx: &mut Context<'_, Self>) {
        let Some(d) = &self.detail else { return };
        if let Some(m) = d.members().get(self.focused) {
            self.pending = Some(Pending::Separate(m.file));
            cx.notify();
        }
    }

    pub fn ask_separate_file(&mut self, file: FileId, cx: &mut Context<'_, Self>) {
        if let Some(ix) = self
            .detail
            .as_ref()
            .and_then(|d| d.members().iter().position(|m| m.file == file))
        {
            self.focused = ix;
        }
        self.pending = Some(Pending::Separate(file));
        cx.notify();
    }

    pub fn confirm(&mut self, cx: &mut Context<'_, Self>) {
        let Some(Pending::Separate(file)) = self.pending.take() else {
            return;
        };
        let Some(d) = &self.detail else { return };
        let group = d.group.id;
        let name = d.card(file).map(|c| c.name.clone()).unwrap_or_default();
        self.run(
            cx,
            move |s| s.separate(group, file),
            move |this, out, cx| {
                this.notice = Some(format!(
                    "“{name}” is now its own photo: it is kept and exported on its own and stays separate after rescans."
                ));
                this.compare = None;
                let next = out.rest_group.or(out.member_group);
                this.detail = None;
                this.reload_then(next, cx);
                this.reload_suggestions(cx);
                crate::views::library::refresh_after_decision(cx);
            },
        );
    }

    pub fn cancel(&mut self, cx: &mut Context<'_, Self>) {
        if self.pending.take().is_some()
            || self.compare.take().is_some()
            || self.explain.take().is_some()
        {
            cx.notify();
        }
    }

    pub fn merge_with(&mut self, other: FileId, cx: &mut Context<'_, Self>) {
        let Some(d) = &self.detail else { return };
        let group = d.group.id;
        self.run(
            cx,
            move |s| s.merge(group, other),
            |this, d, cx| {
                this.notice = Some(
                    "Groups merged by you. Nothing was copied or changed on disk; use Merge with… > Undo merge to \
                     split them again."
                        .into(),
                );
                let id = d.group.id;
                this.show_detail(d);
                this.reload_then(Some(id), cx);
            },
        );
    }

    /// Undo the user merges of the open group (back to automatic matching).
    pub fn undo_merge(&mut self, cx: &mut Context<'_, Self>) {
        let Some(d) = &self.detail else { return };
        let group = d.group.id;
        self.run(
            cx,
            move |s| s.undo_merges(group),
            |this, d, cx| {
                this.notice =
                    Some("Merge undone: the groups are as automatic matching made them.".into());
                let id = d.group.id;
                this.show_detail(d);
                this.reload_then(Some(id), cx);
            },
        );
    }

    /// Mark the open group reviewed and open the next unreviewed group.
    pub fn mark_reviewed(&mut self, cx: &mut Context<'_, Self>) {
        let Some(d) = &self.detail else { return };
        let group = d.group.id;
        let reviewed = !d.reviewed;
        let rows = self.rows().to_vec();
        let at = self.index().unwrap_or(0);
        let next = (1..=rows.len())
            .map(|k| &rows[(at + k) % rows.len()])
            .find(|r| !r.reviewed && r.id != group)
            .map(|r| r.id);
        self.run(
            cx,
            move |s| s.set_reviewed(group, reviewed),
            move |this, now, cx| {
                if now {
                    this.notice = None;
                    this.reload_then(next.or(Some(group)), cx);
                } else {
                    this.reload_then(Some(group), cx);
                }
            },
        );
    }

    pub fn toggle_compare(&mut self, cx: &mut Context<'_, Self>) {
        if self.compare.take().is_some() {
            cx.notify();
            return;
        }
        if self.tab == Tab::Suggestions {
            if let Some(s) = self
                .suggestions
                .as_ref()
                .and_then(|s| s.get(self.selected_suggestion))
            {
                self.compare = Some(Compare {
                    a: s.info.file_a,
                    b: s.info.file_b,
                    zoom: 1.0,
                    center: (0.5, 0.5),
                });
            }
            cx.notify();
            return;
        }
        let Some(d) = &self.detail else { return };
        let keeper = d.group.keeper;
        let other = d
            .members()
            .get(self.focused)
            .map(|m| m.file)
            .filter(|f| *f != keeper)
            .or_else(|| d.members().iter().find(|m| !m.keeper).map(|m| m.file));
        if let Some(b) = other {
            self.compare = Some(Compare {
                a: keeper,
                b,
                zoom: 1.0,
                center: (0.5, 0.5),
            });
        }
        cx.notify();
    }

    fn zoom_step(&mut self, dir: i32, cx: &mut Context<'_, Self>) {
        if let Some(c) = &mut self.compare {
            let ix = ZOOMS.iter().position(|z| *z >= c.zoom - 1e-3).unwrap_or(0) as i32;
            let next = (ix + dir).clamp(0, ZOOMS.len() as i32 - 1) as usize;
            c.zoom = if dir == 0 { 1.0 } else { ZOOMS[next] };
            if c.zoom <= 1.0 {
                c.center = (0.5, 0.5);
            }
            cx.notify();
        }
    }

    pub fn set_compare_zoom(&mut self, zoom: f32, center: (f32, f32), cx: &mut Context<'_, Self>) {
        if let Some(c) = &mut self.compare {
            c.zoom = zoom.clamp(1.0, 8.0);
            c.center = (center.0.clamp(0.0, 1.0), center.1.clamp(0.0, 1.0));
            cx.notify();
        }
    }

    pub fn focus_member(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        let n = self.detail.as_ref().map_or(0, |d| d.members().len());
        if ix < n {
            self.focused = ix;
            if let Some(c) = &mut self.compare
                && let Some(m) = self.detail.as_ref().and_then(|d| d.members().get(ix))
                && !m.keeper
            {
                c.b = m.file;
            }
            cx.notify();
        }
    }

    pub fn dismiss_selected(&mut self, cx: &mut Context<'_, Self>) {
        if self.tab != Tab::Suggestions {
            return;
        }
        let Some(s) = self
            .suggestions
            .as_ref()
            .and_then(|s| s.get(self.selected_suggestion))
        else {
            return;
        };
        let (a, b) = (s.info.file_a, s.info.file_b);
        self.run(
            cx,
            move |src| src.dismiss(a, b),
            |this, (), cx| {
                this.notice = Some("Suggestion dismissed. Both photos are unchanged.".into());
                this.explain = None;
                this.reload_suggestions(cx);
                this.reload(cx);
            },
        );
    }

    pub fn select_suggestion(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        self.selected_suggestion = ix;
        self.explain = None;
        cx.notify();
    }

    pub fn explain_suggestion(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        let Some(s) = self.suggestions.as_ref().and_then(|s| s.get(ix)) else {
            return;
        };
        let (a, b) = (s.info.file_a, s.info.file_b);
        self.selected_suggestion = ix;
        self.run(
            cx,
            move |src| src.explain(a, b),
            move |this, lines, _| {
                this.explain = Some((a, b, lines));
            },
        );
    }

    fn ensure_focus(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.source.is_none() {
            return;
        }
        // The shell keeps focus on itself when it switches views; take it so our keys work.
        if !self.focus.contains_focused(window, cx)
            && crate::shell::Shell::for_window(window, cx)
                .is_some_and(|s| s.read(cx).focus_handle().is_focused(window))
        {
            let focus = self.focus.clone();
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
    }
}

// ---- rendering -------------------------------------------------------------------------------------------

/// What the group holds, for the "Why this copy" card: (amount, description) rows.
pub fn group_composition(d: &ReviewDetail) -> Vec<(String, String)> {
    let count = |e: MemberEdge| d.members().iter().filter(|m| m.edge == e).count();
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_owned()
        } else {
            many.to_owned()
        }
    };
    let mut rows = Vec::new();
    let keepers = d.group.keeper_set.len().max(1);
    rows.push((
        keepers.to_string(),
        plural(
            keepers,
            "keeper: the copy export takes",
            "keepers (RAW+JPEG pair): export takes both together",
        ),
    ));
    for (edge, one, many) in [
        (
            MemberEdge::Exact,
            "exact copy, byte for byte",
            "exact copies, byte for byte",
        ),
        (
            MemberEdge::Derivative,
            "resized or recompressed version",
            "resized or recompressed versions",
        ),
        (
            MemberEdge::UserMerge,
            "copy merged by you",
            "copies merged by you",
        ),
    ] {
        let n = count(edge);
        if n > 0 {
            rows.push((n.to_string(), plural(n, one, many)));
        }
    }
    rows.push((
        format_bytes(d.group.reclaimable_bytes),
        "in the other copies: not copied again".to_owned(),
    ));
    rows
}

/// The member's join badge and its tooltip: "Keeper", "Exact copy of the keeper", "Resized copy of #2", ...,
/// short enough for a card at the default font size, with the full wording in the tooltip.
pub fn edge_label(d: &ReviewDetail, m: &GroupMember) -> (String, String) {
    let via = d.matched_label(m.file);
    let against = via.strip_prefix(" of ").unwrap_or_default();
    let matched = |what: &str| {
        if against.is_empty() {
            what.to_owned()
        } else {
            format!("{what}, matched against {against}")
        }
    };
    match m.edge {
        MemberEdge::Keeper => (
            "Keeper".to_owned(),
            "Keeper: the copy export takes".to_owned(),
        ),
        MemberEdge::Exact => (
            format!("Exact copy{via}"),
            matched("Exact copy, byte for byte"),
        ),
        MemberEdge::Derivative => (
            format!("Resized copy{via}"),
            matched("Same image, resized or recompressed"),
        ),
        MemberEdge::UserMerge => (format!("Merged by you{via}"), matched("Merged by you")),
    }
}

/// The badge of member `n` ([`edge_label`]): `duplicates-edge-{n}` and its label `duplicates-edge-{n}-label` for
/// tests.
fn edge_chip(d: &ReviewDetail, m: &GroupMember, n: usize, cx: &App) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let (label, tip) = edge_label(d, m);
    let (fg, bg) = match m.edge {
        MemberEdge::Keeper => (colors.on_accent, colors.accent),
        MemberEdge::Exact => (
            colors.tone_text(colors.success),
            ui_theme::Colors::chip_bg(colors.success),
        ),
        MemberEdge::Derivative => {
            let close = m.phash_distance.unwrap_or(0) >= CLOSE_PHASH
                || m.dhash_distance.unwrap_or(0) >= CLOSE_DHASH;
            let tone = if close { colors.warning } else { colors.info };
            (colors.tone_text(tone), ui_theme::Colors::chip_bg(tone))
        }
        MemberEdge::UserMerge => (colors.accent_text, colors.accent_soft),
    };
    let tip = SharedString::from(tip);
    chip_frame(fg, bg, cx)
        .id(SharedString::from(format!("duplicates-edge-{n}")))
        .test_support()
        .child(
            chip_label(label)
                .id(SharedString::from(format!("duplicates-edge-{n}-label")))
                .test_support(),
        )
        .when(m.edge == MemberEdge::Keeper, |c| {
            c.child(chip_icon(IconName::Crown, fg))
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .into_any_element()
}

impl DuplicatesView {
    fn render_list_pane(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let list = self.list.clone();
        let (total, reviewed) = list.as_ref().map_or((0, 0), |l| (l.groups, l.reviewed));
        let suggestions = list.as_ref().map_or(0, |l| l.suggestions);
        let fraction = if total == 0 {
            0.0
        } else {
            reviewed as f32 / total as f32
        };
        let tab = self.tab;
        let weak = cx.weak_entity();
        let tabs = h_flex()
            .gap(Tokens::space(1.0))
            .child(segmented(
                "duplicates-tab",
                &["Groups", "Similar shots"],
                usize::from(tab == Tab::Suggestions),
                {
                    let weak = weak.clone();
                    move |ix, _, cx| {
                        let _ = weak.update(cx, |v, cx| {
                            v.set_tab(
                                if ix == 1 {
                                    Tab::Suggestions
                                } else {
                                    Tab::Groups
                                },
                                cx,
                            )
                        });
                    }
                },
                cx,
            ))
            .child(div().flex_1())
            .child(kbd("B", cx));
        let header = v_flex()
            .flex_none()
            .px(Tokens::space(4.0))
            .pt(Tokens::space(4.0))
            .pb(Tokens::space(3.0))
            .gap(Tokens::space(2.5))
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .items_baseline()
                    .gap(Tokens::space(2.0))
                    .child(
                        div()
                            .text_size(Tokens::text_lg())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Duplicates"),
                    )
                    .child(
                        div()
                            .id("duplicates-progress")
                            .test_support()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child(match &list {
                                Some(l) => format!(
                                    "{} of {} reviewed",
                                    format_count(l.reviewed),
                                    format_count(l.groups)
                                ),
                                None => "Loading…".into(),
                            }),
                    ),
            )
            .child(progress_bar(fraction, cx))
            .child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(match &list {
                        Some(l) => format!(
                            "{} in extra copies, not copied again on export · {} similar shots to look at",
                            format_bytes(l.reclaimable_bytes),
                            format_count(suggestions)
                        ),
                        None => String::new(),
                    }),
            )
            .child(tabs);
        if tab == Tab::Suggestions {
            return v_flex()
                .w(Tokens::size(320.0))
                .flex_none()
                .h_full()
                .bg(colors.panel)
                .border_r_1()
                .border_color(colors.border)
                .child(header)
                .child(self.render_suggestion_list(cx))
                .into_any_element();
        }

        let sort_ix = SORTS.iter().position(|(_, s)| *s == self.sort).unwrap_or(0);
        let sort_menu = {
            let weak = weak.clone();
            Button::new("duplicates-sort")
                .label(format!("Sort: {}", SORTS[sort_ix].0))
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (ix, (label, sort)) in SORTS.iter().enumerate() {
                        let weak = weak.clone();
                        let sort = *sort;
                        menu =
                            menu.item(PopupMenuItem::new(*label).checked(ix == sort_ix).on_click(
                                move |_, _, cx| {
                                    let _ = weak.update(cx, |v, cx| v.set_sort(sort, cx));
                                },
                            ));
                    }
                    menu
                })
        };
        let f = self.filter;
        let filters = h_flex()
            .flex_wrap()
            .gap(Tokens::space(1.5))
            .child(filter_chip(
                "duplicates-filter-unreviewed",
                "Unreviewed",
                f.unreviewed,
                cx,
                |v, cx| {
                    let mut f = v.filter;
                    f.unreviewed = !f.unreviewed;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "duplicates-filter-exact",
                "Exact only",
                f.exact_only,
                cx,
                |v, cx| {
                    let mut f = v.filter;
                    f.exact_only = !f.exact_only;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "duplicates-filter-derivatives",
                "Resized copies",
                f.has_derivatives,
                cx,
                |v, cx| {
                    let mut f = v.filter;
                    f.has_derivatives = !f.has_derivatives;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "duplicates-filter-overridden",
                "Chosen by you",
                f.overridden,
                cx,
                |v, cx| {
                    let mut f = v.filter;
                    f.overridden = !f.overridden;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "duplicates-filter-suggestions",
                "Has similar shots",
                f.has_suggestions,
                cx,
                |v, cx| {
                    let mut f = v.filter;
                    f.has_suggestions = !f.has_suggestions;
                    v.set_filter(f, cx);
                },
            ));
        let controls = v_flex()
            .flex_none()
            .px(Tokens::space(4.0))
            .py(Tokens::space(2.5))
            .gap(Tokens::space(2.0))
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex().child(sort_menu).child(div().flex_1()).child(
                    div()
                        .text_size(Tokens::text_2xs())
                        .text_color(colors.text_faint)
                        .child(format!("{} shown", format_count(self.rows().len() as u64))),
                ),
            )
            .child(filters);

        let rows = list.clone();
        let n = rows.as_ref().map_or(0, |l| l.rows.len());
        let selected = self.index();
        let images = self.images.clone();
        let list_el = uniform_list(
            "duplicates-groups",
            n,
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                let colors = *ui_theme::colors(cx);
                let Some(rows) = rows.clone() else {
                    return Vec::new();
                };
                range
                    .map(|ix| {
                        let r = &rows.rows[ix];
                        let img = images.thumb(r.thumb, cx);
                        let on = selected == Some(ix);
                        h_flex()
                            .id(("duplicates-row", ix))
                            .w_full()
                            .h(Tokens::size(60.0))
                            .px(Tokens::space(3.0))
                            .gap(Tokens::space(3.0))
                            .items_center()
                            .border_b_1()
                            .border_color(colors.border)
                            .cursor_pointer()
                            .when(on, |d| d.bg(colors.accent_soft))
                            .when(!on, |d| d.hover(|d| d.bg(colors.hover)))
                            .on_click(cx.listener(move |this, _, _, cx| this.select_index(ix, cx)))
                            .child(
                                div()
                                    .size(Tokens::size(44.0))
                                    .flex_none()
                                    .rounded(Tokens::RADIUS_SM)
                                    .overflow_hidden()
                                    .when(on, |d| d.border_2().border_color(colors.accent))
                                    .child(photo(img, false, cx)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(Tokens::space(0.5))
                                    .child(
                                        div()
                                            .text_size(Tokens::text_sm())
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(colors.text)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(r.keeper_name.clone()),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(Tokens::space(1.5))
                                            .font_family(MONO_FONT_FAMILY)
                                            .text_size(Tokens::text_2xs())
                                            .text_color(colors.text_muted)
                                            .child(format!("{} copies", r.member_count))
                                            .child("·")
                                            .child(format_bytes(r.reclaimable_bytes))
                                            .when_some(r.capture_ms, |d, ms| {
                                                d.child("·").child(date_of(ms))
                                            }),
                                    )
                                    .when_some(r.attention.clone(), |d, why| {
                                        d.child(
                                            div()
                                                .text_size(Tokens::text_2xs())
                                                .text_color(colors.warning)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(why),
                                        )
                                    }),
                            )
                            .child(
                                v_flex()
                                    .flex_none()
                                    .items_end()
                                    .gap(Tokens::space(1.0))
                                    .when(r.reviewed, |d| {
                                        d.child(
                                            Icon::new(IconName::CircleCheck)
                                                .size(Tokens::icon_sm())
                                                .text_color(colors.success),
                                        )
                                    })
                                    .when(r.overridden, |d| {
                                        d.child(
                                            Icon::new(IconName::Crown)
                                                .size(Tokens::icon_sm())
                                                .text_color(colors.accent_text),
                                        )
                                    }),
                            )
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1();
        let empty = n == 0 && list.is_some();
        v_flex()
            .w(Tokens::size(320.0))
            .flex_none()
            .h_full()
            .bg(colors.panel)
            .border_r_1()
            .border_color(colors.border)
            .child(header)
            .child(controls)
            .when(empty, |d| {
                d.child(
                    div()
                        .id("duplicates-empty-filter")
                        .test_support()
                        .p(Tokens::space(6.0))
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child(if total == 0 {
                            "No duplicates found yet. Scan a source to find copies of the same photo."
                        } else {
                            "No group matches these filters."
                        }),
                )
            })
            .child(list_el)
            .into_any_element()
    }

    fn render_suggestion_list(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let items = self.suggestions.clone();
        let n = items.as_ref().map_or(0, |s| s.len());
        let selected = self.selected_suggestion;
        let images = self.images.clone();
        uniform_list(
            "duplicates-suggestions",
            n,
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                let Some(items) = items.clone() else {
                    return Vec::new();
                };
                range
                    .map(|ix| {
                        let s = &items[ix];
                        let ia = images.thumb(s.a.card.thumb, cx);
                        let ib = images.thumb(s.b.card.thumb, cx);
                        let on = ix == selected;
                        h_flex()
                            .id(("duplicates-suggestion-row", ix))
                            .w_full()
                            .h(Tokens::size(60.0))
                            .px(Tokens::space(3.0))
                            .gap(Tokens::space(3.0))
                            .items_center()
                            .border_b_1()
                            .border_color(colors.border)
                            .cursor_pointer()
                            .when(on, |d| d.bg(colors.accent_soft))
                            .when(!on, |d| d.hover(|d| d.bg(colors.hover)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.select_suggestion(ix, cx)),
                            )
                            .child(
                                h_flex()
                                    .flex_none()
                                    .gap(Tokens::space(0.5))
                                    .child(
                                        div()
                                            .size(Tokens::size(36.0))
                                            .rounded(Tokens::RADIUS_SM)
                                            .overflow_hidden()
                                            .child(photo(ia, false, cx)),
                                    )
                                    .child(
                                        div()
                                            .size(Tokens::size(36.0))
                                            .rounded(Tokens::RADIUS_SM)
                                            .overflow_hidden()
                                            .child(photo(ib, false, cx)),
                                    ),
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
                                            .child(format!(
                                                "{}  ·  {}",
                                                s.a.card.name, s.b.card.name
                                            )),
                                    )
                                    .child(
                                        div()
                                            .font_family(MONO_FONT_FAMILY)
                                            .text_size(Tokens::text_2xs())
                                            .text_color(colors.text_muted)
                                            .child(
                                                s.delta_label().unwrap_or_else(|| "undated".into()),
                                            ),
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .flex_1()
        .into_any_element()
    }

    fn render_member_card(
        &self,
        d: &ReviewDetail,
        ix: usize,
        m: &GroupMember,
        card: Option<&MemberCard>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let img = self.images.thumb(card.and_then(|c| c.thumb), cx);
        let focused = ix == self.focused;
        let n = ix + 1;
        let name = card.map(|c| c.name.clone()).unwrap_or_default();
        let folder = card
            .map(|c| {
                if c.folder.is_empty() {
                    c.source.clone()
                } else {
                    format!("{}/{}", c.source, c.folder)
                }
            })
            .unwrap_or_default();
        let quality = card.and_then(|c| c.quality).map_or_else(
            || crate::gallery_inspector::ABSENT.to_owned(),
            |q| format!("Q{q}"),
        );
        let exif = card.map(|c| {
            [
                ("Date", c.has_date),
                ("GPS", c.has_gps),
                ("Camera", c.has_camera),
                ("Maker notes", c.maker_notes),
            ]
        });
        let file = m.file;
        let distances = (m.edge == MemberEdge::Derivative).then(|| {
            format!(
                "pHash {} · dHash {}  (same photo at ≤ {} / ≤ {})",
                m.phash_distance.unwrap_or(0),
                m.dhash_distance.unwrap_or(0),
                ps_dedup_thresholds().0,
                ps_dedup_thresholds().1
            )
        });
        let mut actions = h_flex().gap(Tokens::space(1.5)).flex_wrap();
        if !m.keeper {
            actions = actions
                .child(action_button(
                    format!("duplicates-keep-{n}"),
                    "Make keeper",
                    Some(IconName::Crown),
                    (n <= 9).then(|| ["1", "2", "3", "4", "5", "6", "7", "8", "9"][n - 1]),
                    Tone::Normal,
                    cx,
                    move |this, _, cx| this.pick_keeper(n, cx),
                ))
                .child(action_button(
                    format!("duplicates-separate-{n}"),
                    "Not the same photo",
                    Some(IconName::Scissors),
                    focused.then_some("S"),
                    Tone::Caution,
                    cx,
                    move |this, _, cx| this.ask_separate_file(file, cx),
                ));
        }
        v_flex()
            .id(SharedString::from(format!("duplicates-member-{n}")))
            .test_support()
            .w(Tokens::size(232.0))
            .flex_none()
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.raised)
            .border_1()
            // Only the keyboard-focused card is outlined in the accent; the keeper is marked by its badge, so the
            // two never look alike.
            .border_color(if focused {
                colors.accent
            } else {
                colors.border
            })
            .when(focused, |d| d.shadow_md())
            .overflow_hidden()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.focus_member(ix, cx)))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(Tokens::size(150.0))
                    .overflow_hidden()
                    .bg(colors.gallery)
                    .child(photo(img, true, cx))
                    .child(
                        div()
                            .absolute()
                            .top(Tokens::space(2.0))
                            .left(Tokens::space(2.0))
                            .child(kbd(n.to_string(), cx)),
                    ),
            )
            .child(
                v_flex()
                    .p(Tokens::space(3.0))
                    .gap(Tokens::space(2.0))
                    .child(edge_chip(d, m, n, cx))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(Tokens::text_sm())
                                    .font_weight(FontWeight::MEDIUM)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .font_family(MONO_FONT_FAMILY)
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_faint)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(folder),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(3.0))
                            .child(stat("Pixels", dims(m.width, m.height), cx).flex_1())
                            .child(stat("Size", format_bytes(m.size), cx).flex_1()),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(3.0))
                            .child(
                                stat(
                                    "Format",
                                    m.format.map_or("?".to_owned(), |f| f.label().to_owned()),
                                    cx,
                                )
                                .flex_1(),
                            )
                            .child(stat("Quality estimate", quality, cx).flex_1()),
                    )
                    .child(stat(
                        "Captured",
                        card.and_then(|c| c.captured.clone())
                            .unwrap_or_else(|| "no capture date".into()),
                        cx,
                    ))
                    .when_some(exif, |d, exif| {
                        d.child(
                            h_flex()
                                .id(SharedString::from(format!("duplicates-exif-{n}")))
                                .gap(Tokens::space(1.5))
                                .flex_wrap()
                                .children(exif.into_iter().map(|(label, on)| {
                                    h_flex()
                                        .gap(Tokens::space(1.0))
                                        .text_size(Tokens::text_2xs())
                                        .text_color(if on {
                                            colors.text_muted
                                        } else {
                                            colors.text_faint
                                        })
                                        .child(div().size(Tokens::dot_size()).rounded_full().bg(
                                            if on {
                                                colors.success
                                            } else {
                                                colors.border_strong
                                            },
                                        ))
                                        .child(label)
                                })),
                        )
                    })
                    .when_some(distances, |d, t| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("duplicates-distances-{n}")))
                                .test_support()
                                .font_family(MONO_FONT_FAMILY)
                                .text_size(Tokens::text_2xs())
                                .text_color(colors.text_muted)
                                .child(t),
                        )
                    })
                    .child(actions),
            )
            .into_any_element()
    }

    fn render_why(&self, d: &ReviewDetail, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let compare_line = why_compare_line(d);
        v_flex()
            .id("duplicates-why")
            .test_support()
            .w(Tokens::size(300.0))
            .flex_none()
            .gap(Tokens::space(2.0))
            .p(Tokens::space(4.0))
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.raised)
            .border_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(Icon::new(IconName::Star).size(Tokens::icon_md()).text_color(colors.accent_text))
                    .child(
                        div()
                            .text_size(Tokens::text_md())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Why this copy"),
                    )
                    .when(d.group.chosen_by_you, |e| {
                        e.child(icon_chip("duplicates-chosen", IconName::Crown, "Chosen by you", colors.accent_text, colors.accent_soft, cx))
                    }),
            )
            .children(d.group.keeper_reasons.iter().enumerate().map(|(i, r)| {
                h_flex()
                    .id(SharedString::from(format!("duplicates-why-{i}")))
                    .test_support()
                    .items_start()
                    .gap(Tokens::space(2.0))
                    .child(Icon::new(IconName::Check).size(Tokens::icon_sm()).text_color(colors.success))
                    .child(div().flex_1().min_w_0().text_size(Tokens::text_sm()).child(r.clone()))
            }))
            .when_some(compare_line, |e, t| {
                e.child(
                    div()
                        .id("duplicates-why-compare")
                        .test_support()
                        .pt(Tokens::space(1.0))
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(Tokens::text_2xs())
                        .text_color(colors.text_muted)
                        .child(t),
                )
            })
            .child(
                v_flex()
                    .id("duplicates-why-group")
                    .test_support()
                    .pt(Tokens::space(2.0))
                    .gap(Tokens::space(1.0))
                    .border_t_1()
                    .border_color(colors.border)
                    .child(eyebrow("In this group", cx))
                    .children(group_composition(d).into_iter().map(|(n, what)| {
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .text_size(Tokens::text_sm())
                            .child(
                                div()
                                    .w(Tokens::size(56.0))
                                    .flex_none()
                                    .text_right()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(n),
                            )
                            .child(div().text_color(colors.text_muted).child(what))
                    }))
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .items_start()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(Icon::new(IconName::ShieldCheck).size(Tokens::icon_sm()))
                    .child(div().flex_1().min_w_0().child(
                        "Nothing is deleted. Every copy stays where it is; export copies the keeper only.",
                    )),
            )
            .into_any_element()
    }

    fn render_hero(&self, d: &ReviewDetail, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let set: Vec<FileId> = if d.is_pair() {
            d.group.keeper_set.clone()
        } else {
            vec![d.group.keeper]
        };
        let tiles: Vec<AnyElement> = set
            .iter()
            .map(|f| {
                let card = d.card(*f);
                let img = self
                    .images
                    .preview(*f, cx)
                    .or_else(|| self.images.thumb(card.and_then(|c| c.thumb), cx));
                let m = d.group.member(*f);
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(Tokens::space(1.5))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .rounded(Tokens::RADIUS_MD)
                            .overflow_hidden()
                            .bg(colors.gallery)
                            .child(photo(img, true, cx)),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .font_family(MONO_FONT_FAMILY)
                            .text_size(Tokens::text_2xs())
                            .text_color(colors.text_muted)
                            .child(card.map(|c| c.name.clone()).unwrap_or_default())
                            .child(m.map(|m| dims(m.width, m.height)).unwrap_or_default())
                            .child(
                                m.and_then(|m| m.format)
                                    .map(|f| f.label().to_owned())
                                    .unwrap_or_default(),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .id(if d.is_pair() {
                "duplicates-keeper-pair"
            } else {
                "duplicates-keeper"
            })
            .test_support()
            .flex_1()
            .min_w(Tokens::size(if d.is_pair() { 360.0 } else { 240.0 }))
            .max_w(Tokens::size(if d.is_pair() { 640.0 } else { 480.0 }))
            .h(Tokens::size(300.0))
            .p(Tokens::space(3.0))
            .gap(Tokens::space(2.0))
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.raised)
            .border_1()
            .border_color(ui_theme::Colors::tint(colors.accent, 0.6))
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(icon_chip(
                        "duplicates-pair-keeper",
                        IconName::Crown,
                        "Keeper",
                        colors.on_accent,
                        colors.accent,
                        cx,
                    ))
                    .when(d.is_pair(), |e| {
                        e.child(chip(
                            "duplicates-pair-note",
                            "RAW + JPEG pair · kept and exported together",
                            colors.accent_text,
                            colors.accent_soft,
                            cx,
                        ))
                    }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap(Tokens::space(2.0))
                    .children(tiles),
            )
            .into_any_element()
    }

    fn render_detail(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let Some(d) = self.detail.clone() else {
            return v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(Tokens::space(2.0))
                .bg(colors.gallery)
                .child(
                    Icon::new(IconName::Copy)
                        .size(Tokens::icon_lg())
                        .text_color(colors.text_faint),
                )
                .child(
                    div()
                        .id("duplicates-detail-empty")
                        .test_support()
                        .max_w(Tokens::prose_width())
                        .text_center()
                        .text_color(if self.error.is_some() {
                            colors.danger
                        } else {
                            colors.text_muted
                        })
                        .child(match &self.error {
                            Some(e) => e.clone(),
                            None if self.list.as_ref().is_some_and(|l| l.groups == 0) => {
                                "No duplicate groups".to_owned()
                            }
                            None => "Loading…".to_owned(),
                        }),
                )
                .into_any_element();
        };
        if let Some(c) = self.compare {
            return self.render_compare(c, window, cx);
        }
        let keeper_card = d.card(d.group.keeper);
        let title = keeper_card.map(|c| c.name.clone()).unwrap_or_default();
        let captured = keeper_card.and_then(|c| c.captured.clone());
        let others: Vec<(FileId, String, i64)> = self
            .rows()
            .iter()
            .filter(|r| r.id != d.group.id)
            .map(|r| {
                let dist = match (r.capture_ms, keeper_card.and_then(|c| c.capture_ms)) {
                    (Some(a), Some(b)) => (a - b).abs(),
                    _ => i64::MAX,
                };
                (r.id, r.keeper_name.clone(), dist)
            })
            .collect();
        let mut nearest = others;
        nearest.sort_by_key(|o| o.2);
        nearest.truncate(8);
        let merged_by_you = d.members().iter().any(|m| m.edge == MemberEdge::UserMerge);
        let weak = cx.weak_entity();
        let merge = Button::new("duplicates-merge")
            .label("Merge with…")
            .icon(Icon::new(IconName::Merge))
            .compact()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.label("Same photo as another group? (nearest in time)");
                for (id, name, _) in &nearest {
                    let weak = weak.clone();
                    let id = *id;
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |v, cx| v.merge_with(id, cx));
                    }));
                }
                if merged_by_you {
                    let weak = weak.clone();
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("Undo merge").on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |v, cx| v.undo_merge(cx));
                        }));
                }
                menu
            });
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
                    .min_w(Tokens::size(240.0))
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .child(
                                div()
                                    .id("duplicates-title")
                                    .test_support()
                                    .text_size(Tokens::text_xl())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(title),
                            )
                            .when(d.reviewed, |e| {
                                e.child(icon_chip(
                                    "duplicates-reviewed-chip",
                                    IconName::CircleCheck,
                                    "Reviewed",
                                    colors.tone_text(colors.success),
                                    ui_theme::Colors::chip_bg(colors.success),
                                    cx,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child(format!(
                                "{} copies · {} not copied again{}",
                                d.members().len(),
                                format_bytes(d.group.reclaimable_bytes),
                                captured
                                    .map(|c| format!(" · captured {c}"))
                                    .unwrap_or_default()
                            )),
                    ),
            )
            .child(merge)
            .child(action_button(
                "duplicates-compare",
                "Compare",
                Some(IconName::Columns2),
                Some("Space"),
                Tone::Normal,
                cx,
                |this, _, cx| this.toggle_compare(cx),
            ))
            .child(action_button(
                "duplicates-reviewed",
                if d.reviewed {
                    "Reviewed"
                } else {
                    "Mark reviewed"
                },
                Some(IconName::Check),
                Some("R"),
                if d.reviewed {
                    Tone::Normal
                } else {
                    Tone::Primary
                },
                cx,
                |this, _, cx| this.mark_reviewed(cx),
            ));

        // Copies: the keeper set first (shown in the hero), then every member as a card.
        let cards: Vec<AnyElement> = d
            .members()
            .iter()
            .enumerate()
            .map(|(ix, m)| self.render_member_card(&d, ix, m, d.card(m.file), cx))
            .collect();
        let related = d.group.suggestions.len();
        let body = v_flex()
            .id("duplicates-detail-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(Tokens::space(5.0))
            .py(Tokens::space(4.0))
            .gap(Tokens::space(4.0))
            .child(
                h_flex()
                    .items_stretch()
                    .gap(Tokens::space(4.0))
                    .child(self.render_hero(&d, cx))
                    .child(self.render_why(&d, cx)),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(Tokens::space(2.0))
                    .items_baseline()
                    .child(eyebrow(format!("Why these are the same photo · {} copies", d.members().len()), cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(Tokens::text_2xs())
                            .text_color(colors.text_faint)
                            .child("1–9 keeper · ←/→ focus · S not the same photo · Space compare · J/K next/previous"),
                    ),
            )
            .child(
                div()
                    .id("duplicates-members")
                    .test_support()
                    .w_full()
                    // Wrap instead of a horizontal strip, so no card is cut off mid-text.
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_start()
                            .gap(Tokens::space(3.0))
                            .pb(Tokens::space(2.0))
                            .children(cards),
                    ),
            )
            .when(related > 0, |e| {
                e.child(
                    h_flex()
                        .id("duplicates-related-suggestions")
                        .test_support()
                        .gap(Tokens::space(2.0))
                        .p(Tokens::space(3.0))
                        .rounded(Tokens::RADIUS_MD)
                        .border_1()
                        .border_color(colors.border)
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child(Icon::new(IconName::Layers).size(Tokens::icon_sm()))
                        .child(format!(
                            "{related} similar shot{} nearby (bursts are never merged automatically)",
                            if related == 1 { "" } else { "s" }
                        ))
                        .child(div().flex_1())
                        .child(action_button(
                            "duplicates-open-suggestions",
                            "Show similar shots",
                            None,
                            Some("B"),
                            Tone::Quiet,
                            cx,
                            |this, _, cx| this.set_tab(Tab::Suggestions, cx),
                        )),
                )
            });
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(colors.background)
            .child(header)
            .children(self.render_banner(cx))
            .child(body)
            .children(self.render_confirm(&d, cx))
            .into_any_element()
    }

    fn render_banner(&self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let colors = *ui_theme::colors(cx);
        let (text, tone, id) = match (&self.error, &self.notice) {
            (Some(e), _) => (e.clone(), colors.danger, "duplicates-error"),
            (None, Some(n)) => (n.clone(), colors.success, "duplicates-notice"),
            _ => return None,
        };
        Some(
            h_flex()
                .id(id)
                .test_support()
                .mx(Tokens::space(5.0))
                .mt(Tokens::space(3.0))
                .px(Tokens::space(3.0))
                .py(Tokens::space(2.0))
                .gap(Tokens::space(2.0))
                .rounded(Tokens::RADIUS_MD)
                .bg(ui_theme::Colors::tint(tone, 0.12))
                .text_size(Tokens::text_sm())
                .text_color(colors.text)
                .child(
                    Icon::new(IconName::Info)
                        .size(Tokens::icon_sm())
                        .text_color(tone),
                )
                .child(div().flex_1().child(text))
                .into_any_element(),
        )
    }

    fn render_confirm(&self, d: &ReviewDetail, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let colors = *ui_theme::colors(cx);
        let Some(Pending::Separate(file)) = &self.pending else {
            return None;
        };
        let name = d.card(*file).map(|c| c.name.clone()).unwrap_or_default();
        Some(
            v_flex()
                .id("duplicates-confirm-separate")
                .test_support()
                .flex_none()
                .m(Tokens::space(4.0))
                .p(Tokens::space(4.0))
                .gap(Tokens::space(2.0))
                .rounded(Tokens::RADIUS_LG)
                .bg(colors.overlay)
                .border_1()
                .border_color(colors.danger)
                .shadow_lg()
                .child(
                    h_flex()
                        .gap(Tokens::space(2.0))
                        .child(Icon::new(IconName::Scissors).size(Tokens::icon_md()).text_color(colors.danger))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("“{name}” is not the same photo?")),
                        ),
                )
                .child(div().text_size(Tokens::text_sm()).text_color(colors.text_muted).child(
                    "This is the safety valve for a wrong merge. The copy leaves this group at once, becomes its own \
                     photo in the Library and is kept and exported on its own. It stays separate after every rescan. \
                     No file is moved or deleted.",
                ))
                .child(
                    h_flex()
                        .gap(Tokens::space(2.0))
                        .child(action_button(
                            "duplicates-confirm-separate-ok",
                            "Separate this copy",
                            Some(IconName::Scissors),
                            Some("Enter"),
                            Tone::Primary,
                            cx,
                            |this, _, cx| this.confirm(cx),
                        ))
                        .child(action_button(
                            "duplicates-confirm-separate-cancel",
                            "Keep in group",
                            None,
                            Some("Esc"),
                            Tone::Quiet,
                            cx,
                            |this, _, cx| this.cancel(cx),
                        )),
                )
                .into_any_element(),
        )
    }

    fn compare_side(
        &self,
        f: FileId,
        cx: &mut Context<'_, Self>,
    ) -> (Option<Arc<RenderImage>>, String) {
        let thumb_key = self
            .detail
            .as_ref()
            .and_then(|d| d.card(f))
            .map(|c| (c.thumb, c.name.clone()))
            .or_else(|| {
                self.suggestions.as_ref().and_then(|s| {
                    s.iter().find_map(|x| {
                        if x.a.card.file == f {
                            Some((x.a.card.thumb, x.a.card.name.clone()))
                        } else if x.b.card.file == f {
                            Some((x.b.card.thumb, x.b.card.name.clone()))
                        } else {
                            None
                        }
                    })
                })
            })
            .unwrap_or((None, String::new()));
        let img = self
            .images
            .preview(f, cx)
            .or_else(|| self.images.thumb(thumb_key.0, cx));
        (img, thumb_key.1)
    }

    fn render_compare(
        &mut self,
        c: Compare,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let (ia, na) = self.compare_side(c.a, cx);
        let (ib, nb) = self.compare_side(c.b, cx);
        let pane = |img: Option<Arc<RenderImage>>,
                    label: String,
                    id: &'static str,
                    cx: &mut Context<'_, Self>| {
            let zoom = c.zoom;
            let center = c.center;
            let bg = colors.loupe;
            v_flex()
                .id(id)
                .test_support()
                .flex_1()
                .min_w_0()
                .h_full()
                .gap(Tokens::space(1.5))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-canvas")))
                        .flex_1()
                        .min_h_0()
                        .rounded(Tokens::RADIUS_MD)
                        .overflow_hidden()
                        .bg(bg)
                        .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _, cx| {
                            let dy = f32::from(e.delta.pixel_delta(lpx(16.0)).y);
                            let next = if dy > 0.0 { zoom * 1.15 } else { zoom / 1.15 };
                            this.set_compare_zoom(next, center, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e: &MouseDownEvent, _, _| {
                                this.drag_from = Some(e.position);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, _| {
                                this.drag_from = None;
                            }),
                        )
                        .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                            if let (Some(from), Some(MouseButton::Left)) =
                                (this.drag_from, e.pressed_button)
                            {
                                let d = e.position - from;
                                this.drag_from = Some(e.position);
                                let k = 1.0 / (zoom * 600.0);
                                let (x, y) = center;
                                this.set_compare_zoom(
                                    zoom,
                                    (x - f32::from(d.x) * k, y - f32::from(d.y) * k),
                                    cx,
                                );
                            }
                        }))
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| {
                                    let Some(img) = img.clone() else { return };
                                    let s = img.size(0);
                                    let (w, h) = (s.width.0 as f32, s.height.0 as f32);
                                    let (bw, bh) = (
                                        f32::from(bounds.size.width),
                                        f32::from(bounds.size.height),
                                    );
                                    let k = (bw / w.max(1.0)).min(bh / h.max(1.0)) * zoom;
                                    let (dw, dh) = (w * k, h * k);
                                    let cxp = center.0 * dw;
                                    let cyp = center.1 * dh;
                                    let x = if dw <= bw {
                                        (bw - dw) / 2.0
                                    } else {
                                        (bw / 2.0 - cxp).clamp(bw - dw, 0.0)
                                    };
                                    let y = if dh <= bh {
                                        (bh - dh) / 2.0
                                    } else {
                                        (bh / 2.0 - cyp).clamp(bh - dh, 0.0)
                                    };
                                    let r = Bounds::new(
                                        point(bounds.left() + lpx(x), bounds.top() + lpx(y)),
                                        size(lpx(dw), lpx(dh)),
                                    );
                                    window.with_content_mask(
                                        Some(ContentMask { bounds }),
                                        |window| {
                                            let _ = window.paint_image(
                                                r,
                                                r,
                                                Corners::default(),
                                                img,
                                                0,
                                                false,
                                            );
                                        },
                                    );
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_muted)
                        .child(label),
                )
        };
        let left = pane(ia, na, "duplicates-compare-a", cx);
        let right = pane(ib, nb, "duplicates-compare-b", cx);
        v_flex()
            .id("duplicates-compare-loupe")
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(colors.background)
            .child(
                h_flex()
                    .flex_none()
                    .px(Tokens::space(5.0))
                    .py(Tokens::space(3.0))
                    .gap(Tokens::space(3.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .text_size(Tokens::text_lg())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Compare"),
                    )
                    .child(
                        div()
                            .id("duplicates-compare-zoom")
                            .test_support()
                            .font_family(MONO_FONT_FAMILY)
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child(if c.zoom <= 1.0 {
                                "Fit · zoom and pan stay in sync".to_owned()
                            } else {
                                format!("{:.0}% of fit · zoom and pan stay in sync", c.zoom * 100.0)
                            }),
                    )
                    .child(div().flex_1())
                    .child(action_button(
                        "duplicates-compare-out",
                        "−",
                        None,
                        Some("-"),
                        Tone::Quiet,
                        cx,
                        |this, _, cx| this.zoom_step(-1, cx),
                    ))
                    .child(action_button(
                        "duplicates-compare-fit",
                        "Fit",
                        None,
                        Some("0"),
                        Tone::Quiet,
                        cx,
                        |this, _, cx| this.zoom_step(0, cx),
                    ))
                    .child(action_button(
                        "duplicates-compare-in",
                        "+",
                        Some(IconName::ZoomIn),
                        Some("="),
                        Tone::Quiet,
                        cx,
                        |this, _, cx| this.zoom_step(1, cx),
                    ))
                    .child(action_button(
                        "duplicates-compare-close",
                        "Close",
                        None,
                        Some("Esc"),
                        Tone::Normal,
                        cx,
                        |this, _, cx| this.cancel(cx),
                    )),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .p(Tokens::space(4.0))
                    .gap(Tokens::space(3.0))
                    .child(left)
                    .child(right),
            )
            .into_any_element()
    }

    fn render_suggestion_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        if let Some(c) = self.compare {
            return self.render_compare(c, window, cx);
        }
        let Some(s) = self
            .suggestions
            .as_ref()
            .and_then(|s| s.get(self.selected_suggestion))
            .cloned()
        else {
            return v_flex()
                .id("duplicates-no-suggestions")
                .test_support()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(Tokens::space(2.0))
                .bg(colors.gallery)
                .child(
                    Icon::new(IconName::Layers)
                        .size(Tokens::icon_lg())
                        .text_color(colors.text_faint),
                )
                .child(
                    div()
                        .text_color(colors.text_muted)
                        .child(if self.suggestions.is_some() {
                            "No similar shots to look at"
                        } else {
                            "Loading…"
                        }),
                )
                .into_any_element();
        };
        let ix = self.selected_suggestion;
        let side = |side: &ps_service::review::SuggestionSide,
                    id: &'static str,
                    this: &Self,
                    cx: &mut Context<'_, Self>| {
            let img = this
                .images
                .preview(side.card.file, cx)
                .or_else(|| this.images.thumb(side.card.thumb, cx));
            v_flex()
                .id(id)
                .test_support()
                .flex_1()
                .min_w_0()
                .gap(Tokens::space(2.0))
                .child(
                    div()
                        .w_full()
                        .h(Tokens::size(340.0))
                        .rounded(Tokens::RADIUS_LG)
                        .overflow_hidden()
                        .bg(colors.gallery)
                        .child(photo(img, true, cx)),
                )
                .child(
                    div()
                        .text_size(Tokens::text_sm())
                        .font_weight(FontWeight::MEDIUM)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(side.card.name.clone()),
                )
                .child(
                    h_flex()
                        .gap(Tokens::space(3.0))
                        .child(stat("Pixels", dims(side.width, side.height), cx))
                        .child(stat("Size", format_bytes(side.size), cx))
                        .child(stat(
                            "Captured",
                            side.card
                                .captured
                                .clone()
                                .unwrap_or_else(|| "undated".into()),
                            cx,
                        )),
                )
                .child(
                    div()
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(Tokens::text_2xs())
                        .text_color(colors.text_faint)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(format!("{}/{}", side.card.source, side.card.folder)),
                )
        };
        let a = side(&s.a, "duplicates-suggestion-a", self, cx);
        let b = side(&s.b, "duplicates-suggestion-b", self, cx);
        let delta = s.delta_label();
        let explain = self
            .explain
            .as_ref()
            .filter(|e| e.0 == s.info.file_a && e.1 == s.info.file_b)
            .map(|e| e.2.clone());
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(colors.background)
            .child(
                h_flex()
                    .flex_none()
                    .px(Tokens::space(5.0))
                    .py(Tokens::space(3.0))
                    .gap(Tokens::space(3.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(Tokens::text_xl())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Similar shots"),
                            )
                            .child(
                                div()
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_muted)
                                    .child("Look alike, but kept as two photos. Nothing here merges them."),
                            ),
                    )
                    .child(action_button("duplicates-explain", "Why not merged?", Some(IconName::Info), None, Tone::Normal, cx, move |this, _, cx| this.explain_suggestion(ix, cx)))
                    .child(action_button("duplicates-compare-suggestion", "Compare", Some(IconName::Columns2), Some("Space"), Tone::Normal, cx, |this, _, cx| this.toggle_compare(cx)))
                    .child(action_button("duplicates-dismiss", "Dismiss", Some(IconName::X), Some("X"), Tone::Primary, cx, |this, _, cx| this.dismiss_selected(cx))),
            )
            .children(self.render_banner(cx))
            .child(
                v_flex()
                    .id("duplicates-suggestion-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(Tokens::space(5.0))
                    .py(Tokens::space(4.0))
                    .gap(Tokens::space(4.0))
                    .child(
                        h_flex()
                            .id("duplicates-suggestion-reason")
                            .test_support()
                            .gap(Tokens::space(3.0))
                            .p(Tokens::space(3.0))
                            .rounded(Tokens::RADIUS_MD)
                            .bg(colors.raised)
                            .border_1()
                            .border_color(colors.border)
                            .child(Icon::new(IconName::Clock).size(Tokens::icon_md()).text_color(colors.accent_text))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .id("duplicates-suggestion-delta")
                                            .test_support()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(match &delta {
                                                Some(d) => format!("Taken {d}"),
                                                None => "Capture time unknown".into(),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(Tokens::text_sm())
                                            .text_color(colors.text_muted)
                                            .child(s.info.reason.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .font_family(MONO_FONT_FAMILY)
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_faint)
                                    .child(format!(
                                        "pHash {} · dHash {}",
                                        s.info.phash_distance, s.info.dhash_distance
                                    )),
                            ),
                    )
                    .child(h_flex().items_start().gap(Tokens::space(4.0)).child(a).child(b))
                    .when_some(explain, |e, lines| {
                        e.child(
                            v_flex()
                                .id("duplicates-explanation")
                                .test_support()
                                .gap(Tokens::space(1.0))
                                .p(Tokens::space(3.0))
                                .rounded(Tokens::RADIUS_MD)
                                .bg(colors.raised)
                                .child(eyebrow("Every grouping rule, for this pair", cx))
                                .children(lines.into_iter().map(|l| {
                                    div()
                                        .font_family(MONO_FONT_FAMILY)
                                        .text_size(Tokens::text_2xs())
                                        .text_color(colors.text_muted)
                                        .child(l)
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_faint)
                            .child(Icon::new(IconName::ShieldCheck).size(Tokens::icon_sm()))
                            .child("Both shots stay in your Library and are exported. Dismissing only hides the suggestion."),
                    ),
            )
            .into_any_element()
    }
}

/// Grouping thresholds shown on derivative cards (ADR-0013).
fn ps_dedup_thresholds() -> (u8, u8) {
    (CLOSE_PHASH + 1, CLOSE_DHASH + 1)
}

impl Focusable for DuplicatesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DuplicatesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.images.release(window);
        self.ensure_focus(window, cx);
        let colors = *ui_theme::colors(cx);
        if self.source.is_none() {
            return v_flex()
                .id("view-duplicates")
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .bg(colors.gallery)
                .text_color(colors.text_muted)
                .child("Duplicate review needs a library. Add a source to begin.")
                .into_any_element();
        }
        let list = self.render_list_pane(cx);
        let detail = match self.tab {
            Tab::Groups => self.render_detail(window, cx),
            Tab::Suggestions => self.render_suggestion_detail(window, cx),
        };
        let mut context = KeyContext::new_with_defaults();
        context.add(CONTEXT);
        h_flex()
            .id("view-duplicates")
            .test_support()
            .key_context(context)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .on_action(cx.listener(|this, _: &NextGroup, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PrevGroup, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &MarkReviewed, _, cx| this.mark_reviewed(cx)))
            .on_action(cx.listener(|this, _: &SeparateFocused, _, cx| this.ask_separate(cx)))
            .on_action(cx.listener(|this, _: &ToggleCompare, _, cx| this.toggle_compare(cx)))
            .on_action(cx.listener(|this, _: &DismissSuggestion, _, cx| this.dismiss_selected(cx)))
            .on_action(cx.listener(|this, _: &FocusNextMember, _, cx| {
                let n = this.focused + 1;
                this.focus_member(n, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPrevMember, _, cx| {
                if let Some(n) = this.focused.checked_sub(1) {
                    this.focus_member(n, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ConfirmPending, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|this, _: &CancelPending, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &ToggleSuggestions, _, cx| {
                let t = if this.tab == Tab::Groups {
                    Tab::Suggestions
                } else {
                    Tab::Groups
                };
                this.set_tab(t, cx)
            }))
            .on_action(cx.listener(|this, _: &CompareZoomIn, _, cx| this.zoom_step(1, cx)))
            .on_action(cx.listener(|this, _: &CompareZoomOut, _, cx| this.zoom_step(-1, cx)))
            .on_action(cx.listener(|this, _: &CompareZoomFit, _, cx| this.zoom_step(0, cx)))
            .on_action(cx.listener(|this, a: &PickKeeper, _, cx| this.pick_keeper(a.0, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .child(list)
            .child(detail)
            .into_any_element()
    }
}
