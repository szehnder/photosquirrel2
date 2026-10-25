//! The pure export input: what to export, independent of the scanner and the catalog.

use std::path::PathBuf;

use crate::date::DateOrigin;
use ps_model::{
    CaptureTime, ContentId, DatePrecision, FileId, Format, GroupId, ImageInsight, MediaKind,
    RelPath, SourceId,
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
    /// Filing date (ADR-0043, [`crate::date::resolve`]): the file's own embedded date, else the best date its
    /// duplicate group knows, else a file-name or sidecar date, else the file time.
    pub capture: Option<CaptureTime>,
    /// The filing date is only the file modification time (placed under `Undated/`).
    pub undated: bool,
    /// Where `capture` came from (shown in the plan, the CLI and the manifest).
    pub date_origin: DateOrigin,
    /// Second, day or month: a month-precision date files under `<YYYY>/<YYYY-MM>/` (ADR-0069).
    pub precision: DatePrecision,
    /// Apple ContentIdentifier pairing a Live Photo still with its video.
    pub content_identifier: Option<String>,
    /// The name to give the library copy when it differs from the source name: a keeper called
    /// "Copy of IMG_5437.JPG" whose group also has "IMG_5437.JPG" is filed under the cleaner name
    /// ([`crate::layout::cleaner_name`]). `None`: the source name.
    pub library_name: Option<Vec<u8>>,
    /// Where the file is now, relative to its source root, when that is not `rel_path`: a cleanup run moved
    /// it into the quarantine folder (ADR-0034) and its library copy has since gone, so it is exported again
    /// from there (ADR-0066). Read-only like every source read; the file is never moved back by export.
    pub read_from: Option<RelPath>,
    /// Pixel size, when the catalog knows it (shown for better copies, ADR-0092).
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl KeeperFile {
    /// The file name in the library (before any collision suffix).
    pub fn name(&self) -> &[u8] {
        self.library_name
            .as_deref()
            .unwrap_or_else(|| self.rel_path.file_name())
    }

    pub fn kind(&self) -> Option<MediaKind> {
        self.format.map(Format::kind)
    }

    /// The path to read the file from, relative to its source root.
    pub fn read_path(&self) -> &RelPath {
        self.read_from.as_ref().unwrap_or(&self.rel_path)
    }
}

/// Another member of a unit's group (a duplicate or lesser copy): provenance, and the earlier library copy
/// that covers the group when its keeper changed after an export (ADR-0092).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyRef {
    pub file: FileId,
    pub source: SourceId,
    pub rel_path: RelPath,
    /// The member's catalog identity (no file is read to know it); `None` for a file without one.
    pub content: Option<ContentId>,
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
