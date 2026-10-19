//! Engine options.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ps_model::{ClassifierSettings, DeviceClass};

use crate::fs::SourceFs;
use crate::lane::{MAX_READERS, WINDOW};
use crate::memory::DEFAULT_MEMORY_CAP;

/// Decides a source root's device class (default: [`crate::source::volume_info`]).
pub type DeviceClassFn = Arc<dyn Fn(&Path) -> DeviceClass + Send + Sync>;

/// How the engine scans.
#[derive(Clone)]
pub struct ScanOptions {
    /// Classifier thresholds (ADR-0015, from settings).
    pub classifier: ClassifierSettings,
    /// Video poster-frame tool (ffmpeg). `None`: videos get placeholders. The tool opens source files itself
    /// (read-only) and is outside the instrumented file layer.
    pub poster_tool: Option<PathBuf>,
    /// CPU workers: by default the cores minus one, leaving one for the UI (ADR-0009).
    pub cpu_threads: usize,
    /// Global cap on file buffers in flight (512 MB).
    pub memory_cap: u64,
    /// Upper bound of a lane's readers (16).
    pub max_readers: usize,
    /// Lane controller measurement window (2 s).
    pub lane_window: Duration,
    /// Override device-class detection (tests, or a user's per-source choice).
    pub device_class: Option<DeviceClassFn>,
    /// Test and benchmark hook: every reader sleeps this long before each file, simulating a slow device.
    pub read_delay: Option<Duration>,
    /// Test and benchmark hook: every read chunk (at most 1 MB) takes at least this long (slow device).
    pub chunk_delay: Option<Duration>,
    /// The read-only file layer (instrumented in tests).
    pub fs: SourceFs,
    /// At open, mark runs left `running` as `interrupted` (crash recovery, ADR-0008). An observer process
    /// (a CLI status query while another process scans the same catalog) turns this off.
    pub recover_interrupted: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            classifier: ClassifierSettings::default(),
            poster_tool: None,
            cpu_threads: default_cpu_threads(),
            memory_cap: DEFAULT_MEMORY_CAP,
            max_readers: MAX_READERS,
            lane_window: WINDOW,
            device_class: None,
            read_delay: None,
            chunk_delay: None,
            fs: SourceFs::new(),
            recover_interrupted: true,
        }
    }
}

impl std::fmt::Debug for ScanOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanOptions")
            .field("cpu_threads", &self.cpu_threads)
            .field("memory_cap", &self.memory_cap)
            .field("max_readers", &self.max_readers)
            .field("lane_window", &self.lane_window)
            .field("read_delay", &self.read_delay)
            .field("chunk_delay", &self.chunk_delay)
            .field("fs", &self.fs)
            .finish_non_exhaustive()
    }
}

/// Cores minus one (at least one).
pub fn default_cpu_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .max(1)
}
