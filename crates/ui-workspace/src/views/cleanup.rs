//! The Cleanup panel of the Reports view (change `quarantine-cleanup`, task 5.3; ADR-0024, ADR-0061): review
//! a plan (what moves where, per folder, how much space you can free), an explicit confirmation, live progress,
//! and the history of runs with Undo.
//!
//! Nothing moves without the confirmation: "Move N files…" only opens it, and only its own button starts the
//! run. Undo also asks first. Every call goes to the backend on a background thread; progress is sampled
//! (ADR-0019).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::SourceId;
use ui_theme::{Colors, Tokens};

use crate::assets::IconName;
use crate::inspector::format_count;
use crate::scan_backend::{
    CleanupOutcome, CleanupProgress, CleanupReview, CleanupRunRow, CleanupRunState,
    SharedScanBackend, UndoOutcome,
};
use crate::service::Loadable;
use crate::views::kit::{self, Tone};

/// Progress is sampled this often (ADR-0019).
const SAMPLE: Duration = Duration::from_millis(100);

/// Rows of a plan's folder list and move list shown before "and N more".
const FOLDER_ROWS: usize = 5;
const MOVE_ROWS: usize = 3;

/// The run changed files (a cleanup or an undo finished): the Reports view refreshes its counts.
#[derive(Clone, Debug)]
pub struct CleanupChanged;

/// Where the panel stands.
#[derive(Clone)]
pub enum CleanupPhase {
    /// Nothing selected: the history only.
    Idle,
    /// Planning a source.
    Planning(SourceId),
    /// A planned run is shown for review.
    Review,
    /// The user clicked "Move N files…": the confirmation is shown.
    Confirming,
    Running {
        progress: Arc<CleanupProgress>,
        cancel: Arc<AtomicBool>,
    },
    Done(CleanupOutcome),
    Failed(String),
}

type Slot<T> = Arc<Mutex<Option<anyhow::Result<T>>>>;

pub struct CleanupView {
    backend: Option<SharedScanBackend>,
    pub runs: Loadable<Vec<CleanupRunRow>>,
    pub review: Option<CleanupReview>,
    pub phase: CleanupPhase,
    /// The user confirmed that the source is a network share (needed for those).
    pub network_ack: bool,
    /// Undo asked for this run; the confirmation is shown.
    pub undo_confirm: Option<i64>,
    pub undoing: Option<i64>,
    pub last_undo: Option<Result<UndoOutcome, String>>,
    /// Nothing to clean up on the source last planned.
    pub nothing_on: Option<SourceId>,
    task: Option<Task<()>>,
    run_task: Option<Task<()>>,
}

impl EventEmitter<CleanupChanged> for CleanupView {}

impl CleanupView {
    pub fn new(backend: Option<SharedScanBackend>, cx: &mut Context<'_, Self>) -> Self {
        let mut this = Self {
            backend,
            runs: Loadable::Loading,
            review: None,
            phase: CleanupPhase::Idle,
            network_ack: false,
            undo_confirm: None,
            undoing: None,
            last_undo: None,
            nothing_on: None,
            task: None,
            run_task: None,
        };
        // Shown at start-up: the history (and the latest plan) are in frame one when quick (start-up grace).
        let early = this.backend.clone().and_then(|b| {
            let deadline = crate::startup::StartupGrace::deadline(cx)?;
            crate::startup::StartupGrace::within(deadline, move || load(&b))
        });
        match early {
            Some(r) => this.loaded(r),
            None => this.refresh(cx),
        }
        this
    }

    fn loaded(&mut self, r: anyhow::Result<(Vec<CleanupRunRow>, Option<CleanupReview>)>) {
        match r {
            Ok((runs, review)) => {
                self.runs = Loadable::Ready(runs);
                // Show the newest planned run for review unless something else is on screen.
                if matches!(self.phase, CleanupPhase::Idle | CleanupPhase::Review) {
                    self.phase = if review.is_some() {
                        CleanupPhase::Review
                    } else {
                        CleanupPhase::Idle
                    };
                    self.review = review;
                }
            }
            Err(e) => self.runs = Loadable::Failed(format!("{e:#}")),
        }
    }

    /// Reload the history (and the newest planned run).
    pub fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let reply = cx.background_spawn(async move { load(&b) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.loaded(r);
                cx.notify();
            });
        }));
    }

    /// Something to show: a run, a plan, or work in progress.
    pub fn has_content(&self) -> bool {
        !matches!(self.phase, CleanupPhase::Idle)
            || self.nothing_on.is_some()
            || self.last_undo.is_some()
            || self.runs.ready().is_some_and(|r| !r.is_empty())
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            CleanupPhase::Running { .. } | CleanupPhase::Planning(_)
        ) || self.undoing.is_some()
    }

    /// Plan a cleanup of a whole source (stored as a planned run; moves nothing) and show it for review.
    pub fn plan(&mut self, source: SourceId, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        if self.busy() {
            return;
        }
        self.phase = CleanupPhase::Planning(source);
        self.nothing_on = None;
        self.network_ack = false;
        cx.notify();
        let reply = cx.background_spawn(async move { b.cleanup_plan(source) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(Some(review)) => {
                        this.review = Some(review);
                        this.phase = CleanupPhase::Review;
                    }
                    Ok(None) => {
                        this.review = None;
                        this.nothing_on = Some(source);
                        this.phase = CleanupPhase::Idle;
                    }
                    Err(e) => this.phase = CleanupPhase::Failed(format!("{e:#}")),
                }
                this.refresh(cx);
                cx.notify();
            });
        }));
    }

    /// Show a stored run for review.
    pub fn review_run(&mut self, run: i64, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        if self.busy() {
            return;
        }
        let reply = cx.background_spawn(async move { b.cleanup_review(run) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(review) => {
                        this.review = Some(review);
                        this.network_ack = false;
                        this.phase = CleanupPhase::Review;
                    }
                    Err(e) => this.phase = CleanupPhase::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        }));
    }

    /// "Move N files…": show the confirmation. Moves nothing.
    pub fn ask(&mut self, cx: &mut Context<'_, Self>) {
        if matches!(self.phase, CleanupPhase::Review)
            && self.review.as_ref().is_some_and(|r| r.run.planned > 0)
        {
            self.phase = CleanupPhase::Confirming;
            cx.notify();
        }
    }

    /// Back from the confirmation, or close the plan.
    pub fn back(&mut self, cx: &mut Context<'_, Self>) {
        self.phase = match self.phase {
            CleanupPhase::Confirming => CleanupPhase::Review,
            CleanupPhase::Running { .. } => return,
            _ => {
                self.review = None;
                CleanupPhase::Idle
            }
        };
        cx.notify();
    }

    fn can_start(&self) -> bool {
        matches!(self.phase, CleanupPhase::Confirming)
            && self
                .review
                .as_ref()
                .is_some_and(|r| r.run.planned > 0 && (!r.run.network || self.network_ack))
    }

    /// Start the confirmed run.
    pub fn start(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(b), Some(review)) = (self.backend.clone(), self.review.clone()) else {
            return;
        };
        if !self.can_start() {
            return;
        }
        let progress = Arc::new(CleanupProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let slot: Slot<CleanupOutcome> = Arc::new(Mutex::new(None));
        let network = self.network_ack;
        {
            let (progress, cancel, slot) = (progress.clone(), cancel.clone(), slot.clone());
            let id = review.run.id;
            // Re-hashing every file is a long blocking job: its own thread, polled below.
            let spawned = std::thread::Builder::new()
                .name("cleanup".into())
                .spawn(move || {
                    let r = b.cleanup_run(id, network, &cancel, &progress);
                    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                });
            if let Err(e) = spawned {
                self.phase = CleanupPhase::Failed(format!("could not start the cleanup: {e}"));
                cx.notify();
                return;
            }
        }
        self.phase = CleanupPhase::Running { progress, cancel };
        cx.notify();
        self.run_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let done = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
                let finished = this
                    .update(cx, |this, cx| {
                        let finished = done.is_some();
                        match done {
                            Some(Ok(o)) => this.phase = CleanupPhase::Done(o),
                            Some(Err(e)) => this.phase = CleanupPhase::Failed(format!("{e:#}")),
                            None => {}
                        }
                        if finished {
                            this.review = None;
                            this.refresh(cx);
                            cx.emit(CleanupChanged);
                        }
                        cx.notify();
                        finished
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
                cx.background_executor().timer(SAMPLE).await;
            }
        }));
    }

    /// Stop between files (the rest stays planned; run again to continue).
    pub fn stop(&mut self) {
        if let CleanupPhase::Running { cancel, .. } = &self.phase {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    /// "Undo": ask first.
    pub fn ask_undo(&mut self, run: i64, cx: &mut Context<'_, Self>) {
        if !self.busy() {
            self.undo_confirm = Some(run);
            cx.notify();
        }
    }

    /// Undo the run whose confirmation is shown: every quarantined file goes back to its place.
    pub fn undo(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(b), Some(run)) = (self.backend.clone(), self.undo_confirm.take()) else {
            return;
        };
        self.undoing = Some(run);
        self.last_undo = None;
        cx.notify();
        let slot: Slot<UndoOutcome> = Arc::new(Mutex::new(None));
        {
            let slot = slot.clone();
            let spawned = std::thread::Builder::new()
                .name("cleanup-undo".into())
                .spawn(move || {
                    let r = b.cleanup_undo(run);
                    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                });
            if let Err(e) = spawned {
                self.undoing = None;
                self.last_undo = Some(Err(format!("could not start the undo: {e}")));
                cx.notify();
                return;
            }
        }
        self.run_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let done = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
                let finished = this
                    .update(cx, |this, cx| {
                        let Some(r) = done else {
                            return false;
                        };
                        this.undoing = None;
                        this.last_undo = Some(r.map_err(|e| format!("{e:#}")));
                        if matches!(this.phase, CleanupPhase::Done(_)) {
                            this.phase = CleanupPhase::Idle;
                        }
                        this.refresh(cx);
                        cx.emit(CleanupChanged);
                        cx.notify();
                        true
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
                cx.background_executor().timer(SAMPLE).await;
            }
        }));
    }
}

/// The history and the newest planned run's review.
fn load(b: &SharedScanBackend) -> anyhow::Result<(Vec<CleanupRunRow>, Option<CleanupReview>)> {
    let runs = b.cleanup_runs()?;
    let review = match runs
        .iter()
        .find(|r| r.state == CleanupRunState::Planned && r.planned > 0)
    {
        Some(r) => Some(b.cleanup_review(r.id)?),
        None => None,
    };
    Ok((runs, review))
}

fn state_label(r: &CleanupRunRow) -> &'static str {
    match r.state {
        CleanupRunState::Planned if r.quarantined > 0 => "Partly run",
        CleanupRunState::Planned => "Planned",
        CleanupRunState::Running => "Running",
        CleanupRunState::Completed => "Quarantined",
        CleanupRunState::Undone => "Undone",
    }
}

/// A quarantine path for the plan's sample rows, at most `max` characters: the run folder
/// (`.photosquirrel-quarantine/<run>/`) is always shown whole, so the run is visible; only the part below it is
/// shortened in the middle.
pub fn quarantine_target(to: &str, max: usize) -> String {
    let prefix_len = to
        .match_indices('/')
        .nth(1)
        .filter(|_| to.starts_with(".photosquirrel-quarantine/"))
        .map(|(i, _)| i + 1);
    match prefix_len {
        Some(n) if to.chars().count() > max && max > n + 8 => {
            let (prefix, rest) = to.split_at(n);
            format!(
                "{prefix}{}",
                kit::middle_ellipsis(rest, max - prefix.chars().count())
            )
        }
        _ => kit::middle_ellipsis(to, max),
    }
}

fn mono(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
        .text_size(Tokens::text_mono())
        .text_color(color)
        .child(text.into())
}

fn note_box(
    id: &'static str,
    label: String,
    tone: Hsla,
    icon: IconName,
    colors: &Colors,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .gap(Tokens::space(2.0))
        .items_start()
        .p(Tokens::space(3.0))
        .rounded(Tokens::RADIUS_MD)
        .bg(Colors::tint(tone, 0.08))
        .border_1()
        .border_color(Colors::tint(tone, 0.3))
        .text_size(Tokens::text_sm())
        .text_color(colors.text)
        .child(Icon::new(icon).size(Tokens::icon_sm()).text_color(tone))
        .child(div().flex_1().min_w_0().child(label))
}

impl CleanupView {
    fn review_card(&self, review: &CleanupReview, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let r = &review.run;
        let quarantine = kit::display_path(&r.quarantine_dir);
        let mut card = kit::card(cx)
            .id("cleanup-plan")
            .test_support()
            .aria_label(SharedString::from(format!(
                "Cleanup plan {}: {} files, {}",
                r.id,
                r.planned,
                kit::format_bytes(r.planned_bytes)
            )))
            .child(
                kit::card_title(format!("Cleanup plan · {}", r.source_name), cx)
                    .child(div().flex_1())
                    .child(kit::chip(
                        format!("Run {}", r.id),
                        colors.text_muted,
                        colors.raised,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(6.0))
                    .child(kit::stat(
                        "cleanup-files",
                        "Files to move",
                        format_count(r.planned),
                        cx,
                    ))
                    .child(kit::stat(
                        "cleanup-bytes",
                        "Space you can free",
                        kit::format_bytes(r.planned_bytes),
                        cx,
                    ))
                    .child(kit::stat(
                        "cleanup-folders",
                        "Folders",
                        format_count(review.folders.len() as u64),
                        cx,
                    )),
            )
            .child(
                div()
                    .id("cleanup-explain")
                    .test_support()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text_muted)
                    .child(
                        "Only files that are already in your library (exported) or are copies of an exported \
                         photo (covered) are listed. Right before each move the file is re-read and hashed and its \
                         library copy re-verified; anything that changed stays where it is. Files are renamed into \
                         a quarantine folder on the same drive: nothing is copied, nothing is deleted, and Undo \
                         puts everything back. The space is freed only once you delete that folder yourself.",
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .items_center()
                    .child(
                        Icon::new(IconName::Folder)
                            .size(Tokens::icon_sm())
                            .text_color(colors.text_muted),
                    )
                    .child(mono(format!("Quarantine: {quarantine}"), colors.text_faint)),
            );

        // Per folder: files and bytes.
        let mut folders = v_flex()
            .id("cleanup-folders-list")
            .test_support()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .py(Tokens::space(1.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(Tokens::text_2xs())
                    .text_color(colors.text_faint)
                    .child(div().flex_1().child("FOLDER"))
                    .child(div().w(Tokens::size(76.0)).text_right().child("FILES"))
                    .child(div().w(Tokens::size(96.0)).text_right().child("SIZE")),
            );
        let mut by_size: Vec<&(String, u64, u64)> = review.folders.iter().collect();
        by_size.sort_by_key(|a| std::cmp::Reverse(a.2));
        for (i, (f, n, bytes)) in by_size.iter().take(FOLDER_ROWS).enumerate() {
            folders = folders.child(
                h_flex()
                    .id(SharedString::from(format!("cleanup-folder-{i}")))
                    .test_support()
                    .aria_label(SharedString::from(format!("{f}: {n} files")))
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .py(Tokens::space(1.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(mono(f.clone(), colors.text))
                    .child(
                        div()
                            .w(Tokens::size(76.0))
                            .text_right()
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text)
                            .child(format_count(*n)),
                    )
                    .child(
                        div()
                            .w(Tokens::size(96.0))
                            .text_right()
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text_muted)
                            .child(kit::format_bytes(*bytes)),
                    ),
            );
        }
        if review.folders.len() > FOLDER_ROWS {
            folders = folders.child(
                div()
                    .py(Tokens::space(1.0))
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(format!(
                        "and {} more folders",
                        review.folders.len() - FOLDER_ROWS
                    )),
            );
        }
        card = card.child(folders);

        // Where files go.
        let mut moves = v_flex().w_full().gap(Tokens::space(0.5));
        for (i, (from, to)) in review.moves.iter().take(MOVE_ROWS).enumerate() {
            moves = moves.child(
                h_flex()
                    .id(SharedString::from(format!("cleanup-move-{i}")))
                    .test_support()
                    .aria_label(SharedString::from(format!("{from} → {to}")))
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .items_center()
                    .child(mono(kit::middle_ellipsis(from, 60), colors.text))
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .size(Tokens::icon_sm())
                            .text_color(colors.text_faint),
                    )
                    .child(mono(quarantine_target(to, 70), colors.text_faint)),
            );
        }
        if r.planned as usize > MOVE_ROWS {
            moves = moves.child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(format!(
                        "and {} more files",
                        format_count(r.planned - MOVE_ROWS as u64)
                    )),
            );
        }
        card = card.child(moves);

        // The explicit confirmation.
        match self.phase {
            CleanupPhase::Confirming => {
                let ready = self.can_start();
                let mut confirm = v_flex()
                    .id("cleanup-confirmation")
                    .test_support()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .p(Tokens::space(3.0))
                    .rounded(Tokens::RADIUS_MD)
                    .bg(Colors::tint(colors.warning, 0.08))
                    .border_1()
                    .border_color(Colors::tint(colors.warning, 0.35))
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .text_color(colors.warning)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::new(IconName::TriangleAlert).size(Tokens::icon_sm()))
                            .child(format!(
                                "Move {} files ({}) out of their folders on {}?",
                                format_count(r.planned),
                                kit::format_bytes(r.planned_bytes),
                                r.source_name
                            )),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text)
                            .child(format!(
                                "They go to {quarantine}, on the same drive. Apps that open them from their old \
                                 place will not find them until you undo. You can undo this run at any time; \
                                 PhotoSquirrel never empties the quarantine (your file manager does)."
                            )),
                    );
                if r.network {
                    let ack = self.network_ack;
                    confirm = confirm.child(
                        h_flex()
                            .id("cleanup-network-ack")
                            .test_support()
                            .role(Role::CheckBox)
                            .aria_label(SharedString::from(format!(
                                "{} is a network share: renames there may not be atomic{}",
                                r.source_name,
                                if ack { " (confirmed)" } else { "" }
                            )))
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.network_ack = !this.network_ack;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .size(Tokens::size(16.0))
                                    .flex_none()
                                    .rounded(Tokens::RADIUS_SM)
                                    .border_1()
                                    .border_color(colors.border_strong)
                                    .when(ack, |d| d.bg(colors.accent))
                                    .when(ack, |d| {
                                        d.child(
                                            Icon::new(IconName::Check)
                                                .size(Tokens::icon_sm())
                                                .text_color(colors.on_accent),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(Tokens::text_sm())
                                    .text_color(colors.text)
                                    .child(format!(
                                        "{} is a network share, where a rename may not be atomic. Move anyway.",
                                        r.source_name
                                    )),
                            ),
                    );
                }
                confirm = confirm.child(
                    h_flex()
                        .w_full()
                        .gap(Tokens::space(2.0))
                        .justify_end()
                        .child(kit::button(
                            "cleanup-back",
                            "Back",
                            None,
                            Tone::Secondary,
                            kit::on(cx.listener(|this, _, _, cx| this.back(cx))),
                            cx,
                        ))
                        .child(kit::button(
                            "cleanup-start",
                            format!("Move {} files to quarantine", format_count(r.planned)),
                            Some(IconName::ShieldCheck),
                            Tone::Danger,
                            ready
                                .then(|| kit::on(cx.listener(|this, _, _, cx| this.start(cx))))
                                .flatten(),
                            cx,
                        )),
                );
                card = card.child(confirm);
            }
            _ => {
                card = card.child(
                    h_flex()
                        .w_full()
                        .gap(Tokens::space(2.0))
                        .justify_end()
                        .child(kit::button(
                            "cleanup-close",
                            "Not now",
                            None,
                            Tone::Ghost,
                            kit::on(cx.listener(|this, _, _, cx| this.back(cx))),
                            cx,
                        ))
                        .child(kit::button(
                            "cleanup-confirm",
                            format!("Move {} files…", format_count(r.planned)),
                            Some(IconName::ArrowRight),
                            Tone::Primary,
                            (r.planned > 0)
                                .then(|| kit::on(cx.listener(|this, _, _, cx| this.ask(cx))))
                                .flatten(),
                            cx,
                        )),
                );
            }
        }
        card.into_any_element()
    }

    fn history(&self, runs: &[CleanupRunRow], cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let mut list = v_flex().id("cleanup-history").test_support().w_full();
        for r in runs.iter().take(20) {
            let id = r.id;
            let confirming = self.undo_confirm == Some(id);
            let undoing = self.undoing == Some(id);
            let can_undo = r.quarantined > 0 && !self.busy();
            let summary = format!(
                "{} · {} quarantined ({}) · {} skipped · {} restored",
                state_label(r),
                format_count(r.quarantined),
                kit::format_bytes(r.quarantined_bytes),
                format_count(r.skipped),
                format_count(r.restored)
            );
            let mut row = h_flex()
                .id(SharedString::from(format!("cleanup-run-{id}")))
                .test_support()
                .aria_label(SharedString::from(format!("Run {id}: {summary}")))
                .w_full()
                .gap(Tokens::space(2.0))
                .py(Tokens::space(1.5))
                .items_center()
                .border_b_1()
                .border_color(colors.border)
                .child(
                    Icon::new(IconName::HardDrive)
                        .size(Tokens::icon_sm())
                        .text_color(colors.text_muted),
                )
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .child(format!("Run {id} · {}", r.source_name)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_muted)
                        .child(format!("{summary} · planned {}", kit::ago(r.created_at_ms))),
                );
            if r.planned > 0 && r.state != CleanupRunState::Undone {
                row = row.child(kit::button(
                    SharedString::from(format!("cleanup-review-{id}")),
                    "Review",
                    None,
                    Tone::Ghost,
                    (!self.busy())
                        .then(|| {
                            kit::on(cx.listener(move |this, _, _, cx| this.review_run(id, cx)))
                        })
                        .flatten(),
                    cx,
                ));
            }
            if r.quarantined > 0 {
                row = row.child(kit::button(
                    SharedString::from(format!("cleanup-undo-{id}")),
                    if undoing { "Undoing…" } else { "Undo" },
                    Some(IconName::Undo2),
                    Tone::Secondary,
                    (can_undo && !confirming)
                        .then(|| kit::on(cx.listener(move |this, _, _, cx| this.ask_undo(id, cx))))
                        .flatten(),
                    cx,
                ));
            }
            list = list.child(row);
            if confirming {
                list = list.child(
                    h_flex()
                        .id("cleanup-undo-confirmation")
                        .test_support()
                        .w_full()
                        .gap(Tokens::space(2.0))
                        .p(Tokens::space(2.0))
                        .items_center()
                        .rounded(Tokens::RADIUS_MD)
                        .bg(colors.raised)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text)
                                .child(format!(
                                    "Move {} files back to their places on {}? A path that is in use now is never \
                                     overwritten; that file stays in quarantine.",
                                    format_count(r.quarantined),
                                    r.source_name
                                )),
                        )
                        .child(kit::button(
                            "cleanup-undo-cancel",
                            "Cancel",
                            None,
                            Tone::Ghost,
                            kit::on(cx.listener(|this, _, _, cx| {
                                this.undo_confirm = None;
                                cx.notify();
                            })),
                            cx,
                        ))
                        .child(kit::button(
                            "cleanup-undo-confirm",
                            format!("Put back {} files", format_count(r.quarantined)),
                            Some(IconName::Undo2),
                            Tone::Primary,
                            kit::on(cx.listener(|this, _, _, cx| this.undo(cx))),
                            cx,
                        )),
                );
            }
        }
        list.into_any_element()
    }
}

impl Render for CleanupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let mut root = v_flex().id("cleanup").w_full().gap(Tokens::space(4.0));
        if self.backend.is_none() || !self.has_content() {
            return root.into_any_element();
        }
        // Review, confirmation, progress or result.
        match self.phase.clone() {
            CleanupPhase::Planning(_) => {
                root = root.child(
                    kit::card(cx).id("cleanup-planning").test_support().child(
                        div()
                            .text_color(colors.text_muted)
                            .child("Planning the cleanup…"),
                    ),
                );
            }
            CleanupPhase::Review | CleanupPhase::Confirming => {
                if let Some(review) = self.review.clone() {
                    root = root.child(self.review_card(&review, cx));
                }
            }
            CleanupPhase::Running { progress, .. } => {
                let total = progress.total.load(Ordering::Relaxed);
                let done = progress.done.load(Ordering::Relaxed);
                let moved = progress.moved.load(Ordering::Relaxed);
                let skipped = progress.skipped.load(Ordering::Relaxed);
                let bytes = progress.bytes_moved.load(Ordering::Relaxed);
                root = root.child(
                    kit::card(cx)
                        .id("cleanup-progress")
                        .test_support()
                        .aria_label(SharedString::from(format!(
                            "{done} of {total} files checked"
                        )))
                        .child(
                            kit::card_title("Moving files to quarantine", cx)
                                .child(div().flex_1())
                                .child(kit::button(
                                    "cleanup-stop",
                                    "Stop",
                                    None,
                                    Tone::Secondary,
                                    kit::on(cx.listener(|this, _, _, cx| {
                                        this.stop();
                                        cx.notify();
                                    })),
                                    cx,
                                )),
                        )
                        .child(kit::bar(
                            if total == 0 {
                                0.0
                            } else {
                                done as f32 / total as f32
                            },
                            colors.accent,
                            cx,
                        ))
                        .child(
                            div()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "{} of {} checked · {} moved ({}) · {} left in place",
                                    format_count(done),
                                    format_count(total),
                                    format_count(moved),
                                    kit::format_bytes(bytes),
                                    format_count(skipped)
                                )),
                        ),
                );
            }
            CleanupPhase::Done(o) => {
                let mut card = kit::card(cx)
                    .id("cleanup-result")
                    .test_support()
                    .aria_label(SharedString::from(format!(
                        "{} moved, {} left in place",
                        o.moved,
                        o.skipped.len()
                    )))
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .text_color(colors.success)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::new(IconName::CircleCheck).size(Tokens::icon_sm()))
                            .child(format!(
                                "Moved {} files ({}) to quarantine in {}{}",
                                format_count(o.moved),
                                kit::format_bytes(o.bytes_moved),
                                kit::format_duration(o.elapsed_ms),
                                if o.cancelled {
                                    format!(
                                        "; stopped, {} still planned",
                                        format_count(o.not_attempted)
                                    )
                                } else {
                                    String::new()
                                }
                            )),
                    );
                if !o.skipped.is_empty() {
                    card = card.child(
                        div()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.warning)
                            .child(format!(
                                "{} files were left where they are:",
                                format_count(o.skipped.len() as u64)
                            )),
                    );
                    for (i, (path, why)) in o.skipped.iter().take(8).enumerate() {
                        card = card.child(
                            h_flex()
                                .id(SharedString::from(format!("cleanup-skipped-{i}")))
                                .test_support()
                                .aria_label(SharedString::from(format!("{path} — {why}")))
                                .w_full()
                                .gap(Tokens::space(3.0))
                                .child(mono(kit::middle_ellipsis(path, 70), colors.text))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(Tokens::text_sm())
                                        .text_color(colors.text_muted)
                                        .child(why.clone()),
                                ),
                        );
                    }
                }
                root = root.child(card);
            }
            CleanupPhase::Failed(e) => {
                root = root.child(
                    note_box(
                        "cleanup-error",
                        e,
                        colors.danger,
                        IconName::CircleAlert,
                        &colors,
                    )
                    .test_support(),
                );
            }
            CleanupPhase::Idle => {}
        }
        if let Some(sid) = self.nothing_on {
            root = root.child(
                note_box(
                    "cleanup-nothing",
                    format!(
                        "Nothing to clean up on source {}: no file there is exported or covered by an exported copy yet.",
                        sid.0
                    ),
                    colors.info,
                    IconName::Info,
                    &colors,
                )
                .test_support(),
            );
        }
        match &self.last_undo {
            Some(Ok(u)) => {
                let text = if u.conflicts.is_empty() {
                    format!(
                        "Put back {} files ({}){}.",
                        format_count(u.restored),
                        kit::format_bytes(u.bytes_restored),
                        if u.changed > 0 {
                            format!("; {} had changed while in quarantine", u.changed)
                        } else {
                            String::new()
                        }
                    )
                } else {
                    format!(
                        "Put back {} files; {} stay in quarantine because their place is in use: {}",
                        format_count(u.restored),
                        u.conflicts.len(),
                        u.conflicts
                            .iter()
                            .take(3)
                            .map(|(p, _)| p.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let tone = if u.conflicts.is_empty() {
                    colors.success
                } else {
                    colors.warning
                };
                root = root.child(
                    note_box(
                        "cleanup-undo-result",
                        text.clone(),
                        tone,
                        IconName::Undo2,
                        &colors,
                    )
                    .test_support()
                    .aria_label(SharedString::from(text)),
                );
            }
            Some(Err(e)) => {
                root = root.child(
                    note_box(
                        "cleanup-undo-error",
                        e.clone(),
                        colors.danger,
                        IconName::CircleAlert,
                        &colors,
                    )
                    .test_support(),
                );
            }
            None => {}
        }
        if let Some(runs) = self.runs.ready().filter(|r| !r.is_empty()) {
            let runs = runs.clone();
            root = root.child(
                kit::card(cx)
                    .child(kit::card_title("Cleanup runs", cx))
                    .child(
                        div()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child(
                                "Quarantined files wait in each drive's .photosquirrel-quarantine folder. Undo puts \
                                 a run back; emptying the quarantine is up to you, in your file manager.",
                            ),
                    )
                    .child(self.history(&runs, cx)),
            );
        }
        root.into_any_element()
    }
}
