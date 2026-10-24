# 0086. TIFFs above 64 MB are analysed in one sequential pass in their I/O lane (amends 0010, 0011)

- Status: accepted
- Date: 2026-10-24
- Deciders: maintainers
- Amends: [0010](0010-single-read-analysis.md), [0011](0011-content-identity.md)

## Context
ADR-0010 streams files above 64 MB for their identity only; stills then get "preview unavailable (file too large)"
and no perceptual hash, so they match only exact copies. Scanned TIFFs are commonly 100 MB to 1 GB (a 14000x10000
16-bit scan is 979 MB). Reading them twice (identity, then decode) doubles the I/O ADR-0010 exists to avoid, and
holding them in memory does not fit 15 workers at 1M-file scale on a 30 GB machine.

## Decision
A plain TIFF above 64 MB is hashed and decoded from **one sequential read in its I/O lane**:

- The probe's prefix and fetched directories (ADR-0084) are kept in memory. A one-pass reader serves reads inside
  them from memory; every other read must be at or after the physical position: the gap is read through and fed
  to the identity when the identity needs it, else skipped with a seek; a backward read is an error.
- The strip decoder (ADR-0085) visits chunks in file order, so it only moves forward. The identity is fed every
  byte in file order (full BLAKE3 up to 256 MB) or only the size and the three 4 MB windows (sampled, above 256 MB,
  ADR-0011), through an incremental hasher equal to the one-shot one. After the decode, the rest the identity
  needs is read and the end of the file checked.
- The decode holds at most one 32 MB chunk plus the downsampled image; the lane acquires the in-flight budget for
  64 MB (one analysis buffer), so the 512 MB cap and two-buffers-per-reader rule hold.
- The decoded image goes to the CPU worker with the job, which hashes it and makes the thumbnail. A decode error
  (a larger strip, an unsupported colour model, a backward read) gives a placeholder naming the reason and keeps
  the identity; an I/O error or a cancel is a read failure as before.

## Alternatives considered
- **Second read in the CPU worker** - two reads per file and I/O outside the lane's concurrency control.
- **Raise the buffer limit** - memory grows with file size and worker count.
- **Decode only an embedded preview** - most scans have none, or a 160 px thumbnail.

## Consequences
- Large scans get thumbnails, hashes and groups; one read per file still holds.
- The decode's CPU time is spent on the lane thread (about 8 s for a 1 GB LZW scan), delaying that reader's next
  file; other readers of the lane continue, and cancellation is checked on every 1 MB read.
- A file above 256 MB is now read in full when its first image has no usable reduced-resolution level, where the
  sampled identity alone read 12 MB.

## Revisit when
Lanes on slow devices are measured to stall on large TIFF decodes, or other formats above 64 MB need decodes.
