//! Where generated files go. The generator only ever calls [`CorpusWriter::write`] with a relative,
//! `/`-separated path; the writer decides what that means. [`MemWriter`] keeps everything in memory (unit tests),
//! [`DirWriter`] writes below one empty output folder (CLI, integration tests).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Receives the corpus files. Implementations must be thread-safe: files are written from rayon workers.
pub trait CorpusWriter: Sync {
    /// Store `bytes` at relative path `rel` (`/`-separated, no `..`) with modification time `mtime`
    /// (Unix seconds).
    fn write(&self, rel: &str, bytes: &[u8], mtime: i64) -> io::Result<()>;
}

/// One file captured by a [`MemWriter`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemFile {
    pub bytes: Vec<u8>,
    pub mtime: i64,
}

/// Keeps the corpus in memory, sorted by path.
#[derive(Default)]
pub struct MemWriter {
    files: Mutex<BTreeMap<String, MemFile>>,
}

impl MemWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every file written so far, sorted by path.
    pub fn into_files(self) -> BTreeMap<String, MemFile> {
        self.files.into_inner().unwrap_or_else(|e| e.into_inner())
    }
}

impl CorpusWriter for MemWriter {
    fn write(&self, rel: &str, bytes: &[u8], mtime: i64) -> io::Result<()> {
        check_relative(rel)?;
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        if files.contains_key(rel) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                rel.to_string(),
            ));
        }
        files.insert(
            rel.to_string(),
            MemFile {
                bytes: bytes.to_vec(),
                mtime,
            },
        );
        Ok(())
    }
}

/// Errors creating a [`DirWriter`].
#[derive(Debug, thiserror::Error)]
pub enum DirWriterError {
    #[error("output folder {0} is not empty")]
    NotEmpty(PathBuf),
    #[error("output path {0} exists and is not a folder")]
    NotADirectory(PathBuf),
    #[error("cannot use output folder {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Writes below a single output folder that must be empty (or absent) when the writer is created. Files are
/// created with `create_new`, so nothing that already exists is ever overwritten, and nothing outside the
/// folder is touched.
pub struct DirWriter {
    root: PathBuf,
}

impl DirWriter {
    /// Prepare `root` (creating it if missing). Refuses a non-empty folder.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, DirWriterError> {
        let root = root.as_ref().to_path_buf();
        let io_err = |source| DirWriterError::Io {
            path: root.clone(),
            source,
        };
        match fs::metadata(&root) {
            Ok(m) if !m.is_dir() => return Err(DirWriterError::NotADirectory(root)),
            Ok(_) => {
                if fs::read_dir(&root).map_err(io_err)?.next().is_some() {
                    return Err(DirWriterError::NotEmpty(root));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&root).map_err(io_err)?;
            }
            Err(e) => return Err(io_err(e)),
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl CorpusWriter for DirWriter {
    fn write(&self, rel: &str, bytes: &[u8], mtime: i64) -> io::Result<()> {
        check_relative(rel)?;
        let path = rel.split('/').fold(self.root.clone(), |p, c| p.join(c));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(bytes)?;
        f.set_modified(unix_time(mtime))?;
        Ok(())
    }
}

/// Unix seconds to `SystemTime` (supports times before 1970).
pub fn unix_time(secs: i64) -> SystemTime {
    if secs >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    }
}

fn check_relative(rel: &str) -> io::Result<()> {
    let bad = rel.is_empty()
        || rel.contains('\\')
        || rel
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
        || Path::new(rel)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)));
    if bad {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("corpus path must be relative and normal: {rel}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_writer_rejects_escapes_and_duplicates() {
        let w = MemWriter::new();
        w.write("a/b.jpg", b"x", 5).expect("write");
        assert!(w.write("a/b.jpg", b"y", 5).is_err());
        assert!(w.write("../evil", b"y", 5).is_err());
        assert!(w.write("/abs", b"y", 5).is_err());
        assert!(w.write("a/./b", b"y", 5).is_err());
        let files = w.into_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files["a/b.jpg"].mtime, 5);
    }

    #[test]
    fn dir_writer_sets_mtime_and_refuses_non_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("out");
        let w = DirWriter::new(&out).expect("new");
        w.write("x/y/z.bin", b"abc", 1_400_000_000).expect("write");
        let meta = fs::metadata(out.join("x/y/z.bin")).expect("meta");
        assert_eq!(meta.modified().expect("mtime"), unix_time(1_400_000_000));
        assert!(matches!(
            DirWriter::new(&out),
            Err(DirWriterError::NotEmpty(_))
        ));
    }
}
