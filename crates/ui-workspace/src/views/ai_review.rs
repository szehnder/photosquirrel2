//! The AI Review view (change `ai-insights`, spec `ai-review`): analysed photos newest first, updating live
//! while the queue runs (a 500 ms poll of a cheap stamp, ADR-0019); subject, category chips, people (adults
//! and children), pets and confidence per photo; filters (category, children, pets, people, needs a
//! decision); accept or reject the AI's exclusion suggestions one by one or in bulk; corrections of category,
//! people counts, pets and tags that win over the AI value (stored separately); queue status with start,
//! pause and resume; the provider and a connection test.
//!
//! AI output is advisory (ADR-0021): the view says so, and nothing here changes groups, keepers or export
//! selection. Accepting an exclusion records a *user* exclusion with the AI's reason.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{Icon, Sizable as _, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{PeopleEstimate, SceneCategory};
use ps_service::ai::{AiStatus, TestReport};
use ps_service::insights::{
    Correction, InsightFilter, InsightItem, InsightStamp, SuggestionStatus,
};
use ui_gallery::source::SharedSource;
use ui_theme::Tokens;
use ui_theme::fonts::MONO_FONT_FAMILY;

use crate::ShellGlobals;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::palette::{CommandMeta, CommandRegistry};
use crate::review_source::{ReviewGlobal, ReviewSource, SecretsGlobal};
use crate::settings::AppSettings;
use crate::views::review_kit::*;
use crate::views::{ViewId, ViewRegistry};
use crate::widgets::eyebrow;

/// How often the view checks for new results (spec: visible within 1 s).
pub const POLL: Duration = Duration::from_millis(500);
/// Results loaded into the view.
pub const LIMIT: u64 = 5_000;

actions!(
    ai_review,
    [
        /// Start or resume the AI queue.
        StartQueue,
        /// Pause the AI queue (in-flight requests finish).
        PauseQueue,
        /// Test the provider connection (lists models; sends no photo).
        TestConnection,
    ]
);

pub fn register(cx: &mut App) {
    for meta in [
        CommandMeta::new("AI", "Start or Resume AI Queue", StartQueue)
            .keywords(&["llm", "describe", "run"]),
        CommandMeta::new("AI", "Pause AI Queue", PauseQueue).keywords(&["stop", "llm"]),
        CommandMeta::new("AI", "Test AI Provider Connection", TestConnection).keywords(&[
            "lm studio",
            "model",
            "check",
        ]),
    ] {
        CommandRegistry::register(cx, meta);
    }
    ViewRegistry::register(cx, ViewId::AiReview, |_, window, cx| {
        let source = ReviewGlobal::get(cx);
        let gallery = cx.global::<ShellGlobals>().gallery.clone();
        let view = cx.new(|cx| AiReviewView::new(source, gallery, window, cx));
        cx.set_global(Current(view.downgrade()));
        view.into()
    });
}

struct Current(WeakEntity<AiReviewView>);

impl Global for Current {}

/// The open AI Review view (tests).
pub fn current(cx: &App) -> Option<Entity<AiReviewView>> {
    cx.try_global::<Current>()?.0.upgrade()
}

/// Category labels for chips.
pub fn category_label(c: SceneCategory) -> &'static str {
    match c {
        SceneCategory::Portrait => "Portrait",
        SceneCategory::People => "People",
        SceneCategory::Pets => "Pets",
        SceneCategory::Animals => "Animals",
        SceneCategory::Nature => "Nature",
        SceneCategory::Landscape => "Landscape",
        SceneCategory::Travel => "Travel",
        SceneCategory::Urban => "Urban",
        SceneCategory::Food => "Food",
        SceneCategory::Event => "Event",
        SceneCategory::Sports => "Sports",
        SceneCategory::Document => "Document",
        SceneCategory::Screenshot => "Screenshot",
        SceneCategory::Artwork => "Artwork",
        SceneCategory::Object => "Object",
        SceneCategory::Other => "Other",
    }
}

pub struct AiReviewView {
    source: Option<Arc<dyn ReviewSource>>,
    images: ReviewImages,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    pub filter: InsightFilter,
    items: Option<Rc<Vec<InsightItem>>>,
    selected: Option<String>,
    stamp: Option<InsightStamp>,
    status: Option<AiStatus>,
    test: Option<Result<TestReport, String>>,
    testing: bool,
    tags: Entity<InputState>,
    pets: Entity<InputState>,
    notice: Option<String>,
    error: Option<String>,
    busy: bool,
    generation: u64,
    /// The selection changed in a background reload: re-sync the editors on the next render.
    editors_dirty: bool,
    _poll: Option<Task<()>>,
}

impl AiReviewView {
    pub fn new(
        source: Option<Arc<dyn ReviewSource>>,
        gallery: SharedSource,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("No tags"));
        let pets = cx.new(|cx| InputState::new(window, cx).placeholder("No pets"));
        let mut this = Self {
            source,
            images: ReviewImages::new(gallery),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            filter: InsightFilter::default(),
            items: None,
            selected: None,
            stamp: None,
            status: None,
            test: None,
            testing: false,
            tags,
            pets,
            notice: None,
            error: None,
            busy: false,
            generation: 0,
            editors_dirty: false,
            _poll: None,
        };
        if crate::startup::StartupGrace::remaining(cx).is_some() {
            this.load_now(window, cx);
        } else {
            this.reload(cx);
        }
        this.start_polling(cx);
        this
    }

    fn load_now(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        self.stamp = src.ai_stamp().ok();
        self.status = src.ai_status().ok();
        if let Ok(items) = src.ai_items(&self.filter, LIMIT) {
            for i in items.iter().take(14) {
                self.images.prime_thumb(i.digest);
            }
            self.selected = items.first().map(|i| i.content.clone());
            self.items = Some(Rc::new(items));
            self.sync_editors(window, cx);
        }
    }

    // ---- state for tests --------------------------------------------------------------------------------

    pub fn items(&self) -> Option<&[InsightItem]> {
        self.items.as_ref().map(|i| i.as_slice())
    }

    pub fn selected(&self) -> Option<&InsightItem> {
        let id = self.selected.as_ref()?;
        self.items.as_ref()?.iter().find(|i| &i.content == id)
    }

    pub fn status(&self) -> Option<&AiStatus> {
        self.status.as_ref()
    }

    pub fn test_report(&self) -> Option<&Result<TestReport, String>> {
        self.test.as_ref()
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    pub fn tags_input(&self) -> &Entity<InputState> {
        &self.tags
    }

    // ---- loading ----------------------------------------------------------------------------------------

    fn start_polling(&mut self, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let src = src.clone();
                let probe = cx
                    .background_executor()
                    .spawn(async move { (src.ai_stamp().ok(), src.ai_status().ok()) })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let (stamp, status) = probe;
                    if status != this.status {
                        this.status = status;
                        cx.notify();
                    }
                    if stamp.is_some() && stamp != this.stamp {
                        this.stamp = stamp;
                        this.reload(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    pub fn reload(&mut self, cx: &mut Context<'_, Self>) {
        let Some(src) = self.source.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let filter = self.filter.clone();
        let task = cx.background_executor().spawn(async move {
            (
                src.ai_items(&filter, LIMIT),
                src.ai_stamp().ok(),
                src.ai_status().ok(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (items, stamp, status) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if stamp.is_some() {
                    this.stamp = stamp;
                }
                this.status = status.or(this.status.take());
                match items {
                    Ok(items) => {
                        let keep = this
                            .selected
                            .as_ref()
                            .is_some_and(|s| items.iter().any(|i| &i.content == s));
                        let changed = !keep;
                        if !keep {
                            this.selected = items.first().map(|i| i.content.clone());
                        }
                        this.items = Some(Rc::new(items));
                        if changed {
                            this.editors_dirty = true;
                        }
                    }
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (tags, pets) = self
            .selected()
            .map(|i| (i.effective.tags.join(", "), i.effective.pets.join(", ")))
            .unwrap_or_default();
        self.tags.update(cx, |s, cx| s.set_value(tags, window, cx));
        self.pets.update(cx, |s, cx| s.set_value(pets, window, cx));
    }

    pub fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(i) = self.items.as_ref().and_then(|i| i.get(ix)) {
            self.selected = Some(i.content.clone());
            self.notice = None;
            self.sync_editors(window, cx);
            cx.notify();
        }
    }

    pub fn set_filter(&mut self, filter: InsightFilter, cx: &mut Context<'_, Self>) {
        self.filter = filter;
        self.reload(cx);
        cx.notify();
    }

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
        self.error = None;
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

    // ---- actions ----------------------------------------------------------------------------------------

    /// Store a correction for the selected photo.
    pub fn correct(&mut self, c: Correction, cx: &mut Context<'_, Self>) {
        let Some(content) = self.selected.clone() else {
            return;
        };
        self.run(
            cx,
            move |s| s.ai_correct(&content, c),
            |this, (), cx| {
                this.notice =
                    Some("Saved as your correction. The AI's original answer is kept.".into());
                this.reload(cx);
            },
        );
    }

    /// Back to the AI's values.
    pub fn revert(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(content) = self.selected.clone() else {
            return;
        };
        let _ = window;
        self.run(
            cx,
            move |s| s.ai_clear(&content),
            |this, (), cx| {
                this.notice = Some("Back to the AI's values.".into());
                this.editors_dirty = true;
                this.reload(cx);
            },
        );
    }

    pub fn set_category(&mut self, c: SceneCategory, cx: &mut Context<'_, Self>) {
        self.correct(
            Correction {
                category: Some(c),
                ..Correction::default()
            },
            cx,
        );
    }

    pub fn adjust_people(&mut self, adults: i32, children: i32, cx: &mut Context<'_, Self>) {
        let Some(i) = self.selected() else { return };
        let p = i.effective.people;
        let a = (p.adults as i32 + adults).max(0) as u32;
        let c = (p.children as i32 + children).max(0) as u32;
        self.correct(
            Correction {
                people: Some(PeopleEstimate {
                    total: a + c,
                    adults: a,
                    children: c,
                }),
                ..Correction::default()
            },
            cx,
        );
    }

    /// Save the tag and pet fields as corrections.
    pub fn save_lists(&mut self, cx: &mut Context<'_, Self>) {
        let split = |s: &str| -> Vec<String> {
            s.split(',')
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .collect()
        };
        let tags = split(&self.tags.read(cx).value());
        let pets = split(&self.pets.read(cx).value());
        let Some(i) = self.selected() else { return };
        let mut c = Correction::default();
        if tags != i.effective.tags {
            c.tags = Some(tags);
        }
        if pets != i.effective.pets {
            c.pets = Some(pets);
        }
        if !c.is_empty() {
            self.correct(c, cx);
        }
    }

    /// Accept or reject the selected photo's suggestion.
    pub fn decide(&mut self, accept: bool, cx: &mut Context<'_, Self>) {
        let Some(content) = self.selected.clone() else {
            return;
        };
        self.decide_many(vec![content], accept, cx);
    }

    /// Accept or reject several suggestions at once.
    pub fn decide_many(&mut self, contents: Vec<String>, accept: bool, cx: &mut Context<'_, Self>) {
        let n = contents.len();
        self.run(
            cx,
            move |s| {
                let mut files = 0;
                for c in &contents {
                    files += s.ai_decide(c, accept)?;
                }
                Ok(files)
            },
            move |this, files, cx| {
                this.notice = Some(if accept {
                    format!(
                        "Excluded {files} file{} by you ({n} suggestion{}). Undo any time in Excluded.",
                        if files == 1 { "" } else { "s" },
                        if n == 1 { "" } else { "s" }
                    )
                } else {
                    format!("Kept as photo{}.", if n == 1 { "" } else { "s" })
                });
                this.reload(cx);
                crate::views::library::refresh_after_decision(cx);
            },
        );
    }

    /// Accept every open suggestion in the current list.
    pub fn accept_all_shown(&mut self, cx: &mut Context<'_, Self>) {
        let contents = self.open_suggestions();
        if !contents.is_empty() {
            self.decide_many(contents, true, cx);
        }
    }

    fn open_suggestions(&self) -> Vec<String> {
        self.items
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .filter(|i| {
                        i.suggestion
                            .as_ref()
                            .is_some_and(|s| s.status == SuggestionStatus::Open)
                    })
                    .map(|i| i.content.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn start_queue(&mut self, cx: &mut Context<'_, Self>) {
        let settings = AppSettings::get(cx).clone();
        let secrets = SecretsGlobal::get(cx);
        self.run(
            cx,
            move |s| s.ai_start(&settings, &secrets),
            |this, added, cx| {
                this.notice = Some(format!(
                    "AI queue running{}. Results appear here as they arrive.",
                    if added > 0 {
                        format!(" ({} photos queued)", format_count(added))
                    } else {
                        String::new()
                    }
                ));
                this.reload(cx);
            },
        );
    }

    pub fn pause_queue(&mut self, cx: &mut Context<'_, Self>) {
        self.run(
            cx,
            |s| s.ai_pause(),
            |this, (), cx| {
                this.notice = Some("Paused. Requests already sent finish and are stored.".into());
                this.reload(cx);
            },
        );
    }

    pub fn test_connection(&mut self, cx: &mut Context<'_, Self>) {
        if self.testing {
            return;
        }
        self.testing = true;
        let settings = AppSettings::get(cx).clone();
        let secrets = SecretsGlobal::get(cx);
        let task = cx
            .background_executor()
            .spawn(async move { ps_service::ai::test_connection(&settings, &secrets) });
        cx.spawn(async move |this, cx| {
            let report = task.await;
            let _ = this.update(cx, |this, cx| {
                this.testing = false;
                this.test = Some(Ok(report));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

// ---- rendering -------------------------------------------------------------------------------------------

fn people_badges(i: &InsightItem, cx: &App) -> Vec<AnyElement> {
    let colors = *ui_theme::colors(cx);
    let e = &i.effective;
    let mut out = Vec::new();
    let soft = ui_theme::Colors::tint(colors.info, 0.14);
    if e.people.adults > 0 {
        out.push(
            icon_chip(
                IconName::User,
                e.people.adults.to_string(),
                colors.info,
                soft,
                cx,
            )
            .into_any_element(),
        );
    }
    if e.people.children > 0 {
        out.push(
            icon_chip(
                IconName::Baby,
                format!(
                    "{} child{}",
                    e.people.children,
                    if e.people.children == 1 { "" } else { "ren" }
                ),
                colors.accent_text,
                colors.accent_soft,
                cx,
            )
            .into_any_element(),
        );
    }
    if e.has_pets() {
        let label = if e.pets.is_empty() {
            "pet".to_owned()
        } else {
            e.pets.join(", ")
        };
        out.push(
            icon_chip(
                IconName::PawPrint,
                label,
                colors.success,
                ui_theme::Colors::tint(colors.success, 0.14),
                cx,
            )
            .into_any_element(),
        );
    }
    out
}

/// `2 h 05 min`, `14 min`, `40 s`.
pub fn duration_label(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3_599 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {:02} min", secs / 3_600, (secs % 3_600) / 60),
    }
}

/// The running queue's progress: done of total, the pace per photo and the time left. Local vision models
/// can take a minute per photo, so the pace is shown per photo rather than per minute when it is slow.
pub fn running_label(s: &AiStatus) -> String {
    let total = s.done + s.pending + s.in_flight;
    let mut out = format!(
        "Running · {} of {}",
        format_count(s.done),
        format_count(total)
    );
    match s.per_minute.filter(|r| *r > 0.0) {
        Some(r) if r < 2.0 => out.push_str(&format!(
            " · ~{} per photo",
            duration_label((60.0 / r).round() as u64)
        )),
        Some(r) => out.push_str(&format!(" · {r:.0} photos/min")),
        None if s.in_flight > 0 => {
            out.push_str(" · describing (a local model can take a minute per photo)")
        }
        None => {}
    }
    if let Some(eta) = s.eta_s.filter(|e| *e > 0) {
        out.push_str(&format!(" · about {} left", duration_label(eta)));
    }
    out
}

fn confidence(c: f32, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    h_flex()
        .gap(Tokens::space(1.5))
        .items_center()
        .child(div().w(Tokens::size(44.0)).child(progress_bar(c, cx)))
        .child(
            div()
                .font_family(MONO_FONT_FAMILY)
                .text_size(Tokens::text_2xs())
                .text_color(colors.text_muted)
                .child(format!("{:.0}%", c * 100.0)),
        )
}

impl AiReviewView {
    fn render_header(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let settings = AppSettings::get(cx).clone();
        let provider = settings.ai.provider;
        let model = if settings.ai.model.is_empty() {
            "loaded model".to_owned()
        } else {
            settings.ai.model.clone()
        };
        let status = self.status.clone();
        let state = status.as_ref().map_or("paused", |s| s.state.as_str());
        let running = state == "running";
        let (pill_fg, pill_text) = match &status {
            Some(s) if s.state == "running" => (colors.success, running_label(s)),
            Some(s) if s.pending + s.in_flight > 0 => (
                colors.warning,
                format!("Paused · {} waiting", format_count(s.pending + s.in_flight)),
            ),
            Some(s) if s.state == "finished" => (colors.text_muted, "Finished".to_owned()),
            Some(_) => (
                colors.text_muted,
                "Not started · nothing is sent until you start".to_owned(),
            ),
            None => (colors.text_muted, "…".to_owned()),
        };
        let conn = match (&self.test, self.testing) {
            (_, true) => Some((colors.text_muted, "Testing…".to_owned())),
            (Some(Ok(r)), _) if r.reachable => Some((
                colors.success,
                format!(
                    "Connected{}{}",
                    r.latency
                        .map(|l| format!(" · {} ms", l.as_millis()))
                        .unwrap_or_default(),
                    match r.model_found {
                        Some(false) => " · model not found",
                        _ => "",
                    }
                ),
            )),
            (Some(Ok(r)), _) => Some((
                colors.danger,
                format!("Unreachable: {}", r.error.clone().unwrap_or_default()),
            )),
            (Some(Err(e)), _) => Some((colors.danger, e.clone())),
            _ => None,
        };
        let demo = self
            .items
            .as_ref()
            .is_some_and(|i| i.iter().any(InsightItem::is_demo));
        let count = self.items.as_ref().map_or(0, |i| i.len());
        v_flex()
            .flex_none()
            .px(Tokens::space(5.0))
            .pt(Tokens::space(4.0))
            .pb(Tokens::space(3.0))
            .gap(Tokens::space(3.0))
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap(Tokens::space(3.0))
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(Tokens::space(2.0))
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_size(Tokens::text_xl())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("AI Review"),
                                    )
                                    .child(
                                        div()
                                            .id("ai-review-count")
                                            .test_support()
                                            .font_family(MONO_FONT_FAMILY)
                                            .text_size(Tokens::text_xs())
                                            .text_color(colors.text_muted)
                                            .child(format!("{} photos · newest first", format_count(count as u64))),
                                    )
                                    .when(demo, |e| {
                                        e.child(
                                            chip(
                                                "Demo data · recorded fixtures, not your model",
                                                colors.warning,
                                                ui_theme::Colors::tint(colors.warning, 0.14),
                                                cx,
                                            )
                                            .id("ai-review-demo"),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap(Tokens::space(2.0))
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_muted)
                                    .child(Icon::new(IconName::Bot).size(Tokens::icon_sm()))
                                    .child(format!("{} · {model}", provider.label()))
                                    .child(if provider.is_hosted() {
                                        chip("hosted · uploads previews", colors.warning, ui_theme::Colors::tint(colors.warning, 0.14), cx)
                                    } else {
                                        chip("local · nothing leaves this computer", colors.success, ui_theme::Colors::tint(colors.success, 0.14), cx)
                                    })
                                    .when_some(conn, |e, (fg, t)| {
                                        e.child(
                                            h_flex()
                                                .id("ai-review-connection")
                                                .test_support()
                                                .gap(Tokens::space(1.0))
                                                .text_color(fg)
                                                .child(div().size(Tokens::dot_size()).rounded_full().bg(fg))
                                                .child(t),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .id("ai-review-queue")
                            .test_support()
                            .gap(Tokens::space(1.5))
                            .px(Tokens::space(2.5))
                            .h(Tokens::control_height())
                            .items_center()
                            .rounded(Tokens::RADIUS_LG)
                            .bg(colors.raised)
                            .border_1()
                            .border_color(colors.border)
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text)
                            .child(div().size(Tokens::dot_size()).rounded_full().bg(pill_fg))
                            .child(pill_text),
                    )
                    .child(action_button(
                        "ai-review-test",
                        "Test connection",
                        Some(IconName::Plug),
                        None,
                        Tone::Normal,
                        cx,
                        |this, _, cx| this.test_connection(cx),
                    ))
                    .child(if running {
                        action_button("ai-review-pause", "Pause", Some(IconName::Pause), None, Tone::Normal, cx, |this, _, cx| {
                            this.pause_queue(cx)
                        })
                    } else {
                        action_button(
                            "ai-review-start",
                            if status.as_ref().is_some_and(|s| s.done > 0) { "Resume" } else { "Start" },
                            Some(IconName::Play),
                            None,
                            Tone::Primary,
                            cx,
                            |this, _, cx| this.start_queue(cx),
                        )
                    }),
            )
            .child(
                h_flex()
                    .id("ai-review-advisory")
                    .test_support()
                    .gap(Tokens::space(2.0))
                    .px(Tokens::space(3.0))
                    .py(Tokens::space(2.0))
                    .rounded(Tokens::RADIUS_MD)
                    .bg(colors.accent_soft)
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text)
                    .child(Icon::new(IconName::Sparkles).size(Tokens::icon_sm()).text_color(colors.accent_text))
                    .child(div().flex_1().child(
                        "AI descriptions are suggestions. They never change duplicate groups, keepers, exclusions or \
                         what is exported. You decide, and your corrections always win.",
                    )),
            )
            .child(self.render_filters(cx))
            .into_any_element()
    }

    fn render_filters(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let f = self.filter.clone();
        let weak = cx.weak_entity();
        let category = {
            let f = f.clone();
            Button::new("ai-review-category")
                .label(
                    f.category
                        .map_or("Any category".to_owned(), |c| category_label(c).to_owned()),
                )
                .compact()
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.category = None;
                        menu = menu.item(
                            PopupMenuItem::new("Any category")
                                .checked(f.category.is_none())
                                .on_click(move |_, _, cx| {
                                    let next = next.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filter(next, cx));
                                }),
                        );
                    }
                    for c in SceneCategory::ALL {
                        let weak = weak.clone();
                        let mut next = f.clone();
                        next.category = Some(c);
                        menu = menu.item(
                            PopupMenuItem::new(category_label(c))
                                .checked(f.category == Some(c))
                                .on_click(move |_, _, cx| {
                                    let next = next.clone();
                                    let _ = weak.update(cx, |v, cx| v.set_filter(next, cx));
                                }),
                        );
                    }
                    menu
                })
        };
        let open = self.open_suggestions().len();
        h_flex()
            .gap(Tokens::space(1.5))
            .flex_wrap()
            .items_center()
            .child(category)
            .child(filter_chip(
                "ai-filter-children",
                "Contains children",
                f.children,
                cx,
                |v, cx| {
                    let mut f = v.filter.clone();
                    f.children = !f.children;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "ai-filter-pets",
                "Pets",
                f.pets,
                cx,
                |v, cx| {
                    let mut f = v.filter.clone();
                    f.pets = !f.pets;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "ai-filter-people",
                "People",
                f.people,
                cx,
                |v, cx| {
                    let mut f = v.filter.clone();
                    f.people = !f.people;
                    v.set_filter(f, cx);
                },
            ))
            .child(filter_chip(
                "ai-filter-suggestions",
                "Needs a decision",
                f.suggestions,
                cx,
                |v, cx| {
                    let mut f = v.filter.clone();
                    f.suggestions = !f.suggestions;
                    v.set_filter(f, cx);
                },
            ))
            .child(div().flex_1())
            .when(open > 0, |e| {
                e.child(
                    div()
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_muted)
                        .child(format!(
                            "{open} suggestion{} shown",
                            if open == 1 { "" } else { "s" }
                        )),
                )
                .child(action_button(
                    "ai-review-accept-all",
                    "Exclude all shown",
                    Some(IconName::EyeOff),
                    None,
                    Tone::Normal,
                    cx,
                    |this, _, cx| this.accept_all_shown(cx),
                ))
            })
            .into_any_element()
    }

    fn render_list(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let items = self.items.clone();
        let n = items.as_ref().map_or(0, |i| i.len());
        let selected = self.selected.clone();
        let images = self.images.clone();
        if n == 0 {
            return v_flex()
                .id("ai-review-empty")
                .test_support()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(Tokens::space(2.0))
                .bg(colors.gallery)
                .child(
                    Icon::new(IconName::Sparkles)
                        .size(Tokens::icon_lg())
                        .text_color(colors.text_faint),
                )
                .child(
                    div()
                        .max_w(Tokens::prose_width())
                        .text_center()
                        .text_color(if self.error.is_some() {
                            colors.danger
                        } else {
                            colors.text_muted
                        })
                        .child(match &self.error {
                            Some(e) => e.clone(),
                            None if !self.filter.is_empty() => {
                                "No described photo matches these filters.".to_owned()
                            }
                            None if items.is_some() => {
                                "No photos described yet. Start the queue to describe \
                                                        your keepers with your local model."
                                    .to_owned()
                            }
                            None => "Loading…".to_owned(),
                        }),
                )
                .into_any_element();
        }
        uniform_list(
            "ai-review-items",
            n,
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                let Some(items) = items.clone() else {
                    return Vec::new();
                };
                range
                    .map(|ix| {
                        let i = &items[ix];
                        let on = selected.as_deref() == Some(i.content.as_str());
                        let img = images.thumb(Some(i.digest), cx);
                        let e = &i.effective;
                        let mut cats = vec![e.category];
                        cats.extend(e.also.iter().copied().take(2));
                        let suggestion = i
                            .suggestion
                            .clone()
                            .filter(|s| s.status == SuggestionStatus::Open);
                        h_flex()
                            .id(("ai-review-row", ix))
                            .w_full()
                            .h(Tokens::size(84.0))
                            .px(Tokens::space(4.0))
                            .gap(Tokens::space(3.0))
                            .items_center()
                            .border_b_1()
                            .border_color(colors.border)
                            .cursor_pointer()
                            .when(on, |d| d.bg(colors.accent_soft))
                            .when(!on, |d| d.hover(|d| d.bg(colors.hover)))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.select(ix, window, cx)),
                            )
                            .child(
                                div()
                                    .size(Tokens::size(64.0))
                                    .flex_none()
                                    .rounded(Tokens::RADIUS_MD)
                                    .overflow_hidden()
                                    .child(photo(img, false, cx)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(Tokens::space(1.5))
                                    .child(
                                        h_flex()
                                            .gap(Tokens::space(2.0))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_size(Tokens::text_sm())
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .child(e.subject.clone()),
                                            )
                                            .when(!i.corrected.is_empty(), |d| {
                                                d.child(chip(
                                                    "edited by you",
                                                    colors.accent_text,
                                                    colors.accent_soft,
                                                    cx,
                                                ))
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(Tokens::space(1.5))
                                            .flex_wrap()
                                            .children(cats.into_iter().map(|c| {
                                                chip(
                                                    category_label(c),
                                                    colors.text,
                                                    colors.raised,
                                                    cx,
                                                )
                                            }))
                                            .children(people_badges(i, cx))
                                            .when_some(suggestion, |d, s| {
                                                d.child(icon_chip(
                                                    IconName::TriangleAlert,
                                                    format!("Exclude as {}?", s.label),
                                                    colors.warning,
                                                    ui_theme::Colors::tint(colors.warning, 0.14),
                                                    cx,
                                                ))
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(Tokens::space(2.0))
                                            .text_size(Tokens::text_2xs())
                                            .text_color(colors.text_faint)
                                            .child(confidence(e.confidence, cx))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .font_family(MONO_FONT_FAMILY)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .child(i.name.clone()),
                                            ),
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .into_any_element()
    }

    fn stepper(
        &self,
        id: &'static str,
        label: &'static str,
        value: u32,
        adults: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let (da, dc) = if adults { (1, 0) } else { (0, 1) };
        h_flex()
            .gap(Tokens::space(2.0))
            .items_center()
            .child(
                div()
                    .w(Tokens::size(72.0))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(action_button(
                format!("{id}-minus"),
                "−",
                None,
                None,
                Tone::Normal,
                cx,
                move |this, _, cx| this.adjust_people(-da, -dc, cx),
            ))
            .child(
                div()
                    .id(id)
                    .test_support()
                    .w(Tokens::size(28.0))
                    .text_center()
                    .font_family(MONO_FONT_FAMILY)
                    .child(value.to_string()),
            )
            .child(action_button(
                format!("{id}-plus"),
                "+",
                None,
                None,
                Tone::Normal,
                cx,
                move |this, _, cx| this.adjust_people(da, dc, cx),
            ))
            .into_any_element()
    }

    fn render_detail(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let Some(i) = self.selected().cloned() else {
            return div()
                .w(Tokens::size(360.0))
                .flex_none()
                .h_full()
                .bg(colors.panel)
                .border_l_1()
                .border_color(colors.border)
                .into_any_element();
        };
        let img = self.images.thumb(Some(i.digest), cx);
        let e = i.effective.clone();
        let corrected = |f: &str| i.corrected.iter().any(|c| c == f);
        let weak = cx.weak_entity();
        let cat_menu = Button::new("ai-review-set-category")
            .label(category_label(e.category))
            .compact()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for c in SceneCategory::ALL {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(category_label(c))
                            .checked(c == e.category)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |v, cx| v.set_category(c, cx));
                            }),
                    );
                }
                menu
            });
        let e = i.effective.clone();
        let suggestion = i.suggestion.clone();
        let mark = |on: bool| -> AnyElement {
            if on {
                div()
                    .text_size(Tokens::text_2xs())
                    .text_color(colors.accent_text)
                    .child("yours")
                    .into_any_element()
            } else {
                div()
                    .text_size(Tokens::text_2xs())
                    .text_color(colors.text_faint)
                    .child("AI")
                    .into_any_element()
            }
        };
        v_flex()
            .id("ai-review-detail")
            .test_support()
            .w(Tokens::size(360.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .p(Tokens::space(4.0))
            .gap(Tokens::space(3.0))
            .child(
                div()
                    .w_full()
                    .h(Tokens::size(220.0))
                    .rounded(Tokens::RADIUS_LG)
                    .overflow_hidden()
                    .bg(colors.gallery)
                    .child(photo(img, true, cx)),
            )
            .child(
                div()
                    .id("ai-detail-subject")
                    .test_support()
                    .text_size(Tokens::text_md())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(e.subject.clone()),
            )
            .when_some(e.setting.clone(), |d, s| {
                d.child(div().text_size(Tokens::text_sm()).text_color(colors.text_muted).child(s))
            })
            .when_some(suggestion.filter(|s| s.status == SuggestionStatus::Open), |d, s| {
                d.child(
                    v_flex()
                        .id("ai-detail-suggestion")
                        .test_support()
                        .gap(Tokens::space(2.0))
                        .p(Tokens::space(3.0))
                        .rounded(Tokens::RADIUS_MD)
                        .border_1()
                        .border_color(colors.warning)
                        .bg(ui_theme::Colors::tint(colors.warning, 0.08))
                        .child(
                            div().text_size(Tokens::text_sm()).child(format!(
                                "The AI thinks this is a {} ({:.0}% sure). It stays in your Library unless you exclude it.",
                                s.label,
                                s.confidence * 100.0
                            )),
                        )
                        .child(
                            h_flex()
                                .gap(Tokens::space(2.0))
                                .child(action_button(
                                    "ai-detail-accept",
                                    format!("Exclude as {}", s.label),
                                    Some(IconName::EyeOff),
                                    None,
                                    Tone::Primary,
                                    cx,
                                    |this, _, cx| this.decide(true, cx),
                                ))
                                .child(action_button(
                                    "ai-detail-reject",
                                    "Keep as photo",
                                    None,
                                    None,
                                    Tone::Quiet,
                                    cx,
                                    |this, _, cx| this.decide(false, cx),
                                )),
                        ),
                )
            })
            .child(eyebrow("What the AI saw · edit to correct", cx))
            .child(
                h_flex()
                    .id("ai-detail-category")
                    .test_support()
                    .gap(Tokens::space(2.0))
                    .items_center()
                    .child(div().w(Tokens::size(72.0)).text_size(Tokens::text_sm()).text_color(colors.text_muted).child("Category"))
                    .child(cat_menu)
                    .child(mark(corrected("category"))),
            )
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(self.stepper("ai-detail-adults", "Adults", e.people.adults, true, cx))
                    .child(mark(corrected("people"))),
            )
            .child(self.stepper("ai-detail-children", "Children", e.people.children, false, cx))
            .child(
                v_flex()
                    .gap(Tokens::space(1.0))
                    .child(h_flex().gap(Tokens::space(2.0)).child(div().text_size(Tokens::text_sm()).text_color(colors.text_muted).child("Pets")).child(mark(corrected("pets"))))
                    .child(Input::new(&self.pets).small().aria_label("Pets")),
            )
            .child(
                v_flex()
                    .gap(Tokens::space(1.0))
                    .child(h_flex().gap(Tokens::space(2.0)).child(div().text_size(Tokens::text_sm()).text_color(colors.text_muted).child("Tags")).child(mark(corrected("tags"))))
                    .child(Input::new(&self.tags).small().aria_label("Tags")),
            )
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(action_button("ai-detail-save", "Save corrections", Some(IconName::Check), None, Tone::Normal, cx, |this, _, cx| {
                        this.save_lists(cx)
                    }))
                    .when(!i.corrected.is_empty(), |d| {
                        d.child(action_button("ai-detail-revert", "Revert to AI", Some(IconName::Undo2), None, Tone::Quiet, cx, |this, window, cx| {
                            this.revert(window, cx)
                        }))
                    }),
            )
            .child(eyebrow("Confidence and provenance", cx))
            .child(confidence(e.confidence, cx))
            .child(
                div()
                    .id("ai-detail-provenance")
                    .test_support()
                    .font_family(MONO_FONT_FAMILY)
                    .text_size(Tokens::text_2xs())
                    .text_color(colors.text_muted)
                    .child(format!(
                        "{} · {} · {} ms · {}",
                        i.provider,
                        i.model,
                        i.latency_ms,
                        date_of(i.at_ms)
                    )),
            )
            .child(
                div()
                    .font_family(MONO_FONT_FAMILY)
                    .text_size(Tokens::text_2xs())
                    .text_color(colors.text_faint)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(crate::views::kit::display_path(std::path::Path::new(&i.path))),
            )
            .children(self.notice.clone().map(|n| {
                div()
                    .id("ai-review-notice")
                    .test_support()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.success)
                    .child(n)
            }))
            .children(self.error.clone().map(|n| {
                div()
                    .id("ai-review-error")
                    .test_support()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.danger)
                    .child(n)
            }))
            .into_any_element()
    }
}

impl Focusable for AiReviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AiReviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.images.release(window);
        if std::mem::take(&mut self.editors_dirty) {
            self.sync_editors(window, cx);
        }
        let colors = *ui_theme::colors(cx);
        if self.source.is_none() {
            return v_flex()
                .id("view-ai_review")
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .bg(colors.gallery)
                .text_color(colors.text_muted)
                .child("AI review needs a library. Add a source to begin.")
                .into_any_element();
        }
        let header = self.render_header(cx);
        let list = self.render_list(cx);
        let detail = self.render_detail(cx);
        v_flex()
            .id("view-ai_review")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .on_action(cx.listener(|this, _: &StartQueue, _, cx| this.start_queue(cx)))
            .on_action(cx.listener(|this, _: &PauseQueue, _, cx| this.pause_queue(cx)))
            .on_action(cx.listener(|this, _: &TestConnection, _, cx| this.test_connection(cx)))
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(v_flex().flex_1().min_w_0().h_full().child(list))
                    .child(detail),
            )
            .into_any_element()
    }
}
