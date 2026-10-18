//! The pure export input: what to export, independent of the scanner and the catalog.

use std::path::PathBuf;

use ps_model::{
    CaptureTime, ContentId, FileId, Format, GroupId, ImageInsight, MediaKind, RelPath, SourceId,
};

/// A registered source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: SourceId,
    /// The source's root folder (absolute).
    pub root: PathBuf,
    /// Reachable now; keepers on offline sources are listed as unavailable and skipped.
    pub online: bool,
    /// For messages ("drive-b").
    pub name: String,
}

/// One file of a keeper set to copy.
#[derive(Clone, Debug, PartialEq)]
pub struct KeeperFile {
    pub file: FileId,
    pub source: SourceId,
    pub rel_path: RelPath,
    pub size: u64,
    pub mtime_ns: i64,
    /// Catalog identity (full or sampled, ADR-0011); the copy is verified against it.
    pub content: ContentId,
    pub format: Option<Format>,
    /// Resolved capture date (DateTimeOriginal, else container date, else file time).
    pub capture: Option<CaptureTime>,
    /// The capture date is only the file modification time (placed under `Undated/`).
    pub undated: bool,
    /// Apple ContentIdentifier pairing a Live Photo still with its video.
    pub content_identifier: Option<String>,
}

impl KeeperFile {
    pub fn name(&self) -> &[u8] {
        self.rel_path.file_name()
    }

    pub fn kind(&self) -> Option<MediaKind> {
        self.format.map(Format::kind)
    }
}

/// Another copy of a unit's content (a duplicate or lesser copy), for provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyRef {
    pub file: FileId,
    pub source: SourceId,
    pub rel_path: RelPath,
}

/// One duplicate group's keeper set (or a singleton): the unit that is exported or not.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportUnit {
    pub group: Option<GroupId>,
    /// The keeper first, then its RAW+JPEG companion (ADR-0014, ADR-0037).
    pub files: Vec<KeeperFile>,
    /// Every catalogued copy of the group, keeper set included.
    pub copies: Vec<CopyRef>,
    /// Human-readable keeper reasons ("Why this copy").
    pub keeper_reasons: Vec<String>,
    /// The effective (user-corrected) insight; advisory, only used for sidecar content (ADR-0021).
    pub insight: Option<ImageInsight>,
    /// User tags.
    pub user_tags: Vec<String>,
    /// User description; falls back to the insight's subject.
    pub description: Option<String>,
}

/// Everything the planner needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportInput {
    pub sources: Vec<SourceInfo>,
    pub units: Vec<ExportUnit>,
}

impl ExportInput {
    pub fn source(&self, id: SourceId) -> Option<&SourceInfo> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// Absolute path of a catalogued file.
    pub fn source_path(&self, source: SourceId, rel: &RelPath) -> Option<PathBuf> {
        self.source(source).map(|s| rel.under(&s.root))
    }
}
