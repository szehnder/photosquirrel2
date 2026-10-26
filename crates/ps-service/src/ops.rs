//! One operation that changes files or their catalog records at a time (ADR-0065).
//!
//! A scan (started or resumed), an export, a cleanup plan, selection, discard, run or undo, cleanup
//! reconciliation and adding,
//! re-linking or removing a source exclude each other within a process: [`ScanService::begin`] takes the slot
//! for an export or a cleanup operation (refused while a scan is active or another operation holds it), and
//! the scan entry points refuse while the slot is held. Reconciliation never touches a run that is live in
//! this process, because a live run holds the slot. Across processes the catalog's owner lock (ADR-0005,
//! ADR-0042) already allows a single writer: every one of these operations needs it, observers are refused.
//!
//! Group, keeper and exclusion edits ([`ScanService::edit`]) are refused while a cleanup run holds the slot,
//! and a cleanup run does not start while one is being saved (ADR-0099): what makes a file safe to move does
//! not change under a run started from this process. The run also re-derives its holds when the catalog's
//! constraints generation moves on, for any write that does not come through here.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use ps_model::ScanRunId;

use crate::scan::ScanService;

/// What holds the operation slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Export,
    CleanupPlan,
    CleanupRun(i64),
    CleanupUndo(i64),
    /// Setting a planned run's selection (ADR-0068).
    CleanupSelect(i64),
    /// Discarding a planned run (ADR-0068).
    CleanupDiscard(i64),
    CleanupReconcile,
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Operation::Export => f.write_str("an export"),
            Operation::CleanupPlan => f.write_str("a cleanup plan"),
            Operation::CleanupRun(id) => write!(f, "cleanup run {id}"),
            Operation::CleanupUndo(id) => write!(f, "the undo of cleanup run {id}"),
            Operation::CleanupSelect(id) => write!(f, "the selection of cleanup run {id}"),
            Operation::CleanupDiscard(id) => write!(f, "the discard of cleanup run {id}"),
            Operation::CleanupReconcile => f.write_str("cleanup reconciliation"),
        }
    }
}

/// Why [`ScanService::begin`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Busy {
    #[error("scan run {0} is running: wait for it to finish (or cancel it) first")]
    Scan(ScanRunId),
    #[error("{0} is running: wait for it to finish first")]
    Operation(Operation),
    /// An edit was asked for while a cleanup run is moving files.
    #[error(
        "a cleanup is running (run {0}): keeper, group and exclusion changes wait until it ends, so nothing it \
         checked changes under it"
    )]
    Cleanup(i64),
    /// A cleanup run was asked for while an edit is being saved.
    #[error(
        "a keeper, group or exclusion change is being saved: start the cleanup again in a moment"
    )]
    Edit,
}

/// The slot (one per [`ScanService`]).
#[derive(Debug, Default)]
pub(crate) struct Ops {
    current: Mutex<Option<Operation>>,
    /// Edits being saved ([`EditGuard`]); changed under `current`'s lock.
    edits: AtomicUsize,
    /// The cleanup run holding the slot (0: none), read without the lock (UI renders).
    cleanup_run: AtomicI64,
}

impl Ops {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Option<Operation>> {
        self.current.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Holds the slot until dropped.
#[derive(Debug)]
#[must_use = "the operation slot is released when the guard is dropped"]
pub struct OpGuard<'a> {
    ops: &'a Ops,
    op: Operation,
}

impl OpGuard<'_> {
    pub fn operation(&self) -> Operation {
        self.op
    }
}

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        let mut cur = self.ops.lock();
        if matches!(self.op, Operation::CleanupRun(_)) {
            self.ops.cleanup_run.store(0, Ordering::SeqCst);
        }
        *cur = None;
    }
}

/// An edit that changes what makes a file safe to clean up is being saved ([`ScanService::edit`]).
#[derive(Debug)]
#[must_use = "the edit counts as in progress until the guard is dropped"]
pub struct EditGuard<'a> {
    ops: &'a Ops,
}

impl Drop for EditGuard<'_> {
    fn drop(&mut self) {
        self.ops.edits.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ScanService {
    /// Take the operation slot for `op`: refused while a scan is active or another operation holds it.
    pub fn begin(&self, op: Operation) -> Result<OpGuard<'_>, Busy> {
        let mut cur = self.ops().lock();
        if let Some(other) = *cur {
            return Err(Busy::Operation(other));
        }
        if let Some(run) = self.active() {
            return Err(Busy::Scan(run));
        }
        if let Operation::CleanupRun(id) = op {
            if self.ops().edits.load(Ordering::SeqCst) > 0 {
                return Err(Busy::Edit);
            }
            self.ops().cleanup_run.store(id.max(1), Ordering::SeqCst);
        }
        *cur = Some(op);
        Ok(OpGuard {
            ops: self.ops(),
            op,
        })
    }

    /// The operation holding the slot now, if any.
    pub fn current_operation(&self) -> Option<Operation> {
        *self.ops().lock()
    }

    /// The cleanup run moving files now, if any. Lock-free: cheap enough for a UI render.
    pub fn cleanup_running(&self) -> Option<i64> {
        Some(self.ops().cleanup_run.load(Ordering::SeqCst)).filter(|id| *id != 0)
    }

    /// Start an edit of keepers, groups or user decisions: refused with [`Busy::Cleanup`] while a cleanup run
    /// is moving files, and a run does not start until the returned guard is dropped (ADR-0099).
    pub fn edit(&self) -> Result<EditGuard<'_>, Busy> {
        let cur = self.ops().lock();
        if let Some(Operation::CleanupRun(id)) = *cur {
            return Err(Busy::Cleanup(id));
        }
        self.ops().edits.fetch_add(1, Ordering::SeqCst);
        drop(cur);
        Ok(EditGuard { ops: self.ops() })
    }
}
