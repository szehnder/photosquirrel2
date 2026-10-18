//! Rename that never replaces an existing file.
//!
//! `std::fs::rename` replaces the target on every platform (`rename(2)`; `MoveFileExW` with
//! `MOVEFILE_REPLACE_EXISTING` on Windows), so a file appearing at the target between a check and the rename
//! would be overwritten. The methods, in order of preference:
//!
//! 1. **Linux / Android**: `renameat2(RENAME_NOREPLACE)`; **macOS / iOS**: `renameatx_np(RENAME_EXCL)`, both
//!    through `rustix::fs::renameat_with` (safe bindings; the workspace denies `unsafe`). Atomic: the kernel
//!    fails with `EEXIST` instead of replacing. Not every file system supports the flag (some FUSE, older
//!    NFS/CIFS return `EINVAL`/`ENOTSUP`), hence:
//! 2. **Link + unlink** (all platforms, and the primary method on Windows): `link(partial, target)` fails
//!    with `EEXIST`/`ERROR_ALREADY_EXISTS` when the target exists, so it is also atomic and no-replace; then
//!    the partial name is removed. On Windows `std::fs::hard_link` is `CreateHardLinkW`; calling `MoveFileExW`
//!    without `MOVEFILE_REPLACE_EXISTING` would need FFI (`unsafe`), and link+unlink gives the same
//!    guarantee on NTFS/ReFS and SMB shares backed by them.
//! 3. **Check-then-rename** (file systems without hard links: FAT/exFAT, some SMB/NAS): only when the caller
//!    allows it. The target is checked and then `rename` runs; the race window is between the two calls and
//!    only another writer inside the library folder could hit it. Logged and reported per file.
//!
//! Errors: an existing target is always [`io::ErrorKind::AlreadyExists`], and the source is left in place.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// How a file was renamed into place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenameMethod {
    /// `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)`.
    NoReplace,
    /// `link` then `unlink` of the partial name.
    LinkUnlink,
    /// Existence check then `rename` (no atomic no-replace primitive on this file system).
    CheckThenRename,
}

fn exists_err(to: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} already exists; never overwritten", to.display()),
    )
}

/// Rename `from` to `to` unless `to` exists. See the module docs.
pub fn rename_no_replace(
    from: &Path,
    to: &Path,
    allow_check_then_rename: bool,
) -> io::Result<RenameMethod> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(RenameMethod::NoReplace),
            Err(rustix::io::Errno::EXIST) => return Err(exists_err(to)),
            Err(
                rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP,
            ) => {
                log::debug!(
                    "no-replace rename unsupported for {}; trying link+unlink",
                    to.display()
                );
            }
            Err(e) => return Err(e.into()),
        }
    }
    link_unlink(from, to, allow_check_then_rename)
}

/// Methods 2 and 3 (public so tests exercise them on every platform).
pub fn link_unlink(
    from: &Path,
    to: &Path,
    allow_check_then_rename: bool,
) -> io::Result<RenameMethod> {
    match fs::hard_link(from, to) {
        Ok(()) => {
            fs::remove_file(from)?;
            Ok(RenameMethod::LinkUnlink)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(exists_err(to)),
        Err(e) if allow_check_then_rename => {
            log::warn!(
                "{}: no atomic no-replace rename on this file system ({e}); checking then renaming",
                to.display()
            );
            check_then_rename(from, to)
        }
        Err(e) => Err(e),
    }
}

fn check_then_rename(from: &Path, to: &Path) -> io::Result<RenameMethod> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(exists_err(to)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::rename(from, to)?;
            Ok(RenameMethod::CheckThenRename)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_replaces() {
        let d = tempfile::tempdir().expect("tempdir");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fs::write(&a, b"new").expect("a");
        fs::write(&b, b"old").expect("b");
        let e = rename_no_replace(&a, &b, true).expect_err("exists");
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&b).expect("b"), b"old");
        assert!(a.exists());
        let e = link_unlink(&a, &b, true).expect_err("exists");
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&b).expect("b"), b"old");
        let c = d.path().join("c");
        let m = rename_no_replace(&a, &c, false).expect("rename");
        if cfg!(any(target_os = "linux", target_vendor = "apple")) {
            assert_eq!(m, RenameMethod::NoReplace);
        }
        assert!(!a.exists());
        assert_eq!(fs::read(&c).expect("c"), b"new");
        fs::write(&a, b"again").expect("a");
        let d2 = d.path().join("d");
        assert_eq!(
            link_unlink(&a, &d2, false).expect("link"),
            RenameMethod::LinkUnlink
        );
        assert!(!a.exists());
    }
}
