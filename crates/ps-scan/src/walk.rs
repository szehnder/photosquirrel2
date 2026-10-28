//! The walk (ADR-0007): list and stat a source tree without reading any file content.
//!
//! Depth-first, each folder's entries sorted by name bytes, so the walk order is the directory order the
//! lanes read in (sequential on spinning disks). Symlinks are never followed (no loops, no escaping the
//! source), the quarantine folder reserved by ADR-0024 is ignored, and unreadable folders are recorded and
//! skipped. Pause and cancel are honoured between folders. A folder that is an export library (it holds
//! `.photosquirrel/manifest.jsonl`, possibly made by another catalog) is never walked: its photos are copies.

use std::path::{Path, PathBuf};

use ps_model::{RelPath, StatFingerprint};

use crate::control::{Cancelled, Control};
use crate::fs::{EntryKind, IdCache, SourceFs};

/// The folder name ADR-0024 reserves for reversible cleanup; never scanned.
pub const QUARANTINE_DIR: &str = ".photosquirrel-quarantine";

/// The folder and file that mark an export library on disk (`ps_export::LIBRARY_META_DIR` and
/// `ps_export::manifest::MANIFEST_FILE`; ps-scan does not depend on ps-export, a ps-service test keeps them equal).
pub const LIBRARY_META_DIR: &str = ".photosquirrel";
pub const LIBRARY_MANIFEST_FILE: &str = "manifest.jsonl";

/// How deep, and through how many folders, [`find_export_library`] looks below a new source root. Deeper
/// libraries are still never scanned: the walk skips them.
pub const LIBRARY_SEARCH_DEPTH: usize = 4;
pub const LIBRARY_SEARCH_MAX_DIRS: usize = 10_000;

/// `dir` is an export library: it holds `.photosquirrel/manifest.jsonl`.
pub fn is_export_library(fs: &SourceFs, dir: &Path) -> bool {
    fs.symlink_metadata(&dir.join(LIBRARY_META_DIR).join(LIBRARY_MANIFEST_FILE))
        .is_ok_and(|m| m.is_file())
}

/// An export library (of this or another catalog) that `root` is, is inside, or contains within
/// [`LIBRARY_SEARCH_DEPTH`] levels. Symlinks are not followed.
pub fn find_export_library(fs: &SourceFs, root: &Path) -> Option<PathBuf> {
    if let Some(lib) = root.ancestors().find(|a| is_export_library(fs, a)) {
        return Some(lib.to_path_buf());
    }
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut seen = 0usize;
    // One file-id probe per device for the whole search, as in the walk (ADR-0112).
    let mut ids = IdCache::default();
    while let Some((dir, depth)) = queue.pop_front() {
        seen += 1;
        if seen > LIBRARY_SEARCH_MAX_DIRS {
            break;
        }
        let Ok(entries) = fs.read_dir_cached(&dir, &mut ids) else {
            continue;
        };
        for e in entries.into_iter().filter(|e| e.kind == EntryKind::Dir) {
            let child = dir.join(&e.name);
            if e.name == LIBRARY_META_DIR {
                if is_export_library(fs, &dir) {
                    return Some(dir);
                }
            } else if depth < LIBRARY_SEARCH_DEPTH {
                queue.push_back((child, depth + 1));
            }
        }
    }
    None
}

/// One regular file the walk found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkEntry {
    pub path: RelPath,
    pub stat: StatFingerprint,
    /// Position in walk (directory) order.
    pub order: u32,
    /// The folder also holds a `Thumbs.db` (classifier hint, ADR-0015).
    pub thumbs_db_sibling: bool,
    /// On FAT local time (ADR-0112): a whole-hour shift of an equal-size file is not a change.
    pub fat_time: bool,
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
    /// Export libraries found inside the source and not walked (relative paths).
    pub libraries_skipped: Vec<String>,
    /// Hidden folders (names starting with `.`) not walked because [`WalkOptions::include_hidden`] is off.
    pub hidden_dirs_skipped: u64,
}

/// What a walk lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkOptions {
    /// Walk folders whose names start with `.` (ADR-0052).
    pub include_hidden: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            include_hidden: true,
        }
    }
}

/// A hidden folder by name (`.thumbnails`, `.git`): Unix convention, which Windows and macOS sources follow
/// for the folders that matter here.
pub fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
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
    options: WalkOptions,
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
    // One file-id probe per device for the whole walk (ADR-0112).
    let mut ids = IdCache::default();
    while let Some((dir, rel)) = stack.pop() {
        ctl.checkpoint()?;
        on_dir(
            &rel.as_ref()
                .map(|r| r.display().into_owned())
                .unwrap_or_default(),
        );
        let entries = match fs.read_dir_cached(&dir, &mut ids) {
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
        if let Some(r) = &rel
            && entries
                .iter()
                .any(|e| e.kind == EntryKind::Dir && e.name == LIBRARY_META_DIR)
            && is_export_library(fs, &dir)
        {
            log::warn!(
                "{} is an export library; its photos are copies and are not scanned",
                dir.display()
            );
            out.libraries_skipped.push(r.display().into_owned());
            continue;
        }
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
                            fat_time: e.fat_time,
                        });
                        order = order.wrapping_add(1);
                    }
                }
                EntryKind::Dir => {
                    if e.name == QUARANTINE_DIR {
                        continue;
                    }
                    if !options.include_hidden && is_hidden_name(&e.name) {
                        out.hidden_dirs_skipped += 1;
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
