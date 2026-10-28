//! The file-system primitives of cleanup (ADR-0061, ADR-0062). The only place in the workspace that moves a
//! user's file; it never copies one and never replaces an existing path.
//!
//! - [`move_no_replace`]: an atomic rename that fails instead of replacing an existing target, or no move at
//!   all. In order:
//!   1. Linux / Android: `renameat2(RENAME_NOREPLACE)`; macOS / iOS: `renameatx_np(RENAME_EXCL)`, through
//!      `rustix`.
//!   2. Windows: `MoveFileExW` with neither `MOVEFILE_REPLACE_EXISTING` nor `MOVEFILE_COPY_ALLOWED` (so it
//!      never replaces and never copies across volumes), through `windows-sys`.
//!   3. Unix file systems without the no-replace flag (`EINVAL`/`ENOTSUP`/`ENOSYS`: some NFS, FUSE, SMB and
//!      exFAT mounts) and other Unixes: `link(from, to)`, which fails with `EEXIST` when the target exists and
//!      so is atomic and no-replace, then - only after the link succeeded and both names are verified to be
//!      the same file (device and inode) - `unlink(from)`. That unlink removes a second name of a file that
//!      is still reachable at `to`; it is the only removal of a name in this module ([`link_then_unlink`]).
//!   4. Otherwise (no hard links either: FAT, some SMB shares) the move is refused
//!      ([`MoveError::Unsupported`]). There is no check-then-rename: `std::fs::rename` replaces an existing
//!      target, so a file created at the target between a check and the rename would be overwritten.
//!
//!   A cross-volume rename (`EXDEV`, `ERROR_NOT_SAME_DEVICE`) is [`MoveError::CrossVolume`]; an existing target
//!   [`MoveError::Occupied`]. In every error case the file is still at `from` (method 3 may leave it at both
//!   names if the unlink itself fails, never at neither).
//! - [`device_of`]: the volume a path lives on (`st_dev`), for the same-volume check before a move.
//! - [`prepare_dir`]: create the folders of a path inside a root one level at a time, refusing any existing
//!   component that is not a real folder (a symlinked quarantine folder could lead to another volume or tree).
//! - [`remove_empty_dirs`]: after a full undo, `remove_dir` (which fails on a non-empty folder) of the empty
//!   folders inside one run's quarantine folder. Never a file.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use ps_model::IdStability;

use crate::rename::RenameMethod;

/// Why a move did not happen. The file is still at its source path.
#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    #[error("{} already exists; never overwritten", .0.display())]
    Occupied(PathBuf),
    #[error("the quarantine folder is on another volume than the file (a move would copy data)")]
    CrossVolume,
    #[error(
        "this file system has neither an atomic no-replace rename nor hard links ({0}); refusing to move, since \
         a plain rename could overwrite a file that appears at the target"
    )]
    Unsupported(String),
    /// The file is no longer at its path (removed or renamed since it was checked).
    #[error("the file is no longer at its path")]
    SourceGone,
    /// The system refused the move for lack of permission (`EACCES`, `EPERM`): a read-only folder, a file
    /// owned by another user, an immutable file.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// `errno` of `e` is `x`.
#[cfg(unix)]
fn errno_is(e: &io::Error, x: rustix::io::Errno) -> bool {
    e.raw_os_error() == Some(x.raw_os_error())
}

/// Different owners for the file and the folder it is moved into (Unix uid). Linux refuses to hard-link a
/// file the caller does not own with `EPERM` when `fs.protected_hardlinks` is on.
#[cfg(unix)]
fn foreign_owner(from: &Path, to: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let dir = to.parent().unwrap_or(Path::new("."));
    match (fs::symlink_metadata(from), fs::symlink_metadata(dir)) {
        (Ok(f), Ok(d)) => f.uid() != d.uid(),
        _ => false,
    }
}

/// Why a rename (`link = false`) or a `link` (`link = true`) of `from` to `to` failed. The file is still at
/// `from` (or gone from it: [`MoveError::SourceGone`]); nothing here ever retries with a plain rename.
///
/// - `EEXIST`: [`MoveError::Occupied`]; `EXDEV`: [`MoveError::CrossVolume`].
/// - `ENOENT` with `from` gone: [`MoveError::SourceGone`] (otherwise a folder on the way to `to` is gone: I/O).
/// - `EPERM` from `link` on a file we own: [`MoveError::Unsupported`], since file systems without hard links
///   (FAT, exFAT, some shares) answer `EPERM`; on a file another user owns it is a permission problem
///   (`protected_hardlinks`). Every other `EACCES` / `EPERM`: [`MoveError::PermissionDenied`].
/// - `ENOTSUP` / `EOPNOTSUPP` / `ENOSYS` from `link`: [`MoveError::Unsupported`].
/// - Anything else: [`MoveError::Io`].
fn move_failure(e: io::Error, from: &Path, to: &Path, link: bool) -> MoveError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        return MoveError::Occupied(to.to_path_buf());
    }
    if is_cross_device(&e) {
        return MoveError::CrossVolume;
    }
    if e.kind() == io::ErrorKind::NotFound {
        return match fs::symlink_metadata(from) {
            Err(g) if g.kind() == io::ErrorKind::NotFound => MoveError::SourceGone,
            _ => MoveError::Io(e),
        };
    }
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        if link && errno_is(&e, Errno::PERM) && !foreign_owner(from, to) {
            return MoveError::Unsupported(format!("the file system refused a hard link: {e}"));
        }
        if link
            && (errno_is(&e, Errno::NOTSUP)
                || errno_is(&e, Errno::OPNOTSUPP)
                || errno_is(&e, Errno::NOSYS))
        {
            return MoveError::Unsupported(e.to_string());
        }
    }
    #[cfg(not(unix))]
    let _ = link;
    if e.kind() == io::ErrorKind::PermissionDenied {
        return MoveError::PermissionDenied(e.to_string());
    }
    MoveError::Io(e)
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

/// Rename `from` to `to` unless `to` exists, never across volumes, never by a plain rename. See the module
/// docs.
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
                    "{}: no atomic no-replace rename on this file system; moving by link then unlink",
                    to.display()
                );
            }
            Err(e) => return Err(move_failure(e.into(), from, to, false)),
        }
    }
    #[cfg(windows)]
    {
        windows::move_file_no_replace(from, to)
    }
    #[cfg(unix)]
    {
        link_then_unlink(from, to, &|| {})
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (from, to);
        Err(MoveError::Unsupported(
            "no no-replace move on this platform".into(),
        ))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    use super::{MoveError, RenameMethod};

    /// `ERROR_FILE_EXISTS`, `ERROR_ALREADY_EXISTS`.
    const EXISTS: [i32; 2] = [80, 183];

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// `MoveFileExW(from, to, 0)`: without `MOVEFILE_REPLACE_EXISTING` it fails when `to` exists, and without
    /// `MOVEFILE_COPY_ALLOWED` it never copies to another volume.
    pub fn move_file_no_replace(from: &Path, to: &Path) -> Result<RenameMethod, MoveError> {
        let (f, t) = (wide(from), wide(to));
        // SAFETY: both arguments are NUL-terminated UTF-16 strings that live until the call returns; flags 0.
        let ok = unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0;
        if ok {
            return Ok(RenameMethod::NoReplace);
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error().is_some_and(|c| EXISTS.contains(&c)) {
            Err(MoveError::Occupied(to.to_path_buf()))
        } else {
            Err(super::move_failure(e, from, to, false))
        }
    }
}

/// The same file (device and inode) on Unix.
#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Method 3 of [`move_no_replace`]: `link(from, to)` (atomic, fails if `to` exists), then - only once `to` is
/// verified to be the very file `from` was (device and inode, a regular file) - `unlink(from)`. `before_link`
/// runs right before the link (tests use it to create the target in the window a check-then-rename would
/// have). Public so tests exercise it on file systems that do support the no-replace flag.
#[cfg(unix)]
pub fn link_then_unlink(
    from: &Path,
    to: &Path,
    before_link: &dyn Fn(),
) -> Result<RenameMethod, MoveError> {
    let before = fs::symlink_metadata(from)?;
    if !before.file_type().is_file() {
        return Err(MoveError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", from.display()),
        )));
    }
    before_link();
    if let Err(e) = fs::hard_link(from, to) {
        return Err(move_failure(e, from, to, true));
    }
    // Both names must now be the file we checked; otherwise nothing is removed (the file stays at `from`,
    // and the extra name at `to` is reported for the user to look at).
    let (at_from, at_to) = (fs::symlink_metadata(from)?, fs::symlink_metadata(to)?);
    if !(at_to.file_type().is_file() && same_file(&before, &at_from) && same_file(&at_from, &at_to))
    {
        return Err(MoveError::Io(io::Error::other(format!(
            "{} was linked to {}, but they are not the same file afterwards; nothing was removed",
            from.display(),
            to.display()
        ))));
    }
    fs::remove_file(from).map_err(|e| {
        MoveError::Io(io::Error::new(
            e.kind(),
            format!(
                "{} is also at {} now, but its old name could not be removed: {e}",
                from.display(),
                to.display()
            ),
        ))
    })?;
    Ok(RenameMethod::LinkUnlink)
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

/// Whether the filesystem holding `path` keeps file ids across mounts (ADR-0112): `statfs` (Linux `f_type`, with the
/// mount table for FUSE; macOS `f_fstypename`). A failed probe is [`IdStability::Stable`], the check before ADR-0112.
#[cfg(target_os = "linux")]
pub fn file_ids(path: &Path) -> IdStability {
    use ps_model::fs_ids;
    use std::os::unix::fs::MetadataExt;
    let Ok(st) = rustix::fs::statfs(path) else {
        return IdStability::Stable;
    };
    #[allow(clippy::unnecessary_cast)]
    let magic = st.f_type as u64;
    let fuse_type = (magic & 0xffff_ffff == fs_ids::FUSE_SUPER_MAGIC)
        .then(|| {
            let (major, minor) = fs_ids::dev_major_minor(fs::metadata(path).ok()?.dev());
            let text = fs::read_to_string("/proc/self/mountinfo").ok()?;
            fs_ids::mountinfo_entry(&text, major, minor).map(|(t, _)| t)
        })
        .flatten();
    fs_ids::stable_for_magic(magic, fuse_type.as_deref()).ids
}

#[cfg(target_os = "macos")]
pub fn file_ids(path: &Path) -> IdStability {
    let Ok(st) = rustix::fs::statfs(path) else {
        return IdStability::Stable;
    };
    let name: Vec<u8> = st
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    ps_model::fs_ids::stable_for_name(&String::from_utf8_lossy(&name)).ids
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub fn file_ids(_path: &Path) -> IdStability {
    IdStability::Stable
}

/// Windows: no file index is compared ([`file_index_of`] is `None`).
#[cfg(not(unix))]
pub fn file_ids(_path: &Path) -> IdStability {
    IdStability::Unrecorded
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
        #[cfg(unix)]
        assert!(matches!(
            link_then_unlink(&a, &b, &|| {}),
            Err(MoveError::Occupied(_))
        ));
        let c = d.path().join("c");
        move_no_replace(&a, &c).expect("move");
        assert!(!a.exists());
        assert_eq!(fs::read(&c).expect("c"), b"photo");
        assert!(device_of(&c).is_ok());
    }

    /// The fallback for file systems without a no-replace rename: a file that appears at the target right
    /// before the move (the window a check-then-rename had) is never replaced, and the source stays.
    #[cfg(unix)]
    #[test]
    fn scenario_cleanup_target_appearing_mid_move_not_replaced() {
        let d = tempfile::tempdir().expect("tempdir");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fs::write(&a, b"photo").expect("a");
        let appear = || fs::write(&b, b"someone else's file").expect("b");
        let r = link_then_unlink(&a, &b, &appear);
        assert!(matches!(r, Err(MoveError::Occupied(_))), "{r:?}");
        assert_eq!(fs::read(&b).expect("b"), b"someone else's file");
        assert_eq!(fs::read(&a).expect("a"), b"photo");
    }

    /// The fallback keeps the file's identity (it is a rename in two steps) and leaves one name.
    #[cfg(unix)]
    #[test]
    fn fallback_moves_the_same_inode() {
        use std::os::unix::fs::MetadataExt;
        let d = tempfile::tempdir().expect("tempdir");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fs::write(&a, b"photo").expect("a");
        let ino = fs::metadata(&a).expect("a").ino();
        assert_eq!(
            link_then_unlink(&a, &b, &|| {}).expect("move"),
            RenameMethod::LinkUnlink
        );
        assert!(!a.exists());
        let m = fs::metadata(&b).expect("b");
        assert_eq!((m.ino(), m.nlink()), (ino, 1));
        assert_eq!(fs::read(&b).expect("b"), b"photo");
        // A folder is never moved this way.
        fs::create_dir(d.path().join("dir")).expect("dir");
        assert!(link_then_unlink(&d.path().join("dir"), &d.path().join("dir2"), &|| {}).is_err());
        assert!(d.path().join("dir").is_dir() && !d.path().join("dir2").exists());
    }

    /// A failed link or rename says why: source gone, permission, or a file system without the operation;
    /// never "unsupported" for an ordinary error.
    #[cfg(unix)]
    #[test]
    fn move_failures_are_classified() {
        use rustix::io::Errno;
        let d = tempfile::tempdir().expect("tempdir");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let err = |x: Errno| io::Error::from_raw_os_error(x.raw_os_error());
        // `a` does not exist yet.
        assert!(matches!(
            move_failure(err(Errno::NOENT), &a, &b, true),
            MoveError::SourceGone
        ));
        fs::write(&a, b"photo").expect("a");
        // The file is there: a folder on the way to the target is what is missing.
        assert!(matches!(
            move_failure(err(Errno::NOENT), &a, &b, true),
            MoveError::Io(_)
        ));
        for link in [true, false] {
            let r = move_failure(err(Errno::ACCESS), &a, &b, link);
            assert!(matches!(r, MoveError::PermissionDenied(_)), "{r:?}");
            assert!(matches!(
                move_failure(err(Errno::EXIST), &a, &b, link),
                MoveError::Occupied(_)
            ));
            assert!(matches!(
                move_failure(err(Errno::XDEV), &a, &b, link),
                MoveError::CrossVolume
            ));
        }
        // EPERM from link on our own file: hard links are not supported (FAT, exFAT); from a rename: permission.
        let r = move_failure(err(Errno::PERM), &a, &b, true);
        assert!(matches!(r, MoveError::Unsupported(_)), "{r:?}");
        let r = move_failure(err(Errno::PERM), &a, &b, false);
        assert!(matches!(r, MoveError::PermissionDenied(_)), "{r:?}");
        for x in [Errno::NOTSUP, Errno::OPNOTSUPP, Errno::NOSYS] {
            let r = move_failure(err(x), &a, &b, true);
            assert!(matches!(r, MoveError::Unsupported(_)), "{x:?}: {r:?}");
        }
        let r = move_failure(err(Errno::IO), &a, &b, true);
        assert!(matches!(r, MoveError::Io(_)), "{r:?}");
    }

    /// The source disappears right before the link: reported as gone, not as a file system without links.
    #[cfg(unix)]
    #[test]
    fn source_gone_before_the_link_is_not_unsupported() {
        let d = tempfile::tempdir().expect("tempdir");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fs::write(&a, b"photo").expect("a");
        let gone = || fs::remove_file(&a).expect("remove");
        let r = link_then_unlink(&a, &b, &gone);
        assert!(matches!(r, Err(MoveError::SourceGone)), "{r:?}");
        assert!(!b.exists());
    }

    /// A target folder the process may not write to: a permission error, and the file stays.
    #[cfg(unix)]
    #[test]
    fn read_only_target_folder_is_a_permission_error() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().expect("tempdir");
        let a = d.path().join("a");
        let ro = d.path().join("ro");
        fs::write(&a, b"photo").expect("a");
        fs::create_dir(&ro).expect("ro");
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).expect("chmod");
        // Root ignores folder permissions: nothing to check then.
        let probe = ro.join("probe");
        if fs::write(&probe, b"").is_ok() {
            let _ = fs::remove_file(&probe);
            let _ = fs::set_permissions(&ro, fs::Permissions::from_mode(0o755));
            return;
        }
        let b = ro.join("b");
        for r in [link_then_unlink(&a, &b, &|| {}), move_no_replace(&a, &b)] {
            assert!(matches!(r, Err(MoveError::PermissionDenied(_))), "{r:?}");
        }
        assert_eq!(fs::read(&a).expect("a"), b"photo");
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).expect("chmod back");
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
    fn this_machines_target_dir_keeps_file_ids() {
        let ids = file_ids(Path::new(env!("CARGO_MANIFEST_DIR")));
        let want = if cfg!(unix) {
            IdStability::Stable
        } else {
            IdStability::Unrecorded
        };
        assert_eq!(ids, want);
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
