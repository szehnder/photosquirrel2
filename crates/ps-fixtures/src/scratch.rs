//! File operations on test corpora in temporary folders (gate re-review fix 11): writing, padding, re-timing,
//! copying, moving, linking, removing and chmod-ing fixture files. `ps-scan`'s tests call these instead of
//! `std::fs`, so `cargo xtask layering`'s read-only check covers `crates/ps-scan/tests` as well as its sources
//! (ADR-0004: the scanner never writes; its tests only arrange temporary corpora through `ps-fixtures`).
//!
//! Never pass a real user folder: every caller works in a `tempfile` directory.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// Write `bytes` to `path` (created or replaced).
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    fs::write(path, bytes)
}

/// Grow (sparsely) or cut the file at `path` to `len` bytes.
pub fn pad(path: impl AsRef<Path>, len: u64) -> io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.set_len(len)
}

/// Set the modification time of the file at `path`.
pub fn set_mtime(path: impl AsRef<Path>, t: SystemTime) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_modified(t)
}

/// Copy a file (contents and permissions).
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    fs::copy(from, to)
}

/// Move a file or folder (a drive "unplugged" or "remounted", a file moved within a corpus).
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    fs::rename(from, to)
}

/// Delete a fixture file.
pub fn remove(path: impl AsRef<Path>) -> io::Result<()> {
    fs::remove_file(path)
}

/// Hard-link `to` to the existing file `from`.
pub fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    fs::hard_link(from, to)
}

/// Set Unix permission bits (no-op elsewhere).
pub fn set_mode(path: impl AsRef<Path>, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_operations_work_on_a_temp_corpus() {
        let tmp = tempfile::tempdir().expect("tmp");
        let a = tmp.path().join("a.bin");
        write(&a, b"abc").expect("write");
        pad(&a, 10).expect("pad");
        assert_eq!(fs::metadata(&a).expect("md").len(), 10);
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        set_mtime(&a, t).expect("mtime");
        assert_eq!(fs::metadata(&a).expect("md").modified().expect("m"), t);
        let b = tmp.path().join("b.bin");
        copy(&a, &b).expect("copy");
        let c = tmp.path().join("c.bin");
        hard_link(&b, &c).expect("link");
        let d = tmp.path().join("d.bin");
        rename(&c, &d).expect("rename");
        set_mode(&d, 0o644).expect("mode");
        remove(&d).expect("remove");
        assert!(!d.exists() && b.exists());
    }
}
