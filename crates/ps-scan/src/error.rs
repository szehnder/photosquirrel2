use std::path::PathBuf;

use ps_model::{RunStatus, ScanRunId, SourceId};

/// Errors of the scan engine's control API (per-file problems never surface here: they are recorded in the
/// catalog as failed files and shown in progress snapshots).
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error(transparent)]
    Catalog(#[from] ps_catalog::CatalogError),

    #[error("folder {} does not exist", .0.display())]
    SourceMissing(PathBuf),

    #[error("{} is not a folder", .0.display())]
    NotAFolder(PathBuf),

    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{} is already a source (source {id})", path.display())]
    AlreadySource { path: PathBuf, id: SourceId },

    #[error("{} is already covered by source {by} ({})", path.display(), by_root.display())]
    AlreadyCovered {
        path: PathBuf,
        by: SourceId,
        by_root: PathBuf,
    },

    #[error("unknown source {0}")]
    NoSuchSource(SourceId),

    #[error("unknown scan run {0}")]
    NoSuchRun(ScanRunId),

    #[error("scan run {0} is still active")]
    RunActive(ScanRunId),

    #[error("no scan is running")]
    NoActiveRun,

    #[error("scan run {run} is {} and cannot be resumed", status.as_str())]
    NotResumable { run: ScanRunId, status: RunStatus },

    #[error(
        "{} does not hold this source's files: {mismatched} of {samples} sampled files do not match",
        path.display()
    )]
    FingerprintMismatch {
        path: PathBuf,
        mismatched: usize,
        samples: usize,
    },

    #[error("source {0} has no fingerprint to re-link with")]
    NoFingerprint(SourceId),

    #[error("removing a source needs explicit confirmation")]
    NotConfirmed,

    #[error("no source to scan")]
    NothingToScan,

    #[error("scan thread failed: {0}")]
    Thread(String),
}

pub type Result<T, E = ScanError> = std::result::Result<T, E>;
