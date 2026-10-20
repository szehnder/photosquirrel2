//! Live export progress for the Export view (change `export`, task 6.1).
//!
//! [`Exports::run`](crate::export::Exports::run) blocks until the export ends. The UI needs a sampled view of
//! it (ADR-0019): [`ExportProgress`] is a set of atomic counters that the copier's [`CopyHooks`] update
//! (bytes as each chunk is written, files as each verified copy is renamed into place), and that the UI reads
//! with [`ExportProgress::snapshot`] at most every 100 ms. The hooks never change what is written.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ps_export::{CopyHooks, NoHooks};
use ps_model::FileId;
use serde::Serialize;
use stamp::Stamp;

use crate::export::{CatalogLedger, Result};

use crate::scan::ScanService;
/// Export types the UI names (so UI crates need only the facade, ADR-0027).
pub use ps_export::{
    Badge, CompletionReport, CopyCheck, DateTotals, ExecConfig, ExportReport, FileDisposition,
    Plan, PlanBlocker, ProfileSettings, StopReason, copy_check_text,
};

/// Atomic counters of a running export.
#[derive(Debug)]
pub struct ExportProgress {
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    files_total: AtomicU64,
    bytes_total: AtomicU64,
    started: Stamp,
}

impl Default for ExportProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// A point-in-time copy of [`ExportProgress`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct ExportSnapshot {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub elapsed_ms: u64,
}

impl ExportSnapshot {
    /// Average bytes per second since the export started.
    pub fn bytes_per_s(&self) -> f64 {
        if self.elapsed_ms == 0 {
            0.0
        } else {
            self.bytes_done as f64 * 1000.0 / self.elapsed_ms as f64
        }
    }

    /// Fraction of bytes copied (0..=1).
    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            if self.files_total == 0 {
                0.0
            } else {
                (self.files_done as f32 / self.files_total as f32).min(1.0)
            }
        } else {
            (self.bytes_done as f32 / self.bytes_total as f32).min(1.0)
        }
    }
}

impl ExportProgress {
    pub fn new() -> Self {
        Self {
            files_done: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            started: Stamp::now(),
        }
    }

    /// Set the totals from the plan being run.
    pub fn begin(&self, plan: &Plan) {
        self.files_total.store(plan.totals.files, Ordering::Relaxed);
        self.bytes_total.store(plan.totals.bytes, Ordering::Relaxed);
        self.started.reset();
    }

    pub fn snapshot(&self) -> ExportSnapshot {
        ExportSnapshot {
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            elapsed_ms: self.started.elapsed_ms(),
        }
    }
}

impl CopyHooks for ExportProgress {
    fn before_write(&self, file: FileId, offset: u64, chunk: &mut [u8]) -> std::io::Result<()> {
        NoHooks.before_write(file, offset, chunk)?;
        self.bytes_done
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn before_rename(&self, _file: FileId, _partial: &Path) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }
}

impl ScanService {
    /// [`Exports::run`](crate::export::Exports::run) reporting progress into `progress` (sampled by the UI).
    /// Same plan, same ledger, same copy-verify-rename protocol; `cancel` stops after the current files and
    /// a re-run resumes.
    pub fn export_with_progress(
        &self,
        library: &Path,
        settings: ProfileSettings,
        cfg: &ExecConfig,
        cancel: &AtomicBool,
        progress: &ExportProgress,
    ) -> Result<(Plan, ExportReport)> {
        let ex = self.exports();
        let (plan, input) = ex.plan(library, settings)?;
        progress.begin(&plan);
        let id = ex.library_id(library)?;
        let mut ledger = CatalogLedger::new(self.engine().catalog().clone(), id);
        let report = ps_export::execute(&plan, &input, &mut ledger, cfg, progress, cancel)?;
        Ok((plan, report))
    }
}

mod stamp {
    //! A resettable start time without locks (milliseconds since a fixed origin).
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    fn origin() -> Instant {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        *ORIGIN.get_or_init(Instant::now)
    }

    #[derive(Debug)]
    pub struct Stamp(AtomicU64);

    impl Stamp {
        pub fn now() -> Self {
            Self(AtomicU64::new(origin().elapsed().as_millis() as u64))
        }
        pub fn reset(&self) {
            self.0
                .store(origin().elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        pub fn elapsed_ms(&self) -> u64 {
            (origin().elapsed().as_millis() as u64).saturating_sub(self.0.load(Ordering::Relaxed))
        }
    }
}
