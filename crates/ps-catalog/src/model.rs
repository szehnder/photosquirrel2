//! Row and command payload types of the catalog.

use ps_model::{
    CaptureTime, ContentId, ContentIdKind, FileId, Format, MediaKind, Metadata, PerceptualHash,
    PhotoClass, RelPath, RunStatus, ScanRunId, SourceId, SourceStatus, Stage, StatFingerprint,
};

/// A file's natural key: its source and its path relative to the source root (ADR-0006).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileKey {
    pub source: SourceId,
    pub path: RelPath,
}

impl FileKey {
    pub fn new(source: SourceId, path: RelPath) -> Self {
        Self { source, path }
    }
}

/// What the walk saw: a path and its stat fingerprint (stage `discovered`), optionally with results of
/// later stages that are already known (synthetic catalogs, imports). The stage is derived from which
/// results are present, so it can never disagree with them (ADR-0008).
#[derive(Clone, Debug, PartialEq)]
pub struct FileRow {
    pub key: FileKey,
    pub stat: StatFingerprint,
    pub results: Results,
}

impl FileRow {
    pub fn discovered(key: FileKey, stat: StatFingerprint) -> Self {
        Self {
            key,
            stat,
            results: Results::None,
        }
    }
}

/// Stage results carried with a [`FileRow`].
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Results {
    #[default]
    None,
    Probed(ProbeResult),
    Analyzed(ProbeResult, Box<AnalysisResult>),
}

/// Results of the probe stage (ADR-0010/0015): format, class and reasons, stored dimensions.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub format: Option<Format>,
    pub class: PhotoClass,
    pub reasons: Vec<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Not worth a full read (junk): the stage becomes `skipped` instead of `probed`.
    pub skip: bool,
}

impl ProbeResult {
    pub fn media_kind(&self) -> Option<MediaKind> {
        self.format.map(Format::kind)
    }

    pub(crate) fn stage(&self) -> Stage {
        if self.skip {
            Stage::Skipped
        } else {
            Stage::Probed
        }
    }
}

/// Results of the analysis stage (ADR-0010/0011/0012): identity, hashes and full metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisResult {
    pub identity: ContentId,
    /// Absent for files that cannot be decoded to pixels (e.g. videos without a poster frame).
    pub hashes: Option<PerceptualHash>,
    pub metadata: Metadata,
}

impl AnalysisResult {
    /// The capture time used for sorting: the resolved capture date, else the EXIF one.
    pub fn capture(&self) -> Option<CaptureTime> {
        self.metadata
            .capture_date
            .map(|d| d.value)
            .or(self.metadata.fields.capture_time)
    }
}

/// A file failed at some stage (stage `failed`, `attempts` + 1, an error row).
#[derive(Clone, Debug, PartialEq)]
pub struct FileFailure {
    pub key: FileKey,
    /// The stage that was being attempted.
    pub stage: Stage,
    pub message: String,
    pub run: Option<ScanRunId>,
}

/// A new source root (ADR-0006).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NewSource {
    /// Storage bytes of the absolute root ([`ps_model::path_to_bytes`]).
    pub root: Vec<u8>,
    pub volume_uuid: Option<String>,
    pub device_hint: Option<String>,
    pub total_bytes: Option<u64>,
    pub fingerprint: Option<Vec<u8>>,
}

/// A stored source.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRecord {
    pub id: SourceId,
    pub root: Vec<u8>,
    pub volume_uuid: Option<String>,
    pub device_hint: Option<String>,
    pub total_bytes: Option<u64>,
    pub fingerprint: Option<Vec<u8>>,
    pub status: SourceStatus,
    pub added_at_ms: i64,
    pub folded_into: Option<SourceId>,
}

/// A stored scan run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRecord {
    pub id: ScanRunId,
    pub status: RunStatus,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub sources: Vec<SourceId>,
    pub stats: serde_json::Value,
}

/// Everything stored about one file (detail pages, inspector, scanner diff).
#[derive(Clone, Debug, PartialEq)]
pub struct FileRecord {
    pub id: FileId,
    pub key: FileKey,
    pub stat: StatFingerprint,
    pub stage: Stage,
    pub class: Option<PhotoClass>,
    pub reasons: Vec<String>,
    pub format: Option<Format>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub identity: Option<ContentId>,
    pub hashes: Option<PerceptualHash>,
    /// Wall-clock capture milliseconds (see [`CaptureTime`]).
    pub capture_ms: Option<i64>,
    pub capture_undated: bool,
    pub camera_model: Option<String>,
    pub has_gps: bool,
    pub attempts: u32,
    pub missing_since_ms: Option<i64>,
    /// When the file was first catalogued (`None` for rows from before schema version 3).
    pub first_seen_ms: Option<i64>,
}

/// One recorded failure of a file (`file_error` row).
#[derive(Clone, Debug, PartialEq)]
pub struct FileErrorRecord {
    /// The stage that was being attempted.
    pub stage: Stage,
    pub message: String,
    pub run: Option<ScanRunId>,
    pub at_ms: i64,
}

impl FileRecord {
    pub fn media_kind(&self) -> Option<MediaKind> {
        self.format.map(Format::kind)
    }
}

pub(crate) mod codes {
    use super::*;

    pub fn stage(s: Stage) -> i64 {
        match s {
            Stage::Discovered => 0,
            Stage::Probed => 1,
            Stage::Analyzed => 2,
            Stage::Thumbed => 3,
            Stage::Skipped => 4,
            Stage::Failed => 5,
        }
    }

    pub fn stage_from(c: i64) -> Option<Stage> {
        Some(match c {
            0 => Stage::Discovered,
            1 => Stage::Probed,
            2 => Stage::Analyzed,
            3 => Stage::Thumbed,
            4 => Stage::Skipped,
            5 => Stage::Failed,
            _ => return None,
        })
    }

    pub fn kind(k: MediaKind) -> i64 {
        match k {
            MediaKind::Image => 0,
            MediaKind::Video => 1,
        }
    }

    pub fn identity_kind(k: ContentIdKind) -> i64 {
        match k {
            ContentIdKind::Full => 0,
            ContentIdKind::Sampled => 1,
        }
    }

    pub fn identity_kind_from(c: i64) -> ContentIdKind {
        if c == 1 {
            ContentIdKind::Sampled
        } else {
            ContentIdKind::Full
        }
    }

    /// Wall-clock milliseconds of a capture time.
    pub fn capture_ms(t: &CaptureTime) -> i64 {
        t.local_seconds * 1000 + i64::from(t.millis)
    }
}

/// Milliseconds since the Unix epoch, now.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
