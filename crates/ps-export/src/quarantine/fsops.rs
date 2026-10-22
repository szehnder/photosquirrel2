//! The file-system primitives of cleanup (ADR-0061). The only place in the workspace that renames a user's
//! file; it never removes, links or copies one.
//!
//! - [`move_no_replace`]: `renameat2(RENAME_NOREPLACE)` (Linux) / `renameatx_np(RENAME_EXCL)` (macOS) through
//!   `rustix`. Where the flag is unsupported (Windows, some network file systems) check-then-rename with
//!   `std::fs::rename`, which never copies across volumes. A cross-volume rename (`EXDEV`,
//!   `ERROR_NOT_SAME_DEVICE`) is reported as [`MoveError::CrossVolume`]; an existing target as
//!   [`MoveError::Occupied`]. In every error case the file is still at `from`.
//! - [`device_of`]: the volume a path lives on (`st_dev`), for the same-volume check before a move.
//! - [`prepare_dir`]: create the folders of a path inside a root one level at a time, refusing any existing
//!   component that is not a real folder (a symlinked quarantine folder could lead to another volume or tree).
//! - [`remove_empty_dirs`]: after a full undo, `remove_dir` (which fails on a non-empty folder) of the empty
//!   folders inside one run's quarantine folder. Never a file.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::rename::RenameMethod;

/// Why a move did not happen. The file is still at its source path.
#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    #[error("{} already exists; never overwritten", .0.display())]
    Occupied(PathBuf),
    #[error("the quarantine folder is on another volume than the file (a move would copy data)")]
    CrossVolume,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// `ERROR_NOT_SAME_DEVICE` on Windows, `EXDEV` elsewhere.
fn is_cross_device(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::CrossesDevices {
        return true;
    }
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Rename `from` to `to` unless `to` exists, never across volumes. See the module docs.
pub fn move_no_replace(from: &Path, to: &Path) -> Result<RenameMethod, MoveError> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(RenameMethod::NoReplace),
            Err(rustix::io::Errno::EXIST) => return Err(MoveError::Occupied(to.to_path_buf())),
            Err(rustix::io::Errno::XDEV) => return Err(MoveError::CrossVolume),
            Err(
                rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP,
            ) => {
                log::warn!(
                    "{}: no atomic no-replace rename on this file system; checking then renaming",
                    to.display()
                );
            }
            Err(e) => return Err(MoveError::Io(e.into())),
        }
    }
    check_then_rename(from, to)
}

fn check_then_rename(from: &Path, to: &Path) -> Result<RenameMethod, MoveError> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(MoveError::Occupied(to.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::rename(from, to) {
            Ok(()) => Ok(RenameMethod::CheckThenRename),
            Err(e) if is_cross_device(&e) => Err(MoveError::CrossVolume),
            Err(e) => Err(MoveError::Io(e)),
        },
        Err(e) => Err(MoveError::Io(e)),
    }
}

/// The device a path lives on (`None` where the platform does not say; the rename's own cross-volume error is
/// then the check).
pub fn device_of(path: &Path) -> io::Result<Option<u64>> {
    let m = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(m.dev()))
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        Ok(None)
    }
}

/// The file identity (inode) where the platform exposes one.
pub fn file_index_of(m: &fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        None
    }
}

fn not_a_folder(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "refusing to move through {}: not a real folder inside the source",
            path.display()
        ),
    )
}

/// Create the folders `rel_dir` (relative, normal components only) inside `root` one level at a time and
/// return the folder. Every existing component must be a real directory, not a symlink.
pub fn prepare_dir(root: &Path, rel_dir: &Path) -> io::Result<PathBuf> {
    let mut dir = root.to_path_buf();
    for comp in rel_dir.components() {
        let Component::Normal(name) = comp else {
            return Err(not_a_folder(&dir));
        };
        dir.push(name);
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(_) => return Err(not_a_folder(&dir)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !fs::symlink_metadata(&dir)?.file_type().is_dir() {
                        return Err(not_a_folder(&dir));
                    }
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    Ok(dir)
}

/// Every folder on the way from `root` to `rel_dir` exists and is a real folder (no symlink), so a path below
/// it is inside `root`.
pub fn real_dirs(root: &Path, rel_dir: &Path) -> io::Result<bool> {
    let mut dir = root.to_path_buf();
    for comp in rel_dir.components() {
        let Component::Normal(name) = comp else {
            return Ok(false);
        };
        dir.push(name);
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Remove the empty folders below `run_dir` (deepest first) with `remove_dir`, which refuses a folder that
/// holds anything. `run_dir` itself (it holds the journal) and every file stay. Returns how many folders were
/// removed.
pub fn remove_empty_dirs(run_dir: &Path) -> usize {
    fn walk(dir: &Path, removed: &mut usize) -> bool {
        let Ok(rd) = fs::read_dir(dir) else {
            return false;
        };
        let mut empty = true;
        for e in rd.flatten() {
            let p = e.path();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && walk(&p, removed) && fs::remove_dir(&p).is_ok() {
                *removed += 1;
            } else {
                empty = false;
            }
        }
        empty
    }
    let mut removed = 0;
    walk(run_dir, &mut removed);
    removed
}

/// fsync a folder so a rename in it is durable (Unix; a no-op elsewhere). Failures are logged, not fatal:
/// the rename already happened and the journal records it.
pub fn sync_dir_logged(dir: &Path) {
    if let Err(e) = crate::library::sync_dir(dir) {
        log::warn!("fsync of {} failed: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_replaces_and_moves_within_a_volume() {
        let d = tempfile::tempdir().expect("tempdir");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fs::write(&a, b"photo").expect("a");
        fs::write(&b, b"other").expect("b");
        assert!(matches!(
            move_no_replace(&a, &b),
            Err(MoveError::Occupied(_))
        ));
        assert_eq!(fs::read(&b).expect("b"), b"other");
        assert_eq!(fs::read(&a).expect("a"), b"photo");
        assert!(matches!(
            check_then_rename(&a, &b),
            Err(MoveError::Occupied(_))
        ));
        let c = d.path().join("c");
        move_no_replace(&a, &c).expect("move");
        assert!(!a.exists());
        assert_eq!(fs::read(&c).expect("c"), b"photo");
        assert!(device_of(&c).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn prepare_dir_refuses_symlinked_components() {
        let d = tempfile::tempdir().expect("tempdir");
        let root = d.path().join("root");
        let outside = d.path().join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, root.join(".photosquirrel-quarantine")).expect("link");
        assert!(prepare_dir(&root, Path::new(".photosquirrel-quarantine/1/a")).is_err());
        assert!(!outside.join("1").exists());
        assert!(!real_dirs(&root, Path::new(".photosquirrel-quarantine")).expect("check"));
        let ok = prepare_dir(&root, Path::new("q/1/a")).expect("prepare");
        assert!(ok.is_dir());
        assert!(real_dirs(&root, Path::new("q/1/a")).expect("check"));
    }

    #[test]
    fn remove_empty_dirs_keeps_files() {
        let d = tempfile::tempdir().expect("tempdir");
        let run = d.path().join("run");
        fs::create_dir_all(run.join("a/b/c")).expect("dirs");
        fs::create_dir_all(run.join("x/y")).expect("dirs");
        fs::write(run.join("x/y/kept.jpg"), b"kept").expect("file");
        fs::write(run.join("journal.jsonl"), b"{}").expect("journal");
        assert_eq!(remove_empty_dirs(&run), 3);
        assert!(!run.join("a").exists());
        assert!(run.join("x/y/kept.jpg").is_file());
        assert!(run.join("journal.jsonl").is_file());
    }
}
