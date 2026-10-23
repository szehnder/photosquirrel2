//! The Cleanup panel of the Reports view (changes `quarantine-cleanup`, `cleanup-selection`; ADR-0024,
//! ADR-0061, ADR-0068): review a plan (what moves where, how much space you can free) in a tree of folders the
//! user can tick, expand to files and untick, an explicit confirmation, live progress, and the history of runs
//! with Undo (whole runs or chosen folders and files) and Discard (plans that never moved a file).
//!
//! Nothing moves without the confirmation: "Move N files…" first stores the selection (catalog rows only),
//! then opens the confirmation with the stored counts, and only its own button starts the run. Undo and
//! Discard also ask first. Every call goes to the backend on a background thread; progress is sampled
//! (ADR-0019). The trees are virtualized ([`uniform_list`] over the visible rows only) and file names load per
//! folder when it is expanded, so a plan of a million files stays responsive.

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
    CleanupTreeKind, SharedScanBackend, UndoOutcome,
};
use crate::service::Loadable;
use crate::views::cleanup_tree::{Row, SelectionTree, Tick};
use crate::views::kit::{self, Tone};

/// Progress is sampled this often (ADR-0019).
const SAMPLE: Duration = Duration::from_millis(100);

/// Rows of a plan's move list shown before "and N more".
const MOVE_ROWS: usize = 3;

/// Height of one selection-tree row (default px; scaled by [`Tokens::size`]).
const TREE_ROW: f32 = 28.0;
/// Rows of a selection tree visible at once (the list scrolls beyond).
const TREE_VISIBLE_ROWS: usize = 12;

/// Which selection tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeOf {
    /// The reviewed plan's planned and unselected files (narrowing).
    Plan,
    /// The quarantined files of the run whose undo is being confirmed.
    Undo,
}

impl TreeOf {
    fn kind(self) -> CleanupTreeKind {
        match self {
            TreeOf::Plan => CleanupTreeKind::Plan,
            TreeOf::Undo => CleanupTreeKind::Undo,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            TreeOf::Plan => "cleanup-tree",
            TreeOf::Undo => "cleanup-undo-tree",
        }
    }
}

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
    /// The user clicked "Move N files…" after changing the selection: it is being stored.
    Selecting,
    /// The confirmation is shown (for the stored selection).
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
    /// The reviewed plan's folders with check boxes (loaded in the background).
    pub plan_tree: Option<SelectionTree>,
    plan_scroll: UniformListScrollHandle,
    /// The folders of the run whose undo is being confirmed.
    pub undo_tree: Option<SelectionTree>,
    undo_scroll: UniformListScrollHandle,
    /// Discard asked for this run; the confirmation is shown.
    pub discard_confirm: Option<i64>,
    pub discarding: Option<i64>,
    /// Why the last selection, discard or tree load was refused.
    pub note: Option<String>,
    task: Option<Task<()>>,
    run_task: Option<Task<()>>,
    tree_task: Option<Task<()>>,
    undo_tree_task: Option<Task<()>>,
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
            plan_tree: None,
            plan_scroll: UniformListScrollHandle::new(),
            undo_tree: None,
            undo_scroll: UniformListScrollHandle::new(),
            discard_confirm: None,
            discarding: None,
            note: None,
            task: None,
            run_task: None,
            tree_task: None,
            undo_tree_task: None,
        };
        // Shown at start-up: the history (and the latest plan) are in frame one when quick (start-up grace).
        let early = this.backend.clone().and_then(|b| {
            let deadline = crate::startup::StartupGrace::deadline(cx)?;
            crate::startup::StartupGrace::within(deadline, move || load(&b, true))
        });
        match early {
            Some(r) => this.loaded(r, cx),
            None => this.refresh(cx),
        }
        this
    }

    fn loaded(&mut self, r: anyhow::Result<Loaded>, cx: &mut Context<'_, Self>) {
        match r {
            Ok((runs, review, tree)) => {
                if let (Some(t), Some(r)) = (tree, review.as_ref())
                    && t.run == r.run.id
                    && self.plan_tree.is_none()
                {
                    self.plan_tree = Some(SelectionTree::new(&t));
                }
                self.runs = Loadable::Ready(runs);
                // Show the newest planned run for review unless something else is on screen.
                if matches!(self.phase, CleanupPhase::Idle | CleanupPhase::Review) {
                    self.phase = if review.is_some() {
                        CleanupPhase::Review
                    } else {
                        CleanupPhase::Idle
                    };
                    self.set_review(review, cx);
                }
            }
            Err(e) => self.runs = Loadable::Failed(format!("{e:#}")),
        }
    }

    /// Show `review` (or none) and load its selection tree when it is another run's.
    fn set_review(&mut self, review: Option<CleanupReview>, cx: &mut Context<'_, Self>) {
        let run = review.as_ref().map(|r| r.run.id);
        self.review = review;
        match run {
            Some(id) if self.plan_tree.as_ref().is_some_and(|t| t.run == id) => {}
            Some(id) => {
                self.plan_tree = None;
                self.load_tree(TreeOf::Plan, id, cx);
            }
            None => {
                self.plan_tree = None;
                self.tree_task = None;
            }
        }
    }

    /// Load a run's selection tree on a background thread.
    fn load_tree(&mut self, which: TreeOf, run: i64, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let kind = which.kind();
        let reply = cx.background_spawn(async move { b.cleanup_tree(run, kind) });
        let task = cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(tree) => {
                        let t = Some(SelectionTree::new(&tree));
                        match which {
                            TreeOf::Plan
                                if this.review.as_ref().is_some_and(|r| r.run.id == run) =>
                            {
                                this.plan_tree = t;
                            }
                            TreeOf::Undo if this.undo_confirm == Some(run) => this.undo_tree = t,
                            _ => {}
                        }
                    }
                    Err(e) => this.note = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        });
        match which {
            TreeOf::Plan => self.tree_task = Some(task),
            TreeOf::Undo => self.undo_tree_task = Some(task),
        }
    }

    fn tree_mut(&mut self, which: TreeOf) -> Option<&mut SelectionTree> {
        match which {
            TreeOf::Plan => self.plan_tree.as_mut(),
            TreeOf::Undo => self.undo_tree.as_mut(),
        }
    }

    fn tree(&self, which: TreeOf) -> Option<&SelectionTree> {
        match which {
            TreeOf::Plan => self.plan_tree.as_ref(),
            TreeOf::Undo => self.undo_tree.as_ref(),
        }
    }

    /// The tree's check boxes can be changed now (not while a selection is stored, a run or undo is going,
    /// or the confirmation shows the stored counts).
    pub fn tree_editable(&self, which: TreeOf) -> bool {
        match which {
            TreeOf::Plan => matches!(self.phase, CleanupPhase::Review) && self.discarding.is_none(),
            TreeOf::Undo => self.undo_confirm.is_some() && self.undoing.is_none(),
        }
    }

    /// Tick or untick a row of a tree (a folder with everything below it, or one file).
    pub fn toggle_row(&mut self, which: TreeOf, ix: usize, cx: &mut Context<'_, Self>) {
        if !self.tree_editable(which) {
            return;
        }
        let Some(t) = self.tree_mut(which) else {
            return;
        };
        match t.row(ix).cloned() {
            Some(Row::Folder(n)) => t.toggle_folder(n),
            Some(Row::File(n, k)) => t.toggle_file(n, k),
            _ => return,
        }
        cx.notify();
    }

    /// Tick or untick everything in a tree.
    pub fn toggle_all(&mut self, which: TreeOf, cx: &mut Context<'_, Self>) {
        if !self.tree_editable(which) {
            return;
        }
        if let Some(t) = self.tree_mut(which) {
            t.toggle_all();
            cx.notify();
        }
    }

    /// Expand or collapse a folder row; its file names load in the background the first time.
    pub fn expand_row(&mut self, which: TreeOf, ix: usize, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let Some(t) = self.tree_mut(which) else {
            return;
        };
        let Some(Row::Folder(n)) = t.row(ix).cloned() else {
            return;
        };
        let (run, kind) = (t.run, t.kind);
        let need = t.toggle_expand(n);
        cx.notify();
        let Some(folder) = need else {
            return;
        };
        let reply = cx.background_spawn(async move { b.cleanup_folder_files(run, folder, kind) });
        cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(t) = this.tree_mut(which).filter(|t| t.run == run) {
                    match r {
                        Ok(rows) => t.set_names(n, &rows),
                        Err(e) => {
                            t.names_failed(n);
                            this.note = Some(format!("{e:#}"));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scroll a tree so that row `ix` is at the top.
    pub fn scroll_tree_to(&self, which: TreeOf, ix: usize) {
        match which {
            TreeOf::Plan => self.plan_scroll.scroll_to_item(ix, ScrollStrategy::Top),
            TreeOf::Undo => self.undo_scroll.scroll_to_item(ix, ScrollStrategy::Top),
        }
    }

    /// Files and bytes the reviewed plan would move now: the tree's picks once it is loaded, else the stored
    /// plan's.
    pub fn picked(&self) -> (u64, u64) {
        match (&self.plan_tree, &self.review) {
            (Some(t), _) => t.picked(),
            (None, Some(r)) => (r.run.planned, r.run.planned_bytes),
            _ => (0, 0),
        }
    }

    /// Reload the history (and the newest planned run).
    pub fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let reply = cx.background_spawn(async move { load(&b, false) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.loaded(r, cx);
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
            CleanupPhase::Running { .. } | CleanupPhase::Planning(_) | CleanupPhase::Selecting
        ) || self.undoing.is_some()
            || self.discarding.is_some()
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
        self.note = None;
        self.discard_confirm = None;
        cx.notify();
        let reply = cx.background_spawn(async move { b.cleanup_plan(source) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(Some(review)) => {
                        this.set_review(Some(review), cx);
                        this.phase = CleanupPhase::Review;
                    }
                    Ok(None) => {
                        this.set_review(None, cx);
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
                        this.set_review(Some(review), cx);
                        this.network_ack = false;
                        this.note = None;
                        this.phase = CleanupPhase::Review;
                    }
                    Err(e) => this.phase = CleanupPhase::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        }));
    }

    /// "Move N files…": store the tree's selection when it changed (catalog rows only, on a background
    /// thread), then show the confirmation with the stored counts. Moves nothing.
    pub fn ask(&mut self, cx: &mut Context<'_, Self>) {
        if !matches!(self.phase, CleanupPhase::Review) || self.picked().0 == 0 {
            return;
        }
        let Some(review) = self.review.clone() else {
            return;
        };
        let changed = self.plan_tree.as_ref().filter(|t| t.changed());
        let (Some(t), Some(b)) = (changed, self.backend.clone()) else {
            if review.run.planned > 0 {
                self.phase = CleanupPhase::Confirming;
                cx.notify();
            }
            return;
        };
        let selection = t.selection();
        let run = review.run.id;
        self.phase = CleanupPhase::Selecting;
        self.note = None;
        cx.notify();
        let reply = cx.background_spawn(async move { b.cleanup_select(run, selection) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(stored) => {
                        // The confirmation shows what the catalog holds now.
                        this.review = Some(stored);
                        if let Some(t) = this.plan_tree.as_mut() {
                            t.mark_stored();
                        }
                        this.phase = CleanupPhase::Confirming;
                    }
                    Err(e) => {
                        this.note = Some(format!("{e:#}"));
                        this.phase = CleanupPhase::Review;
                    }
                }
                this.refresh(cx);
                cx.notify();
            });
        }));
    }

    /// Back from the confirmation, or close the plan.
    pub fn back(&mut self, cx: &mut Context<'_, Self>) {
        self.phase = match self.phase {
            CleanupPhase::Confirming => CleanupPhase::Review,
            CleanupPhase::Running { .. } => return,
            CleanupPhase::Selecting => return,
            _ => {
                self.set_review(None, cx);
                self.note = None;
                self.discard_confirm = None;
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
                            this.set_review(None, cx);
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

    /// "Undo…": ask first, with the run's quarantined folders in a tree (all ticked).
    pub fn ask_undo(&mut self, run: i64, cx: &mut Context<'_, Self>) {
        if !self.busy() {
            self.undo_confirm = Some(run);
            self.undo_tree = None;
            self.discard_confirm = None;
            self.load_tree(TreeOf::Undo, run, cx);
            cx.notify();
        }
    }

    /// Close the undo confirmation.
    pub fn cancel_undo(&mut self, cx: &mut Context<'_, Self>) {
        self.undo_confirm = None;
        self.undo_tree = None;
        self.undo_tree_task = None;
        cx.notify();
    }

    /// Undo the run whose confirmation is shown: the quarantined files its tree picks go back to their places.
    pub fn undo(&mut self, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let Some(run) = self.undo_confirm else {
            return;
        };
        let Some(selection) = self
            .undo_tree
            .as_ref()
            .filter(|t| t.run == run && t.picked().0 > 0)
            .map(SelectionTree::selection)
        else {
            return;
        };
        self.undo_confirm = None;
        self.undo_tree = None;
        self.undoing = Some(run);
        self.last_undo = None;
        cx.notify();
        let slot: Slot<UndoOutcome> = Arc::new(Mutex::new(None));
        {
            let slot = slot.clone();
            let spawned = std::thread::Builder::new()
                .name("cleanup-undo".into())
                .spawn(move || {
                    let r = b.cleanup_undo(run, selection);
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

impl CleanupView {
    /// "Discard": ask first (nothing on the drive changes either way).
    pub fn ask_discard(&mut self, run: i64, cx: &mut Context<'_, Self>) {
        if !self.busy() {
            self.discard_confirm = Some(run);
            self.undo_confirm = None;
            cx.notify();
        }
    }

    pub fn cancel_discard(&mut self, cx: &mut Context<'_, Self>) {
        self.discard_confirm = None;
        cx.notify();
    }

    /// Discard the plan whose confirmation is shown: catalog rows only, on a background thread.
    pub fn discard(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(b), Some(run)) = (self.backend.clone(), self.discard_confirm.take()) else {
            return;
        };
        if self.busy() {
            return;
        }
        self.discarding = Some(run);
        self.note = None;
        cx.notify();
        let reply = cx.background_spawn(async move { b.cleanup_discard(run) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.discarding = None;
                match r {
                    Ok(_) => {
                        if this.review.as_ref().is_some_and(|r| r.run.id == run) {
                            this.set_review(None, cx);
                            if matches!(this.phase, CleanupPhase::Review | CleanupPhase::Confirming)
                            {
                                this.phase = CleanupPhase::Idle;
                            }
                        }
                    }
                    Err(e) => this.note = Some(format!("{e:#}")),
                }
                this.refresh(cx);
                cx.notify();
            });
        }));
    }

    /// A run that can be discarded: planned, never moved a file.
    fn discardable(r: &CleanupRunRow) -> bool {
        r.state == CleanupRunState::Planned
            && r.quarantined == 0
            && r.restored == 0
            && r.planned + r.unselected > 0
    }
}

/// The history and the newest planned run's review.
/// The history, the newest planned run's review and, with `with_tree` (at start-up, so the first frame shows
/// it), that run's selection tree.
fn load(b: &SharedScanBackend, with_tree: bool) -> anyhow::Result<Loaded> {
    let runs = b.cleanup_runs()?;
    let newest = runs
        .iter()
        .find(|r| r.state == CleanupRunState::Planned && r.planned > 0)
        .map(|r| r.id);
    let review = newest.map(|id| b.cleanup_review(id)).transpose()?;
    let tree = match newest {
        Some(id) if with_tree => Some(b.cleanup_tree(id, CleanupTreeKind::Plan)?),
        _ => None,
    };
    Ok((runs, review, tree))
}

type Loaded = (
    Vec<CleanupRunRow>,
    Option<CleanupReview>,
    Option<crate::scan_backend::CleanupTree>,
);

fn state_label(r: &CleanupRunRow) -> &'static str {
    match r.state {
        CleanupRunState::Planned if r.quarantined > 0 => "Partly run",
        CleanupRunState::Planned => "Planned",
        CleanupRunState::Running => "Running",
        CleanupRunState::Completed => "Quarantined",
        CleanupRunState::Undone => "Undone",
        CleanupRunState::Discarded => "Discarded",
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
        let (picked, picked_bytes) = self.picked();
        let folders_n = self
            .plan_tree
            .as_ref()
            .map_or(review.folders.len(), SelectionTree::picked_folder_count);
        let mut card = kit::card(cx)
            .id("cleanup-plan")
            .test_support()
            .aria_label(SharedString::from(format!(
                "Cleanup plan {}: {} files, {}",
                r.id,
                picked,
                kit::format_bytes(picked_bytes)
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
                        format_count(picked),
                        cx,
                    ))
                    .child(kit::stat(
                        "cleanup-bytes",
                        "Space you can free",
                        kit::format_bytes(picked_bytes),
                        cx,
                    ))
                    .child(kit::stat(
                        "cleanup-folders",
                        "Folders",
                        format_count(folders_n as u64),
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
                         puts everything back. The space is freed only once you delete that folder yourself. \
                         Untick folders or files to keep them where they are.",
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

        // The selection tree: folders (and, expanded, files) with check boxes.
        if let Some(note) = self.note.clone() {
            card = card.child(
                note_box(
                    "cleanup-note",
                    note,
                    colors.warning,
                    IconName::TriangleAlert,
                    &colors,
                )
                .test_support(),
            );
        }
        card = card.child(self.tree_box(TreeOf::Plan, cx));

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
        if r.planned > MOVE_ROWS as u64 {
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
                    .aria_label(SharedString::from(format!(
                        "Move {} files ({}) out of their folders on {}?",
                        format_count(r.planned),
                        kit::format_bytes(r.planned_bytes),
                        r.source_name
                    )))
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
            _ if self.discard_confirm == Some(r.id) => {
                card = card.child(self.discard_confirmation(r, cx));
            }
            _ => {
                let selecting = matches!(self.phase, CleanupPhase::Selecting);
                let can_move = picked > 0 && !selecting && self.discarding.is_none();
                let move_label = if selecting {
                    "Saving the selection…".to_owned()
                } else if picked == 0 {
                    "Nothing selected".to_owned()
                } else {
                    format!("Move {} files…", format_count(picked))
                };
                card = card.child(
                    h_flex()
                        .w_full()
                        .gap(Tokens::space(2.0))
                        .items_center()
                        .child(kit::button(
                            "cleanup-discard",
                            "Discard plan",
                            Some(IconName::X),
                            Tone::Ghost,
                            (Self::discardable(r) && !self.busy())
                                .then(|| {
                                    let id = r.id;
                                    kit::on(
                                        cx.listener(move |this, _, _, cx| this.ask_discard(id, cx)),
                                    )
                                })
                                .flatten(),
                            cx,
                        ))
                        .child(div().flex_1())
                        .child(kit::button(
                            "cleanup-close",
                            "Not now",
                            None,
                            Tone::Ghost,
                            (!selecting)
                                .then(|| kit::on(cx.listener(|this, _, _, cx| this.back(cx))))
                                .flatten(),
                            cx,
                        ))
                        .child(kit::button(
                            "cleanup-confirm",
                            move_label,
                            Some(IconName::ArrowRight),
                            Tone::Primary,
                            can_move
                                .then(|| kit::on(cx.listener(|this, _, _, cx| this.ask(cx))))
                                .flatten(),
                            cx,
                        )),
                );
            }
        }
        card.into_any_element()
    }

    fn discard_confirmation(&self, r: &CleanupRunRow, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        h_flex()
            .id("cleanup-discard-confirmation")
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
                        "Discard plan {}? Nothing on the drive changes: its {} files stay where they are, and the \
                         plan stays in the history as discarded.",
                        r.id,
                        format_count(r.planned + r.unselected)
                    )),
            )
            .child(kit::button(
                "cleanup-discard-cancel",
                "Cancel",
                None,
                Tone::Ghost,
                kit::on(cx.listener(|this, _, _, cx| this.cancel_discard(cx))),
                cx,
            ))
            .child(kit::button(
                "cleanup-discard-confirm",
                "Discard plan",
                Some(IconName::X),
                Tone::Secondary,
                kit::on(cx.listener(|this, _, _, cx| this.discard(cx))),
                cx,
            ))
            .into_any_element()
    }

    fn undo_confirmation(&self, r: &CleanupRunRow, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let tree = self.undo_tree.as_ref().filter(|t| t.run == r.id);
        let picked = tree.map(|t| t.picked());
        let n = picked.map_or(r.quarantined, |p| p.0);
        let ready = picked.is_some_and(|p| p.0 > 0);
        let label = match picked {
            None => "Loading…".to_owned(),
            Some((0, _)) => "Nothing selected".to_owned(),
            Some((n, _)) => format!("Put back {} files", format_count(n)),
        };
        v_flex()
            .id("cleanup-undo-confirmation")
            .test_support()
            .w_full()
            .gap(Tokens::space(2.0))
            .p(Tokens::space(2.0))
            .rounded(Tokens::RADIUS_MD)
            .bg(colors.raised)
            .child(
                div()
                    .w_full()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text)
                    .child(format!(
                        "Move {} files back to their places on {}? Untick folders or files to leave them in \
                         quarantine. A path that is in use now is never overwritten; that file stays in quarantine.",
                        format_count(n),
                        r.source_name
                    )),
            )
            .child(self.tree_box(TreeOf::Undo, cx))
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .justify_end()
                    .child(kit::button(
                        "cleanup-undo-cancel",
                        "Cancel",
                        None,
                        Tone::Ghost,
                        kit::on(cx.listener(|this, _, _, cx| this.cancel_undo(cx))),
                        cx,
                    ))
                    .child(kit::button(
                        "cleanup-undo-confirm",
                        label,
                        Some(IconName::Undo2),
                        Tone::Primary,
                        ready
                            .then(|| kit::on(cx.listener(|this, _, _, cx| this.undo(cx))))
                            .flatten(),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn history(&self, runs: &[CleanupRunRow], cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let mut list = v_flex().id("cleanup-history").test_support().w_full();
        let reviewed = self.review.as_ref().map(|r| r.run.id);
        for r in runs.iter().take(20) {
            let id = r.id;
            let confirming = self.undo_confirm == Some(id);
            let undoing = self.undoing == Some(id);
            let discarding = self.discarding == Some(id);
            let can_undo = r.quarantined > 0 && !self.busy();
            let mut summary = format!(
                "{} · {} quarantined ({}) · {} skipped · {} restored",
                state_label(r),
                format_count(r.quarantined),
                kit::format_bytes(r.quarantined_bytes),
                format_count(r.skipped),
                format_count(r.restored)
            );
            if r.unselected > 0 && r.state != CleanupRunState::Discarded {
                summary.push_str(&format!(" · {} left out", format_count(r.unselected)));
            }
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
            if r.planned > 0
                && !matches!(
                    r.state,
                    CleanupRunState::Undone | CleanupRunState::Discarded
                )
            {
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
            if Self::discardable(r) {
                row = row.child(kit::button(
                    SharedString::from(format!("cleanup-discard-{id}")),
                    if discarding {
                        "Discarding…"
                    } else {
                        "Discard"
                    },
                    None,
                    Tone::Ghost,
                    (!self.busy() && self.discard_confirm != Some(id))
                        .then(|| {
                            kit::on(cx.listener(move |this, _, _, cx| this.ask_discard(id, cx)))
                        })
                        .flatten(),
                    cx,
                ));
            }
            if r.quarantined > 0 {
                row = row.child(kit::button(
                    SharedString::from(format!("cleanup-undo-{id}")),
                    if undoing { "Undoing…" } else { "Undo…" },
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
                list = list.child(self.undo_confirmation(r, cx));
            }
            // The review card asks for its own plan; other plans ask here.
            if self.discard_confirm == Some(id) && reviewed != Some(id) {
                list = list.child(self.discard_confirmation(r, cx));
            }
        }
        list.into_any_element()
    }

    /// A selection tree: a header with a check box for everything, then the visible rows in a virtualized
    /// list (only the rows on screen are built).
    fn tree_box(&self, which: TreeOf, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let prefix = which.prefix();
        let Some(tree) = self.tree(which) else {
            return div()
                .id(SharedString::from(format!("{prefix}-loading")))
                .test_support()
                .py(Tokens::space(1.0))
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child("Loading folders…")
                .into_any_element();
        };
        let editable = self.tree_editable(which);
        let n = tree.rows().len();
        let (picked, picked_bytes) = tree.picked();
        let (total, _) = tree.total();
        let header = h_flex()
            .w_full()
            .gap(Tokens::space(2.0))
            .py(Tokens::space(1.0))
            .items_center()
            .border_b_1()
            .border_color(colors.border)
            .text_size(Tokens::text_2xs())
            .text_color(colors.text_faint)
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-all")))
                    .test_support()
                    .role(Role::CheckBox)
                    .aria_label(SharedString::from(format!(
                        "{} of {} files selected ({})",
                        picked,
                        total,
                        kit::format_bytes(picked_bytes)
                    )))
                    .when(editable, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_all(which, cx)))
                    })
                    .child(check_box(tree.all_tick(), editable, &colors)),
            )
            .child(div().w(Tokens::size(16.0)).flex_none())
            .child(div().flex_1().child("FOLDER"))
            .child(div().w(Tokens::size(96.0)).text_right().child("FILES"))
            .child(div().w(Tokens::size(96.0)).text_right().child("SIZE"));
        let list = uniform_list(
            SharedString::from(format!("{prefix}-list")),
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                this.tree_rows(which, range, cx)
            }),
        )
        .track_scroll(match which {
            TreeOf::Plan => &self.plan_scroll,
            TreeOf::Undo => &self.undo_scroll,
        })
        .w_full()
        .h(Tokens::size(
            TREE_ROW * n.clamp(1, TREE_VISIBLE_ROWS) as f32,
        ));
        v_flex()
            .id(SharedString::from(prefix))
            .test_support()
            .aria_label(SharedString::from(format!(
                "{} folders, {} rows",
                tree.folder_count(),
                n
            )))
            .w_full()
            .child(header)
            .child(list)
            .into_any_element()
    }

    /// The rows `range` of a tree (called by its [`uniform_list`] for the rows on screen only).
    fn tree_rows(
        &mut self,
        which: TreeOf,
        range: std::ops::Range<usize>,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let colors = *ui_theme::colors(cx);
        let editable = self.tree_editable(which);
        let prefix = which.prefix();
        let Some(tree) = self.tree(which) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(range.len());
        for ix in range {
            let Some(row) = tree.row(ix).cloned() else {
                continue;
            };
            let base = h_flex()
                .id(SharedString::from(format!("{prefix}-row-{ix}")))
                .test_support()
                .w_full()
                .h(Tokens::size(TREE_ROW))
                .gap(Tokens::space(2.0))
                .items_center()
                .border_b_1()
                .border_color(colors.border);
            let indent = |depth: usize| div().flex_none().w(Tokens::space(4.0 * depth as f32));
            let el =
                match row {
                    Row::Folder(n) => {
                        let f = tree.folder(n);
                        let state = match f.tick {
                            Tick::All => "selected",
                            Tick::Some => "partly selected",
                            Tick::None => "not selected",
                        };
                        base.aria_label(SharedString::from(format!(
                            "{}: {} of {} files, {}",
                            f.path, f.picked, f.files, state
                        )))
                        .child(
                            div()
                                .id(SharedString::from(format!("{prefix}-check-{ix}")))
                                .test_support()
                                .role(Role::CheckBox)
                                .aria_label(SharedString::from(format!("{} {state}", f.path)))
                                .when(editable, |d| {
                                    d.cursor_pointer().on_click(cx.listener(
                                        move |this, _, _, cx| this.toggle_row(which, ix, cx),
                                    ))
                                })
                                .child(check_box(f.tick, editable, &colors)),
                        )
                        .child(indent(f.depth))
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("{prefix}-expand-{ix}")))
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .gap(Tokens::space(1.0))
                                .items_center()
                                .when(f.expandable, |d| {
                                    d.cursor_pointer().on_click(cx.listener(
                                        move |this, _, _, cx| this.expand_row(which, ix, cx),
                                    ))
                                })
                                .child(
                                    Icon::new(if f.expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .size(Tokens::icon_sm())
                                    .text_color(
                                        if f.expandable {
                                            colors.text_faint
                                        } else {
                                            colors.border
                                        },
                                    ),
                                )
                                .child(
                                    Icon::new(IconName::Folder)
                                        .size(Tokens::icon_sm())
                                        .text_color(colors.text_muted),
                                )
                                .child(mono(
                                    f.name.clone(),
                                    if f.tick == Tick::None {
                                        colors.text_faint
                                    } else {
                                        colors.text
                                    },
                                )),
                        )
                        .child(num_cell(
                            if f.tick == Tick::Some {
                                format!("{} / {}", format_count(f.picked), format_count(f.files))
                            } else {
                                format_count(f.files)
                            },
                            colors.text,
                        ))
                        .child(num_cell(
                            kit::format_bytes(if f.tick == Tick::None {
                                f.bytes
                            } else {
                                f.picked_bytes
                            }),
                            colors.text_muted,
                        ))
                    }
                    Row::File(n, k) => {
                        let f = tree.file(n, k);
                        let tick = if f.picked { Tick::All } else { Tick::None };
                        base.aria_label(SharedString::from(format!(
                            "{}: {}",
                            f.name,
                            if f.picked { "selected" } else { "not selected" }
                        )))
                        .child(
                            div()
                                .id(SharedString::from(format!("{prefix}-check-{ix}")))
                                .test_support()
                                .role(Role::CheckBox)
                                .when(editable, |d| {
                                    d.cursor_pointer().on_click(cx.listener(
                                        move |this, _, _, cx| this.toggle_row(which, ix, cx),
                                    ))
                                })
                                .child(check_box(tick, editable, &colors)),
                        )
                        .child(indent(f.depth))
                        .child(div().w(Tokens::size(16.0)).flex_none())
                        .child(mono(
                            f.name.clone(),
                            if f.picked {
                                colors.text
                            } else {
                                colors.text_faint
                            },
                        ))
                        .child(num_cell(String::new(), colors.text))
                        .child(num_cell(kit::format_bytes(f.size), colors.text_muted))
                    }
                    Row::Loading(n) => {
                        let depth = tree.folder(n).depth + 1;
                        base.child(div().w(Tokens::size(16.0)).flex_none())
                            .child(indent(depth))
                            .child(
                                div()
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_faint)
                                    .child("Loading files…"),
                            )
                    }
                };
            out.push(el.into_any_element());
        }
        out
    }
}

/// A tri-state check box (ticked, partly ticked, empty).
fn check_box(tick: Tick, enabled: bool, colors: &Colors) -> Div {
    let on = tick != Tick::None;
    div()
        .size(Tokens::size(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(Tokens::RADIUS_SM)
        .border_1()
        .border_color(if on {
            colors.accent
        } else {
            colors.border_strong
        })
        .when(on, |d| d.bg(colors.accent))
        .when(!enabled, |d| d.opacity(0.6))
        .when(on, |d| {
            d.child(
                Icon::new(if tick == Tick::All {
                    IconName::Check
                } else {
                    IconName::Minus
                })
                .size(Tokens::icon_sm())
                .text_color(colors.on_accent),
            )
        })
}

fn num_cell(text: String, color: Hsla) -> Div {
    div()
        .w(Tokens::size(96.0))
        .flex_none()
        .text_right()
        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
        .text_size(Tokens::text_mono())
        .text_color(color)
        .child(text)
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
            CleanupPhase::Review | CleanupPhase::Confirming | CleanupPhase::Selecting => {
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
