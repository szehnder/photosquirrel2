//! The walk (ADR-0007): list and stat a source tree without reading any file content.
//!
//! Depth-first, each folder's entries sorted by name bytes, so the walk order is the directory order the
//! lanes read in (sequential on spinning disks). Symlinks are never followed (no loops, no escaping the
//! source), the quarantine folder reserved by ADR-0024 is ignored, and unreadable folders are recorded and
//! skipped. Pause and cancel are honoured between folders.

use std::path::{Path, PathBuf};

use ps_model::{RelPath, StatFingerprint};

use crate::control::{Cancelled, Control};
use crate::fs::{EntryKind, SourceFs};

/// The folder name ADR-0024 reserves for reversible cleanup; never scanned.
pub const QUARANTINE_DIR: &str = ".photosquirrel-quarantine";

/// One regular file the walk found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkEntry {
    pub path: RelPath,
    pub stat: StatFingerprint,
    /// Position in walk (directory) order.
    pub order: u32,
    /// The folder also holds a `Thumbs.db` (classifier hint, ADR-0015).
    pub thumbs_db_sibling: bool,
}

/// What a walk produced.
#[derive(Debug, Default)]
pub struct WalkOutput {
    /// Every regular file, sorted by relative-path bytes (the catalog's order, for the merge-join diff).
    pub entries: Vec<WalkEntry>,
    pub dirs: u64,
    pub symlinks_skipped: u64,
    pub other_skipped: u64,
    /// Folders or entries that could not be listed or stat-ed.
    pub errors: Vec<(String, String)>,
}

/// Why a walk stopped early.
#[derive(Debug)]
pub enum WalkError {
    Cancelled,
    /// The source root itself is gone or unreadable.
    RootUnavailable(std::io::Error),
}

impl From<Cancelled> for WalkError {
    fn from(_: Cancelled) -> Self {
        WalkError::Cancelled
    }
}

/// Walk `root`. `on_dir` is told the folder being listed (progress).
pub fn walk(
    fs: &SourceFs,
    root: &Path,
    ctl: &Control,
    mut on_dir: impl FnMut(&str),
) -> Result<WalkOutput, WalkError> {
    let meta = fs.metadata(root).map_err(WalkError::RootUnavailable)?;
    if !meta.is_dir() {
        return Err(WalkError::RootUnavailable(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "source root is not a folder",
        )));
    }
    let mut out = WalkOutput::default();
    // (absolute dir, relative dir)
    let mut stack: Vec<(PathBuf, Option<RelPath>)> = vec![(root.to_path_buf(), None)];
    let mut order = 0u32;
    while let Some((dir, rel)) = stack.pop() {
        ctl.checkpoint()?;
        on_dir(
            &rel.as_ref()
                .map(|r| r.display().into_owned())
                .unwrap_or_default(),
        );
        let entries = match fs.read_dir(&dir) {
            Ok(e) => e,
            Err(e) if rel.is_none() => return Err(WalkError::RootUnavailable(e)),
            Err(e) => {
                out.errors.push((
                    rel.as_ref()
                        .map(|r| r.display().into_owned())
                        .unwrap_or_default(),
                    e.to_string(),
                ));
                continue;
            }
        };
        out.dirs += 1;
        let thumbs_db = entries.iter().any(|e| {
            e.kind == EntryKind::File && e.name.to_string_lossy().eq_ignore_ascii_case("thumbs.db")
        });
        let mut subdirs = Vec::new();
        for e in entries {
            let Some(name_rel) = RelPath::from_path(Path::new(&e.name)) else {
                continue;
            };
            let child = match &rel {
                Some(r) => r.join(&name_rel),
                None => name_rel,
            };
            if let Some(err) = e.error {
                out.errors
                    .push((child.display().into_owned(), err.to_string()));
                continue;
            }
            match e.kind {
                EntryKind::File => {
                    if let Some(stat) = e.stat {
                        out.entries.push(WalkEntry {
                            path: child,
                            stat,
                            order,
                            thumbs_db_sibling: thumbs_db,
                        });
                        order = order.wrapping_add(1);
                    }
                }
                EntryKind::Dir => {
                    if e.name == QUARANTINE_DIR {
                        continue;
                    }
                    subdirs.push((dir.join(&e.name), Some(child)));
                }
                EntryKind::Symlink => out.symlinks_skipped += 1,
                EntryKind::Other => out.other_skipped += 1,
            }
        }
        // Depth-first in name order: push in reverse so the first sub-folder is listed next.
        stack.extend(subdirs.into_iter().rev());
    }
    out.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
