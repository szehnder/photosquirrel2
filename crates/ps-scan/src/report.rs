//! The run summary stored as the run's `stats` JSON (per-stage totals, diff, incomplete sources).

use serde::{Deserialize, Serialize};

use ps_model::{DeviceClass, DiffCounts, RunStatus, ScanRunId, SourceId, StageCounts};

/// Per-source part of a [`RunSummary`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceSummary {
    pub source: SourceId,
    pub device: DeviceClass,
    pub files: u64,
    pub to_process: u64,
    pub processed: u64,
    pub bytes_read: u64,
    pub stages: StageCounts,
    pub diff: DiffCounts,
    /// The source was offline or went offline: its unfinished files keep their last stage.
    pub incomplete: bool,
    pub symlinks_skipped: u64,
    pub walk_errors: u64,
}

/// What a run did. Stored in `scan_run.stats`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run: ScanRunId,
    pub status: RunStatus,
    pub elapsed_ms: u64,
    /// Stage of every present file of the run's sources at the end.
    pub stages: StageCounts,
    pub diff: DiffCounts,
    pub sources: Vec<SourceSummary>,
    /// Sources that were or went offline during the run (ADR-0008: a field, not a run status).
    pub incomplete_sources: Vec<SourceId>,
    pub processed: u64,
    pub bytes_read: u64,
    pub errors: u64,
    /// Files whose fingerprint changed between the walk and the read; the next rescan picks them up.
    pub changed_during_scan: u64,
    /// Failed files not attempted any more (ADR-0008: at most two retries).
    pub retries_exhausted: u64,
    /// The post-scan regroup (absent for cancelled runs and runs stored before it existed).
    #[serde(default)]
    pub grouping: Option<crate::post::RegroupReport>,
}

impl RunSummary {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn from_json(v: &serde_json::Value) -> Option<RunSummary> {
        serde_json::from_value(v.clone()).ok()
    }
}
