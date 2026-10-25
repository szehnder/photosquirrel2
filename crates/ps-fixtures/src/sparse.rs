//! Large test files that cost almost no disk: sparse files with a few marked bytes (tiff-support gate review),
//! and resizing them to simulate a file that grows or shrinks while it is read. Only for files in temp dirs.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

/// Create a sparse file of `len` bytes at `path` with its offset (4 bytes, little endian) written every
/// `every` bytes and "end" in its last 3 bytes, so a content hash depends on every region it reads.
pub fn marked(path: &Path, len: u64, every: u64) -> io::Result<()> {
    let mut f = File::create(path)?;
    f.set_len(len)?;
    let mut at = 0u64;
    while at < len {
        f.seek(SeekFrom::Start(at))?;
        let n = (len - at).min(4) as usize;
        f.write_all(&(at as u32).to_le_bytes()[..n])?;
        at += every.max(1);
    }
    if len >= 3 {
        f.seek(SeekFrom::Start(len - 3))?;
        f.write_all(b"end")?;
    }
    Ok(())
}

/// Truncate or extend (sparsely) the file at `path` to `len` bytes.
pub fn resize(path: &Path, len: u64) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.set_len(len)
}
