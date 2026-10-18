//! The verified copy protocol (ADR-0004, ADR-0011, ADR-0022).
//!
//! For one file:
//! 1. Create the target's folders inside the library ([`LibraryRoot::prepare`]); refuse if the target exists.
//! 2. Open the source **read-only** and check its size against the catalog.
//! 3. Create `<target>.ps-partial` with `create_new` (never opens an existing file).
//! 4. Stream the source in large chunks (4 MB by default) through BLAKE3 into the partial file.
//! 5. Set the source's modification time on the partial, `fsync` it.
//! 6. Read the partial back (Linux: after dropping its cached pages, so the bytes come from the device) and
//!    hash it again: the bytes on disk must equal the bytes streamed. Then compare with the catalog identity:
//!    a full identity must equal the streamed hash; a sampled identity (ADR-0011) is recomputed from the
//!    partial and must match, and the streamed full hash is returned for the ledger.
//! 7. Rename into place with [`rename_no_replace`] and fsync the folder.
//!
//! Any failure removes the partial file (only that file) and reports; nothing appears at the target path.
//! [`CopyHooks`] lets tests inject faults (corrupted bytes, disk full, a kill before the rename).

use std::fs::{File, FileTimes, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ps_model::{ContentId, ContentIdKind, FileId, RelPath};

use crate::library::{LibraryRoot, full_hash_of, partial_of, sync_dir};
use crate::rename::{RenameMethod, rename_no_replace};

/// Copy tuning.
#[derive(Clone, Copy, Debug)]
pub struct CopyConfig {
    /// Read/write chunk size.
    pub buffer_bytes: usize,
    /// Allow the check-then-rename fallback on file systems without an atomic no-replace primitive.
    pub allow_check_then_rename: bool,
}

impl Default for CopyConfig {
    fn default() -> Self {
        Self {
            buffer_bytes: 4 << 20,
            allow_check_then_rename: true,
        }
    }
}

/// Fault-injection points. Production uses [`NoHooks`].
pub trait CopyHooks: Sync {
    /// Called with each chunk after it was hashed and before it is written.
    fn before_write(&self, _file: FileId, _offset: u64, _chunk: &mut [u8]) -> io::Result<()> {
        Ok(())
    }
    /// Called after verification, just before the rename.
    fn before_rename(&self, _file: FileId, _partial: &Path) {}
}

/// No faults.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl CopyHooks for NoHooks {}

/// Why a copy was aborted. In every case no file is left at the target path or its partial path.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("the source changed since it was scanned: {0}")]
    SourceChanged(String),
    #[error("hash mismatch: the catalog has {expected}, the copy has {found}")]
    Mismatch { expected: String, found: String },
    #[error("the copy on disk differs from the bytes read (wrote {streamed}, read back {found})")]
    WriteCorrupted { streamed: String, found: String },
    #[error("the library volume is full: {0}")]
    DiskFull(io::Error),
    #[error("{} already exists and is never overwritten", path.display())]
    TargetExists { path: PathBuf },
    #[error("{op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl CopyError {
    pub fn is_disk_full(&self) -> bool {
        matches!(self, CopyError::DiskFull(_))
    }

    fn io<'p>(op: &'static str, path: &'p Path) -> impl FnOnce(io::Error) -> CopyError + 'p {
        move |source| {
            if is_disk_full(&source) {
                CopyError::DiskFull(source)
            } else if source.kind() == io::ErrorKind::AlreadyExists {
                CopyError::TargetExists {
                    path: path.to_path_buf(),
                }
            } else {
                CopyError::Io {
                    op,
                    path: path.to_path_buf(),
                    source,
                }
            }
        }
    }
}

pub fn is_disk_full(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    )
}

/// One copy to make.
#[derive(Clone, Copy, Debug)]
pub struct CopyRequest<'a> {
    pub file: FileId,
    pub source: &'a Path,
    pub target: &'a RelPath,
    pub content: ContentId,
    pub size: u64,
    pub mtime_ns: i64,
}

/// A completed copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyOutcome {
    /// BLAKE3 of every byte (also for sampled identities).
    pub full_hash: [u8; 32],
    pub bytes: u64,
    pub rename: RenameMethod,
}

/// Removes the partial file unless disarmed (error paths and panics; not a killed process, which the next
/// run's recovery handles).
struct PartialGuard<'a> {
    lib: &'a LibraryRoot,
    rel: RelPath,
    armed: bool,
}

impl Drop for PartialGuard<'_> {
    fn drop(&mut self) {
        if self.armed
            && let Err(e) = self.lib.remove_partial(&self.rel)
        {
            log::error!("could not remove partial {}: {e}", self.rel);
        }
    }
}

pub fn system_time(mtime_ns: i64) -> SystemTime {
    if mtime_ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(mtime_ns as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(mtime_ns.unsigned_abs())
    }
}

fn hex(d: &[u8; 32]) -> String {
    ContentId::full(*d).to_hex()
}

/// Fill `buf` from `r` as far as possible; returns the bytes read (less than `buf.len()` only at EOF).
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Drop the cached pages of a synced file so a read-back comes from the device (Linux).
fn drop_cache(f: &File) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let _ = rustix::fs::fadvise(f, 0, None, rustix::fs::Advice::DontNeed);
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = f;
    }
}

/// Copy one file with the protocol in the module docs.
pub fn copy_verified(
    lib: &LibraryRoot,
    req: &CopyRequest<'_>,
    cfg: &CopyConfig,
    hooks: &dyn CopyHooks,
) -> Result<CopyOutcome, CopyError> {
    let target = lib
        .prepare(req.target)
        .map_err(CopyError::io("create folder for", &lib.path(req.target)))?;
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(CopyError::TargetExists { path: target });
    }

    // Sources are only ever opened read-only (ADR-0004).
    let mut src = File::open(req.source).map_err(CopyError::io("open", req.source))?;
    let len = src
        .metadata()
        .map_err(CopyError::io("stat", req.source))?
        .len();
    if len != req.size {
        return Err(CopyError::SourceChanged(format!(
            "{} is {len} bytes, the catalog has {}",
            req.source.display(),
            req.size
        )));
    }

    let partial_rel = partial_of(req.target);
    let partial = lib.path(&partial_rel);
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(CopyError::io("create", &partial))?;
    let mut guard = PartialGuard {
        lib,
        rel: partial_rel,
        armed: true,
    };

    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; cfg.buffer_bytes.max(64 << 10)];
    let mut copied = 0u64;
    loop {
        let n = read_full(&mut src, &mut buf).map_err(CopyError::io("read", req.source))?;
        if n == 0 {
            break;
        }
        let chunk = &mut buf[..n];
        hasher.update(chunk);
        hooks
            .before_write(req.file, copied, chunk)
            .map_err(CopyError::io("write", &partial))?;
        out.write_all(chunk)
            .map_err(CopyError::io("write", &partial))?;
        copied += n as u64;
    }
    if copied != req.size {
        return Err(CopyError::SourceChanged(format!(
            "{}: read {copied} bytes, expected {}",
            req.source.display(),
            req.size
        )));
    }
    let streamed = *hasher.finalize().as_bytes();
    out.set_times(FileTimes::new().set_modified(system_time(req.mtime_ns)))
        .map_err(CopyError::io("set modification time of", &partial))?;
    out.sync_all().map_err(CopyError::io("fsync", &partial))?;
    drop_cache(&out);
    drop(out);

    // Verify the bytes on disk, then the catalog identity.
    let back = File::open(&partial).map_err(CopyError::io("open", &partial))?;
    let (found, n) = full_hash_of(&back).map_err(CopyError::io("read back", &partial))?;
    if found != streamed || n != copied {
        return Err(CopyError::WriteCorrupted {
            streamed: hex(&streamed),
            found: hex(&found),
        });
    }
    match req.content.kind {
        ContentIdKind::Full => {
            if streamed != req.content.digest {
                return Err(CopyError::Mismatch {
                    expected: req.content.to_hex(),
                    found: hex(&streamed),
                });
            }
        }
        ContentIdKind::Sampled => {
            let sampled = ps_hash::identity_from_reader(&back, copied)
                .map_err(CopyError::io("read back", &partial))?;
            if sampled != req.content {
                return Err(CopyError::Mismatch {
                    expected: req.content.to_hex(),
                    found: sampled.to_hex(),
                });
            }
        }
    }
    drop(back);

    hooks.before_rename(req.file, &partial);
    let method = rename_no_replace(&partial, &target, cfg.allow_check_then_rename)
        .map_err(CopyError::io("rename", &target))?;
    guard.armed = false;
    if let Some(dir) = target.parent() {
        sync_dir(dir).map_err(CopyError::io("fsync folder", dir))?;
    }
    Ok(CopyOutcome {
        full_hash: streamed,
        bytes: copied,
        rename: method,
    })
}

/// Write a new small file (a sidecar) with the same partial, fsync, read-back and no-replace protocol.
pub fn write_new_file(
    lib: &LibraryRoot,
    rel: &RelPath,
    bytes: &[u8],
    cfg: &CopyConfig,
) -> Result<RenameMethod, CopyError> {
    let target = lib
        .prepare(rel)
        .map_err(CopyError::io("create folder for", &lib.path(rel)))?;
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(CopyError::TargetExists { path: target });
    }
    let partial_rel = partial_of(rel);
    let partial = lib.path(&partial_rel);
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(CopyError::io("create", &partial))?;
    let mut guard = PartialGuard {
        lib,
        rel: partial_rel,
        armed: true,
    };
    out.write_all(bytes)
        .map_err(CopyError::io("write", &partial))?;
    out.sync_all().map_err(CopyError::io("fsync", &partial))?;
    drop(out);
    let back = std::fs::read(&partial).map_err(CopyError::io("read back", &partial))?;
    if back != bytes {
        return Err(CopyError::WriteCorrupted {
            streamed: hex(blake3::hash(bytes).as_bytes()),
            found: hex(blake3::hash(&back).as_bytes()),
        });
    }
    let method = rename_no_replace(&partial, &target, cfg.allow_check_then_rename)
        .map_err(CopyError::io("rename", &target))?;
    guard.armed = false;
    if let Some(dir) = target.parent() {
        sync_dir(dir).map_err(CopyError::io("fsync folder", dir))?;
    }
    Ok(method)
}
