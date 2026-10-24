//! Read-only file access (ADR-0004). This is the only way `ps-media` opens files.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Open `path` for reading only. Never creates, truncates or writes. On Linux it asks for `O_NOATIME` (so a
/// scan does not even touch access times) and falls back when the kernel refuses it (files we do not own).
pub fn open_read_only(path: &Path) -> io::Result<File> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOATIME: i32 = 0o1_000_000;
        if let Ok(f) = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOATIME)
            .open(path)
        {
            return Ok(f);
        }
    }
    OpenOptions::new().read(true).open(path)
}

/// Read at most `limit` bytes from the start of `r`.
pub fn read_prefix<R: Read>(r: R, limit: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(limit.min(1 << 20));
    r.take(limit as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Read a whole file read-only (the analyzer's single full read, ADR-0010).
pub fn read_all(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = open_read_only(path)?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// A reader that counts the bytes read through it (used to assert the 256 KB probe bound).
#[derive(Debug)]
pub struct CountingReader<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R> CountingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A handle that keeps reporting the count after the reader is consumed.
    pub fn counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.count)
    }

    pub fn bytes_read(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}
