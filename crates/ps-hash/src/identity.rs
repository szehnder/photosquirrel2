//! Content identity (ADR-0011, ADR-0010).
//!
//! Files of at most [`FULL_IDENTITY_MAX_BYTES`] (256 MB) get the BLAKE3 hash of every byte. Larger files get a
//! *sampled* identity: BLAKE3 over the file size as 8 little-endian bytes, then the first, middle and last
//! [`SAMPLE_WINDOW_BYTES`] (4 MB). The middle window starts at `len / 2 - 2 MB`.
//!
//! Hashing is single-threaded on purpose: it is I/O-bound, deterministic, and leaves the cores for decoding.

use std::io::{self, Read, Seek, SeekFrom};

use ps_model::limits::{ANALYSIS_BUFFER_MAX_BYTES, FULL_IDENTITY_MAX_BYTES, SAMPLE_WINDOW_BYTES};
use ps_model::{ContentId, ContentIdKind};

/// Streaming chunk size (1 MB).
pub const STREAM_CHUNK_BYTES: usize = 1024 * 1024;

/// Which kind of identity a file of `len` bytes gets.
pub fn identity_kind_for_len(len: u64) -> ContentIdKind {
    if len <= FULL_IDENTITY_MAX_BYTES {
        ContentIdKind::Full
    } else {
        ContentIdKind::Sampled
    }
}

/// Start offsets of the first, middle and last sample windows of a file of `len` bytes (`len` > 256 MB).
pub fn sample_offsets(len: u64) -> [u64; 3] {
    debug_assert!(len >= 3 * SAMPLE_WINDOW_BYTES);
    [
        0,
        len / 2 - SAMPLE_WINDOW_BYTES / 2,
        len - SAMPLE_WINDOW_BYTES,
    ]
}

/// Identity of a whole file held in memory (the analysis buffer). Buffers above 256 MB get the sampled
/// identity, exactly as [`identity_from_reader`] would compute it.
pub fn identity_from_buffer(bytes: &[u8]) -> ContentId {
    let len = bytes.len() as u64;
    match identity_kind_for_len(len) {
        ContentIdKind::Full => ContentId::full(*blake3::hash(bytes).as_bytes()),
        ContentIdKind::Sampled => {
            let mut h = blake3::Hasher::new();
            h.update(&len.to_le_bytes());
            let w = SAMPLE_WINDOW_BYTES as usize;
            for off in sample_offsets(len) {
                let off = off as usize;
                h.update(&bytes[off..off + w]);
            }
            ContentId::sampled(*h.finalize().as_bytes())
        }
    }
}

/// Identity of a file of `len` bytes read from `reader` (positioned anywhere; it is rewound). Full identities
/// stream in 1 MB chunks so no large buffer is held; sampled identities read only the three windows (12 MB).
///
/// Fails with [`io::ErrorKind::UnexpectedEof`] or [`io::ErrorKind::InvalidData`] when the reader does not hold
/// exactly `len` bytes (the file changed while it was being read).
pub fn identity_from_reader<R: Read + Seek>(mut reader: R, len: u64) -> io::Result<ContentId> {
    match identity_kind_for_len(len) {
        ContentIdKind::Full => {
            reader.seek(SeekFrom::Start(0))?;
            full_identity_streamed(reader, len)
        }
        ContentIdKind::Sampled => {
            let mut h = blake3::Hasher::new();
            h.update(&len.to_le_bytes());
            let mut buf = vec![0u8; STREAM_CHUNK_BYTES];
            for off in sample_offsets(len) {
                reader.seek(SeekFrom::Start(off))?;
                let mut left = SAMPLE_WINDOW_BYTES as usize;
                while left > 0 {
                    let n = left.min(buf.len());
                    reader.read_exact(&mut buf[..n])?;
                    h.update(&buf[..n]);
                    left -= n;
                }
            }
            Ok(ContentId::sampled(*h.finalize().as_bytes()))
        }
    }
}

/// Full BLAKE3 identity of exactly `len` bytes streamed from `reader`'s current position.
fn full_identity_streamed<R: Read>(mut reader: R, len: u64) -> io::Result<ContentId> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; STREAM_CHUNK_BYTES.min(len.max(1) as usize)];
    let mut left = len;
    while left > 0 {
        let n = (left.min(buf.len() as u64)) as usize;
        reader.read_exact(&mut buf[..n])?;
        h.update(&buf[..n]);
        left -= n as u64;
    }
    ensure_at_end(&mut reader)?;
    Ok(ContentId::full(*h.finalize().as_bytes()))
}

fn ensure_at_end<R: Read>(reader: &mut R) -> io::Result<()> {
    let mut probe = [0u8; 1];
    loop {
        match reader.read(&mut probe) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file grew while it was being hashed",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// The result of the single analysis read of a file (ADR-0010).
#[derive(Debug)]
pub struct AnalysisRead {
    pub identity: ContentId,
    /// The whole file when it is at most 64 MB: decode from this buffer instead of reading the file again.
    /// `None` for larger files, whose identity was streamed.
    pub bytes: Option<Vec<u8>>,
}

/// Read a file of `len` bytes once for analysis. Files of at most [`ANALYSIS_BUFFER_MAX_BYTES`] (64 MB) are
/// read into one buffer that is hashed here and returned for decoding; larger files are hashed by streaming
/// (full up to 256 MB, sampled above) without holding more than 1 MB.
pub fn read_for_analysis<R: Read + Seek>(mut reader: R, len: u64) -> io::Result<AnalysisRead> {
    if len <= ANALYSIS_BUFFER_MAX_BYTES {
        reader.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(len as usize);
        (&mut reader).take(len).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file shrank while it was being read",
            ));
        }
        ensure_at_end(&mut reader)?;
        Ok(AnalysisRead {
            identity: identity_from_buffer(&bytes),
            bytes: Some(bytes),
        })
    } else {
        Ok(AnalysisRead {
            identity: identity_from_reader(reader, len)?,
            bytes: None,
        })
    }
}
