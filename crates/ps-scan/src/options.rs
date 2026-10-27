//! Engine options.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ps_media::LocalZone;
use ps_model::{ClassifierSettings, DeviceClass};

use crate::fs::SourceFs;
use crate::lane::{MAX_READERS, WINDOW};
use crate::memory::{DEFAULT_DECODE_MEMORY_CAP, DEFAULT_MEMORY_CAP};

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
    /// Walk hidden folders (names starting with `.`) too (ADR-0052: on by default, so nothing a source holds is
    /// missed; `scanning.include_hidden` in settings).
    pub include_hidden: bool,
    /// CPU workers: by default the cores minus one, leaving one for the UI (ADR-0009).
    pub cpu_threads: usize,
    /// Global cap on file buffers in flight (512 MB).
    pub memory_cap: u64,
    /// Cap on the working memory of HEIC and AVIF decodes in flight (1 GiB; ADR-0095): estimated from each
    /// file's declared size, so parallel decodes of huge images wait for each other. A single decode larger
    /// than the cap runs alone.
    pub decode_memory_cap: u64,
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
    /// At open, mark runs left `running` as `interrupted` (crash recovery, ADR-0008). Only the catalog's owner
    /// (the process holding its exclusive lock) ever does; an observer never does, whatever this says.
    pub recover_interrupted: bool,
    /// The zone that makes file times and UTC container dates local (ADR-0070). `None`: the machine's zone
    /// ([`ps_media::date::os::OsZone`]), read once per run. Tests inject fixed or daylight-saving rules.
    pub zone: Option<Arc<dyn LocalZone>>,
    /// Re-read, read-only, the probe data (at most [`ps_media::PROBE_LIMIT`] bytes) of unchanged files analysed
    /// before classification facts were stored, and re-classify them (ADR-0071). Off by default: it reads up to
    /// 256 KB per such file.
    pub refresh_analysis: bool,
    /// The decoders of this machine (ADR-0089): HEIC through the operating system's decoder when detection
    /// found one. Default: none (core tests; `ps-service` passes the detected registry).
    pub decoders: Arc<ps_media::Decoders>,
    /// The decoder generations placeholders are stamped with and compared against (ADR-0109): the build's
    /// [`ps_media::decoders::GENERATIONS`]; tests simulate an update by raising one.
    pub generations: ps_model::Generations,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            classifier: ClassifierSettings::default(),
            poster_tool: None,
            include_hidden: true,
            cpu_threads: default_cpu_threads(),
            memory_cap: DEFAULT_MEMORY_CAP,
            decode_memory_cap: DEFAULT_DECODE_MEMORY_CAP,
            max_readers: MAX_READERS,
            lane_window: WINDOW,
            device_class: None,
            read_delay: None,
            chunk_delay: None,
            fs: SourceFs::new(),
            recover_interrupted: true,
            zone: None,
            refresh_analysis: false,
            decoders: Arc::new(ps_media::Decoders::none()),
            generations: ps_media::decoders::GENERATIONS,
        }
    }
}

impl std::fmt::Debug for ScanOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanOptions")
            .field("classifier", &self.classifier)
            .field("poster_tool", &self.poster_tool)
            .field("include_hidden", &self.include_hidden)
            .field("cpu_threads", &self.cpu_threads)
            .field("memory_cap", &self.memory_cap)
            .field("decode_memory_cap", &self.decode_memory_cap)
            .field("max_readers", &self.max_readers)
            .field("lane_window", &self.lane_window)
            .field("read_delay", &self.read_delay)
            .field("chunk_delay", &self.chunk_delay)
            .field("fs", &self.fs)
            .field("zone", &self.zone.is_some())
            .field("refresh_analysis", &self.refresh_analysis)
            .field("decoders", &self.decoders)
            .field("generations", &self.generations)
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
