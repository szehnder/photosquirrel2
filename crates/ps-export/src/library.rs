//! The library folder: read-only probing for the planner ([`FsLibrary`]) and the write guard for the
//! executor ([`LibraryRoot`]).
//!
//! [`LibraryRoot`] is the only way this crate builds a path it writes to. It joins a validated [`RelPath`]
//! (no `..`, no absolute parts) onto the canonical root and creates missing folders one level at a time,
//! refusing any existing component that is not a real directory (a symlink could point outside the library).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use ps_model::{ContentId, ContentIdKind, LedgerEntry, RelPath};

use crate::disposition::CopyCheck;

/// Suffix of in-flight copies.
pub const PARTIAL_SUFFIX: &str = ".ps-partial";

/// What is at a library path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    Absent,
    File {
        size: u64,
        /// Modification time, ns since the Unix epoch (`None` when the file system has none).
        mtime_ns: Option<i64>,
    },
    /// A directory, symlink or special file: the name is taken.
    Other,
}

/// Read-only view of the library the planner consults. Tests substitute fakes.
///
/// [`LibraryProbe::existing`] only stats; [`LibraryProbe::identical`] reads the file's contents and is
/// the expensive call: planning makes it only for adoption candidates (a same-size file at a target that the
/// ledger does not know) and for ledger rows whose copy no longer has its recorded modification time
/// ([`check_copy`], ADR-0044).
pub trait LibraryProbe {
    fn existing(&self, rel: &RelPath) -> io::Result<Existing>;
    /// The file at `rel` has exactly this content (full BLAKE3, or the sampled identity for sampled ones).
    fn identical(&self, rel: &RelPath, content: &ContentId, size: u64) -> io::Result<bool>;
    /// Bytes available to this user on the library volume; `None` when unknown.
    fn free_space(&self) -> io::Result<Option<u64>>;
}

/// `rel` with [`PARTIAL_SUFFIX`] appended to its last component.
pub fn partial_of(rel: &RelPath) -> RelPath {
    let mut v = rel.as_bytes().to_vec();
    v.extend_from_slice(PARTIAL_SUFFIX.as_bytes());
    RelPath::from_bytes(v).unwrap_or_else(|| rel.clone())
}

/// The library on the file system, probed read-only.
#[derive(Clone, Debug)]
pub struct FsLibrary {
    root: PathBuf,
}

impl FsLibrary {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl LibraryProbe for FsLibrary {
    fn existing(&self, rel: &RelPath) -> io::Result<Existing> {
        match fs::symlink_metadata(rel.under(&self.root)) {
            Ok(m) if m.file_type().is_file() => Ok(Existing::File {
                size: m.len(),
                mtime_ns: mtime_ns_of(&m),
            }),
            Ok(_) => Ok(Existing::Other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Existing::Absent),
            // A file where a folder is expected ("2011" is a file): the name is unusable.
            Err(e) if e.kind() == io::ErrorKind::NotADirectory => Ok(Existing::Other),
            Err(e) => Err(e),
        }
    }

    fn identical(&self, rel: &RelPath, content: &ContentId, size: u64) -> io::Result<bool> {
        let path = rel.under(&self.root);
        let m = fs::symlink_metadata(&path)?;
        if !m.file_type().is_file() || m.len() != size {
            return Ok(false);
        }
        let file = File::open(&path)?;
        let found = match content.kind {
            ContentIdKind::Full => ContentId::full(full_hash_of(file)?.0),
            ContentIdKind::Sampled => ps_hash::identity_from_reader(file, size)?,
        };
        Ok(found == *content)
    }

    fn free_space(&self) -> io::Result<Option<u64>> {
        free_space(&self.root)
    }
}

/// Modification time of `m` in ns since the Unix epoch.
pub fn mtime_ns_of(m: &fs::Metadata) -> Option<i64> {
    let t = m.modified().ok()?;
    Some(match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).ok()?,
        Err(e) => -i64::try_from(e.duration().as_nanos()).ok()?,
    })
}

/// Check the library copy a `done` ledger row describes (ADR-0044), reading as little as possible:
///
/// - absent, or not a regular file: [`CopyCheck::Missing`];
/// - another size: [`CopyCheck::SizeMismatch`] (nothing read);
/// - `verify`: re-hash it ([`CopyCheck::Ok`] or [`CopyCheck::HashMismatch`]);
/// - the recorded modification time still matches, or the row predates recorded times: trusted, nothing read;
/// - the time changed but the size did not: re-hash this one file to decide.
///
/// The hash compared is the row's full BLAKE3 when recorded, else the catalog identity (full or sampled).
pub fn check_copy(
    probe: &dyn LibraryProbe,
    e: &LedgerEntry,
    verify: bool,
) -> io::Result<CopyCheck> {
    let (size, mtime_ns) = match probe.existing(&e.library_rel_path)? {
        Existing::File { size, mtime_ns } => (size, mtime_ns),
        Existing::Absent | Existing::Other => return Ok(CopyCheck::Missing),
    };
    if size != e.size {
        return Ok(CopyCheck::SizeMismatch {
            expected: e.size,
            found: size,
        });
    }
    let unchanged = match e.library_mtime_ns {
        Some(recorded) => mtime_ns == Some(recorded),
        None => true,
    };
    if unchanged && !verify {
        return Ok(CopyCheck::Ok);
    }
    let expect = e.full_hash.map_or(e.content, ContentId::full);
    Ok(if probe.identical(&e.library_rel_path, &expect, e.size)? {
        CopyCheck::Ok
    } else {
        CopyCheck::HashMismatch
    })
}

/// BLAKE3 of a whole reader and its length.
pub fn full_hash_of(mut r: impl Read) -> io::Result<([u8; 32], u64)> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((*h.finalize().as_bytes(), n))
}

/// Free bytes for this user on the volume holding `path` (or its nearest existing ancestor).
pub fn free_space(path: &Path) -> io::Result<Option<u64>> {
    let mut p = path;
    while !p.exists() {
        match p.parent() {
            Some(parent) => p = parent,
            None => return Ok(None),
        }
    }
    free_space_at(p)
}

#[cfg(unix)]
fn free_space_at(p: &Path) -> io::Result<Option<u64>> {
    let s = rustix::fs::statvfs(p).map_err(io::Error::from)?;
    Ok(Some(s.f_bavail.saturating_mul(s.f_frsize)))
}

#[cfg(not(unix))]
fn free_space_at(_p: &Path) -> io::Result<Option<u64>> {
    // GetDiskFreeSpaceExW needs FFI; unknown free space does not block the plan and disk-full handling
    // during the copy is the backstop (export design, risks).
    Ok(None)
}

/// Make a path absolute and resolve symlinks of its existing part (the rest is appended lexically).
pub fn normalize(path: &Path) -> io::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut existing = abs.as_path();
    let mut rest = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(c) => {
                let mut out = c;
                for comp in rest.iter().rev() {
                    out.push(comp);
                }
                return Ok(lexical(&out));
            }
            Err(_) => match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    rest.push(name.to_os_string());
                    existing = parent;
                }
                _ => return Ok(lexical(&abs)),
            },
        }
    }
}

fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Error when a write would leave the library.
fn escape(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "refusing to write through {}: not a real folder inside the library",
            path.display()
        ),
    )
}

/// The library root, for writing. See the module docs.
#[derive(Clone, Debug)]
pub struct LibraryRoot {
    root: PathBuf,
}

impl LibraryRoot {
    /// Create the library folder if needed and canonicalize it.
    pub fn open_or_create(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(path)?;
        let root = fs::canonicalize(path)?;
        if !fs::metadata(&root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a folder", root.display()),
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path of `rel` (no I/O).
    pub fn path(&self, rel: &RelPath) -> PathBuf {
        rel.under(&self.root)
    }

    /// Create the folders above `rel` one level at a time and return the file's absolute path. Every existing
    /// component must be a real directory (not a symlink), so the result is inside the root.
    pub fn prepare(&self, rel: &RelPath) -> io::Result<PathBuf> {
        let mut dir = self.root.clone();
        if let Some(parent) = rel.parent() {
            for comp in parent.to_path_buf().components() {
                let Component::Normal(name) = comp else {
                    return Err(escape(&dir));
                };
                dir.push(name);
                match fs::symlink_metadata(&dir) {
                    Ok(m) if m.file_type().is_dir() => {}
                    Ok(_) => return Err(escape(&dir)),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&dir) {
                        Ok(()) => {}
                        // Another thread created it meanwhile; re-check it is a real folder.
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                            if !fs::symlink_metadata(&dir)?.file_type().is_dir() {
                                return Err(escape(&dir));
                            }
                        }
                        Err(e) => return Err(e),
                    },
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(self.path(rel))
    }

    /// Remove one of this exporter's own partial files. Refuses anything that is not a regular file named
    /// `*.ps-partial` in a real folder inside the library.
    pub fn remove_partial(&self, partial: &RelPath) -> io::Result<bool> {
        if !partial.as_bytes().ends_with(PARTIAL_SUFFIX.as_bytes()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{partial} is not a partial file"),
            ));
        }
        let path = self.path(partial);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => return Err(escape(&path)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        let parent = path.parent().map(fs::canonicalize).transpose()?;
        if !parent.is_some_and(|p| p.starts_with(&self.root)) {
            return Err(escape(&path));
        }
        fs::remove_file(&path)?;
        Ok(true)
    }
}

/// fsync a directory so a rename in it is durable (Unix; a no-op elsewhere).
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_handles_missing_tail() {
        let d = tempfile::tempdir().expect("tempdir");
        let n = normalize(&d.path().join("a/../b/c")).expect("normalize");
        assert_eq!(
            n,
            fs::canonicalize(d.path())
                .expect("canon")
                .join("b")
                .join("c")
        );
    }

    #[cfg(unix)]
    #[test]
    fn prepare_refuses_symlinked_folder() {
        let d = tempfile::tempdir().expect("tempdir");
        let outside = d.path().join("outside");
        fs::create_dir(&outside).expect("mkdir");
        let lib = LibraryRoot::open_or_create(&d.path().join("lib")).expect("lib");
        std::os::unix::fs::symlink(&outside, lib.root().join("2011")).expect("symlink");
        let rel = RelPath::from_bytes(b"2011/2011-08-14/a.jpg".to_vec()).expect("rel");
        assert!(lib.prepare(&rel).is_err());
        assert!(!outside.join("2011-08-14").exists());
        // A partial reached through the symlink is outside the library: never removed.
        fs::write(outside.join("x.ps-partial"), b"user data").expect("write");
        let p = RelPath::from_bytes(b"2011/x.ps-partial".to_vec()).expect("rel");
        assert!(lib.remove_partial(&p).is_err());
        assert!(outside.join("x.ps-partial").exists());
        assert!(
            lib.remove_partial(&RelPath::from_bytes(b"a.jpg".to_vec()).expect("rel"))
                .is_err()
        );
    }
}
