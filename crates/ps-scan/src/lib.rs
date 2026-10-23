//! Resumable, incremental, threaded scan pipeline (ADR-0007, 0008, 0009).
//!
//! - [`fs`]: the read-only file layer every source access goes through (ADR-0004), with test
//!   instrumentation.
//! - [`source`]: volume identity, device class, the sampled content fingerprint, overlap (ADR-0006).
//! - [`walk`] + [`diff`]: list and stat a tree, merge-join it with the catalog (ADR-0007).
//! - [`lane`]: the adaptive per-source reader-count controller (ADR-0009).
//! - [`memory`]: the 512 MB byte budget with two buffers per reader.
//! - [`control`]: cooperative pause and cancel (ADR-0008).
//! - [`progress`]: atomic counters and the recent ring behind progress snapshots (ADR-0019).
//! - [`Scanner`]: the engine API the service facade wraps (sources, runs, control, snapshots).
//! - [`upgrade`]: perceptual-hash upgrades from the thumbnail store (ADR-0012).
//! - [`redate`]: re-dating unchanged rows from stored facts when the date rules change (ADR-0071).
//! - [`post`]: the post-scan stage, incremental duplicate regrouping persisted through the catalog writer.
//!
//! Threads, not async: one walk thread and one lane (1-16 readers) per source, bounded crossbeam queues, a
//! rayon pool of cores minus one for CPU work, one persist thread, and the catalog's single writer.

pub mod control;
pub mod diff;
mod error;
pub mod fs;
pub mod lane;
pub mod memory;
mod options;
mod pipeline;
pub mod post;
pub mod progress;
pub mod redate;
mod report;
mod scanner;
pub mod source;
pub mod upgrade;
pub mod walk;

pub use control::{Cancelled, Control, ControlState};
pub use error::{Result, ScanError};
pub use fs::{Instrumentation, SourceFs};
pub use options::{DeviceClassFn, ScanOptions, default_cpu_threads};
pub use pipeline::CHUNK;
pub use post::{GroupEngine, RegroupReport};
pub use report::{RunSummary, SourceSummary};
pub use scanner::{AddedSource, Confirmation, Scanner, SourceInfo};
pub use upgrade::{HashUpgradeReport, upgrade_hashes, upgrade_hashes_current};
