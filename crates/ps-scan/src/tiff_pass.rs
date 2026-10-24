//! The one-pass reader for TIFFs above 64 MB (ADR-0086): one sequential read of the file feeds both the content
//! identity and the strip decoder.
//!
//! The decoder sees a `Read + Seek` file. Reads inside the probe prefix or the directory segments fetched by the
//! probe (ADR-0084) are answered from memory; every other read must be at or after the physical position. The
//! physical reads are whole chunks of at most [`CHUNK`] bytes, each fed to the identity in file order before it is
//! used; bytes the identity does not need (between the windows of a sampled identity) are skipped with a seek
//! when the decoder jumps over them. A backward read is an error the decoder reports as a failed decode, and an
//! I/O error (a cancel included) is kept for the caller, which then handles it as a read failure.

use std::io::{self, Read, Seek, SeekFrom};

use ps_hash::IdentityStream;
use ps_media::tiff::Segment;
use ps_model::ContentId;

use crate::pipeline::CHUNK;

pub(crate) struct OnePass<'a, R: Read + Seek> {
    inner: &'a mut R,
    len: u64,
    prefix: &'a [u8],
    segments: &'a [Segment],
    identity: IdentityStream,
    /// The last chunk read: file bytes `[ahead_at, ahead_at + ahead.len())`.
    ahead: Vec<u8>,
    ahead_at: u64,
    /// Physical position of `inner`: everything before it has been read (or skipped) once.
    phys: u64,
    /// Logical position of the `Read + Seek` view.
    pos: u64,
    /// The first I/O error of `inner` (also a cancel).
    error: Option<io::Error>,
}

impl<'a, R: Read + Seek> OnePass<'a, R> {
    /// `inner` has read exactly `prefix` from the start of the file, and the probe's targeted reads (if any)
    /// fetched `segments`; it is moved back to the end of the prefix here.
    pub(crate) fn new(
        inner: &'a mut R,
        len: u64,
        prefix: &'a [u8],
        segments: &'a [Segment],
    ) -> io::Result<Self> {
        let mut identity = IdentityStream::new(len);
        identity.update(0, prefix)?;
        let phys = prefix.len() as u64;
        inner.seek(SeekFrom::Start(phys))?;
        Ok(Self {
            inner,
            len,
            prefix,
            segments,
            identity,
            ahead: Vec::with_capacity(CHUNK),
            ahead_at: phys,
            phys,
            pos: 0,
            error: None,
        })
    }

    /// The bytes held in memory from `pos` on: the prefix, a fetched segment, or the last chunk.
    fn held(&self, pos: u64) -> Option<&[u8]> {
        let p = usize::try_from(pos).ok()?;
        if p < self.prefix.len() {
            return Some(&self.prefix[p..]);
        }
        if pos >= self.ahead_at && pos < self.ahead_at + self.ahead.len() as u64 {
            return Some(&self.ahead[(pos - self.ahead_at) as usize..]);
        }
        self.segments.iter().find_map(|s| {
            let local = pos.checked_sub(s.offset)?;
            (local < s.bytes.len() as u64).then(|| &s.bytes[local as usize..])
        })
    }

    /// Read the next chunk at the physical position into `ahead`, after skipping up to `target` what the
    /// identity does not need. Returns the bytes read (0 at the end of the file).
    fn next_chunk(&mut self, target: u64) -> io::Result<usize> {
        let skip_to = match self.identity.next_needed() {
            Some(n) => n.min(target),
            None => target,
        };
        if skip_to > self.phys {
            self.inner.seek(SeekFrom::Start(skip_to))?;
            self.phys = skip_to;
        }
        self.ahead.resize(CHUNK, 0);
        let mut n = 0;
        while n < CHUNK {
            match self.inner.read(&mut self.ahead[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.ahead.clear();
                    return Err(e);
                }
            }
        }
        self.ahead.truncate(n);
        self.ahead_at = self.phys;
        self.identity.update(self.phys, &self.ahead)?;
        self.phys += n as u64;
        Ok(n)
    }

    fn fail(&mut self, e: io::Error) -> io::Error {
        let msg = e.to_string();
        self.error.get_or_insert(e);
        io::Error::other(format!("one-pass read failed: {msg}"))
    }

    /// The I/O error (or cancel) the decode ran into, if any.
    pub(crate) fn take_error(&mut self) -> Option<io::Error> {
        self.error.take()
    }

    /// Read what the identity still needs, check the end of the file and return the identity.
    pub(crate) fn finish(mut self) -> io::Result<ContentId> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        while let Some(n) = self.identity.next_needed() {
            if self.next_chunk(n)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "file shrank while it was being read",
                ));
            }
        }
        if self.identity.kind() == ps_model::ContentIdKind::Full {
            let mut one = [0u8; 1];
            self.inner.seek(SeekFrom::Start(self.len))?;
            if self.inner.read(&mut one)? != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file grew while it was being read",
                ));
            }
        }
        self.identity.finish()
    }
}

impl<R: Read + Seek> Read for OnePass<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.error.is_some() {
            return Err(io::Error::other("one-pass read failed earlier"));
        }
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(h) = self.held(self.pos) {
                let n = h.len().min(buf.len());
                buf[..n].copy_from_slice(&h[..n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if self.pos < self.phys {
                return Err(io::Error::other(
                    "the TIFF needs data before the current read position (not in file order)",
                ));
            }
            match self.next_chunk(self.pos) {
                Ok(0) => return Ok(0),
                Ok(_) => {}
                Err(e) => return Err(self.fail(e)),
            }
        }
    }
}

impl<R: Read + Seek> Seek for OnePass<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        self.pos = to;
        Ok(to)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    #[test]
    fn serves_prefix_segments_and_forward_reads_and_hashes_once() {
        let bytes = data(5 * CHUNK + 123);
        let prefix = bytes[..1000].to_vec();
        let tail_at = bytes.len() as u64 - 50;
        let segments = vec![Segment {
            offset: tail_at,
            bytes: bytes[tail_at as usize..].to_vec(),
        }];
        let mut file = Cursor::new(bytes.clone());
        file.seek(SeekFrom::Start(1000)).unwrap();
        let mut pass = OnePass::new(&mut file, bytes.len() as u64, &prefix, &segments).unwrap();
        let mut got = vec![0u8; 10];
        // The end of the file comes from the fetched segment, without moving forward.
        pass.seek(SeekFrom::Start(tail_at + 5)).unwrap();
        pass.read_exact(&mut got).unwrap();
        assert_eq!(
            &got[..],
            &bytes[tail_at as usize + 5..tail_at as usize + 15]
        );
        // The header comes from the prefix.
        pass.seek(SeekFrom::Start(0)).unwrap();
        pass.read_exact(&mut got[..4]).unwrap();
        assert_eq!(&got[..4], &bytes[..4]);
        // Forward data, across a chunk boundary.
        pass.seek(SeekFrom::Start(3 * CHUNK as u64 - 3)).unwrap();
        pass.read_exact(&mut got).unwrap();
        assert_eq!(&got[..], &bytes[3 * CHUNK - 3..3 * CHUNK + 7]);
        // Backward, outside what is held: refused.
        pass.seek(SeekFrom::Start(2000)).unwrap();
        assert!(pass.read(&mut got).is_err());
        assert!(
            pass.take_error().is_none(),
            "a backward read is not an I/O error"
        );
        let id = pass.finish().unwrap();
        assert_eq!(id, ps_hash::identity_from_buffer(&bytes));
    }
}
